use axum::Json;
use axum::extract::{Path, State};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use shared::{DeleteResponse, UrlEntry, UrlUpsertRequest, validate_code, validate_url};

use crate::app::AppState;
use crate::db::url_aggregate_pipeline;
use crate::error::AppError;

fn parse_expiry(raw: Option<&str>) -> Result<Option<bson::DateTime>, AppError> {
    let Some(raw) = raw else { return Ok(None) };
    let parsed = chrono::DateTime::parse_from_rfc3339(raw)
        .map_err(|_| AppError::validation("expires_at must be an RFC3339 datetime"))?;
    let millis = parsed.timestamp_millis();
    if millis <= bson::DateTime::now().timestamp_millis() {
        return Err(AppError::validation("expires_at must be in the future"));
    }
    Ok(Some(bson::DateTime::from_millis(millis)))
}

fn is_owned(doc: &Document) -> bool {
    doc.get("owner").is_some_and(|v| !matches!(v, Bson::Null))
}

// Phase 2: when the caller IS the owner, return 409 with the current target
// url instead. Anonymous callers and other owners must never see the url.
fn owned_error(code: &str) -> AppError {
    AppError::forbidden(format!(
        "code '{code}' is already taken by a registered user"
    ))
}

async fn fetch_entry(state: &AppState, code: &str) -> Result<UrlEntry, AppError> {
    let rows: Vec<Document> = state
        .db
        .collection::<Document>("urls")
        .aggregate(url_aggregate_pipeline(doc! {"code": code}, 1, 0, doc! {}))
        .await?
        .try_collect()
        .await?;
    let doc = rows
        .into_iter()
        .next()
        .ok_or_else(|| AppError::internal("upserted url not found"))?;
    bson::from_document(doc).map_err(AppError::internal)
}

async fn upsert(
    state: &AppState,
    code: &str,
    body: &UrlUpsertRequest,
) -> Result<Json<UrlEntry>, AppError> {
    validate_code(code).map_err(AppError::validation)?;
    validate_code(&body.code).map_err(AppError::validation)?;
    validate_url(&body.url).map_err(AppError::validation)?;
    let expires = parse_expiry(body.expires_at.as_deref())?;

    let urls = state.db.collection::<Document>("urls");
    if let Some(existing) = urls.find_one(doc! {"code": code}).await? {
        if is_owned(&existing) {
            return Err(owned_error(code));
        }
    }

    let mut set = doc! {"code": &body.code, "url": &body.url};
    if let Some(expires) = expires {
        set.insert("expires_at", expires);
    }
    urls.update_one(
        doc! {"code": code},
        doc! {"$set": set, "$setOnInsert": {"created_at": bson::DateTime::now()}},
    )
    .upsert(true)
    .await?;
    Ok(Json(fetch_entry(state, &body.code).await?))
}

pub async fn create_url(
    State(state): State<AppState>,
    Json(body): Json<UrlUpsertRequest>,
) -> Result<Json<UrlEntry>, AppError> {
    let code = body.code.clone();
    upsert(&state, &code, &body).await
}

pub async fn update_url(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(body): Json<UrlUpsertRequest>,
) -> Result<Json<UrlEntry>, AppError> {
    upsert(&state, &code, &body).await
}

pub async fn delete_url(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Json<DeleteResponse>, AppError> {
    let urls = state.db.collection::<Document>("urls");
    let existing = urls
        .find_one(doc! {"code": &code})
        .await?
        .ok_or_else(|| AppError::not_found("code not found"))?;
    if is_owned(&existing) {
        return Err(owned_error(&code));
    }
    urls.delete_one(doc! {"code": &code}).await?;
    Ok(Json(DeleteResponse {
        code,
        deleted: true,
    }))
}

pub async fn list_urls() -> AppError {
    AppError::not_implemented()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_expiry_none_passes_through() {
        assert!(parse_expiry(None).unwrap().is_none());
    }

    #[test]
    fn parse_expiry_accepts_future_rfc3339_with_offset() {
        let future = chrono::Utc::now() + chrono::Duration::days(7);
        for raw in [
            future.to_rfc3339(),
            future
                .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap())
                .to_rfc3339(),
        ] {
            let parsed = parse_expiry(Some(&raw)).unwrap().unwrap();
            assert_eq!(parsed.timestamp_millis(), future.timestamp_millis());
        }
    }

    #[test]
    fn parse_expiry_rejects_past_and_garbage() {
        assert!(parse_expiry(Some("2001-01-01T00:00:00Z")).is_err());
        assert!(parse_expiry(Some("not-a-date")).is_err());
        assert!(parse_expiry(Some("2030-01-01")).is_err()); // date without time is not RFC3339
    }

    #[test]
    fn is_owned_matrix() {
        assert!(!is_owned(&doc! {"code": "a"}));
        assert!(!is_owned(&doc! {"code": "a", "owner": Bson::Null}));
        assert!(is_owned(&doc! {"code": "a", "owner": "u1"}));
    }

    #[test]
    fn owned_error_is_403_and_never_contains_url() {
        let err = owned_error("abc");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert!(err.message.contains("'abc'"));
        assert!(!err.message.contains("http"));
    }
}
