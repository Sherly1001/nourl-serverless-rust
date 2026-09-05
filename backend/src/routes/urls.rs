use axum::Json;
use axum::extract::{Path, Query, State};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use shared::{
    DeleteResponse, UrlEntry, UrlListResponse, UrlUpsertRequest, validate_code, validate_url,
};

use crate::app::AppState;
use crate::auth::extract::{CurrentUser, OptionalUser};
use crate::db::url_aggregate_pipeline;
use crate::error::AppError;
use crate::extract::AppJson;
use crate::query::ListParams;
use crate::users::User;

/// What a request says about a link's expiry.
///
/// Three answers rather than two, because `Option<String>` on the wire cannot
/// tell "I am not talking about the expiry" from "remove it" — and the row
/// editor, which sends every field on every save, needs both.
#[derive(Debug)]
enum Expiry {
    /// The field was absent: whatever is stored stays.
    Leave,
    /// An explicit empty string: the link stops expiring.
    Clear,
    At(bson::DateTime),
}

fn parse_expiry(raw: Option<&str>) -> Result<Expiry, AppError> {
    let Some(raw) = raw else {
        return Ok(Expiry::Leave);
    };
    if raw.trim().is_empty() {
        return Ok(Expiry::Clear);
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(raw).map_err(|_| {
        AppError::validation("expires_at must be an RFC3339 datetime").on_field("expires_at")
    })?;
    let millis = parsed.timestamp_millis();
    if millis <= bson::DateTime::now().timestamp_millis() {
        return Err(AppError::validation("expires_at must be in the future").on_field("expires_at"));
    }
    Ok(Expiry::At(bson::DateTime::from_millis(millis)))
}

/// Legacy documents can carry an explicit `owner: null`, which means unowned
/// just as an absent field does.
fn owner_id(doc: &Document) -> Option<&str> {
    match doc.get("owner") {
        Some(Bson::String(id)) => Some(id.as_str()),
        _ => None,
    }
}

/// Someone else's code. The message must stay generic — revealing the target
/// url here would turn the shortener into a lookup service for private links.
fn owned_error(code: &str) -> AppError {
    AppError::forbidden(format!(
        "code '{code}' is already taken by a registered user"
    ))
    .on_field("code")
}

/// The caller's own code. They may see their current target, and the message
/// points them at the edit path instead.
fn own_code_conflict(code: &str, url: &str) -> AppError {
    AppError::conflict(format!(
        "you already use code '{code}' for {url} — edit it instead of recreating it"
    ))
    .on_field("code")
}

/// Who may write to an existing document: nobody owns it, the caller owns it,
/// or the caller is an admin.
fn may_write(existing: &Document, user: Option<&User>) -> bool {
    match owner_id(existing) {
        None => true,
        Some(owner) => user.is_some_and(|u| u.is_admin || u.id == owner),
    }
}

async fn fetch_entry(state: &AppState, code: &str) -> Result<UrlEntry, AppError> {
    let rows: Vec<Document> = state
        .db
        .collection::<Document>("urls")
        .aggregate(url_aggregate_pipeline(
            doc! {"code": code},
            1,
            0,
            doc! {"_id": 1},
        ))
        .await?
        .try_collect()
        .await?;
    let doc = rows
        .into_iter()
        .next()
        .ok_or_else(|| AppError::internal("upserted url not found"))?;
    bson::from_document(doc).map_err(AppError::internal)
}

/// `conflict_on_own` separates "create" from "edit": re-creating a code you
/// already own is a mistake worth a 409, while a PUT at that same code is the
/// edit itself.
async fn upsert(
    state: &AppState,
    code: &str,
    body: &UrlUpsertRequest,
    user: Option<&User>,
    conflict_on_own: bool,
) -> Result<Json<UrlEntry>, AppError> {
    validate_code(code).map_err(|e| AppError::validation(e).on_field("code"))?;
    validate_code(&body.code).map_err(|e| AppError::validation(e).on_field("code"))?;
    validate_url(&body.url).map_err(|e| AppError::validation(e).on_field("url"))?;
    let expires = parse_expiry(body.expires_at.as_deref())?;

    let urls = state.db.collection::<Document>("urls");
    let existing = urls.find_one(doc! {"code": code}).await?;
    let existing_owner = existing
        .as_ref()
        .and_then(|doc| owner_id(doc).map(str::to_string));
    if let Some(existing) = &existing {
        let owned_by_caller = existing_owner
            .as_deref()
            .zip(user)
            .is_some_and(|(owner, u)| owner == u.id);
        if owned_by_caller && conflict_on_own {
            let current = existing.get_str("url").unwrap_or_default();
            return Err(own_code_conflict(code, current));
        }
        if !may_write(existing, user) {
            return Err(owned_error(code));
        }
    }

    // A rename is a write at both ends. `code` carries a unique index, so
    // without this the write would land on it and surface as a 500 — and the
    // destination's owner would never have been consulted at all.
    if body.code != code
        && let Some(target) = urls.find_one(doc! {"code": &body.code}).await?
    {
        match owner_id(&target) {
            // Unowned links are already overwritable and deletable by anyone,
            // so taking the code is no more than a DELETE followed by this
            // same rename. Refusing it would only be friction.
            None => {
                urls.delete_one(doc! {"code": &body.code}).await?;
            }
            // Your own link. Same situation as re-creating a code you own, and
            // it answers the same way rather than quietly destroying the other
            // one.
            Some(owner) if user.is_some_and(|u| u.id == owner) => {
                let current = target.get_str("url").unwrap_or_default();
                return Err(own_code_conflict(&body.code, current));
            }
            // Someone else's. Admins are not excepted: they may edit that link
            // in place, but "may write" is not "may destroy it as a side effect
            // of moving another one".
            Some(_) => return Err(owned_error(&body.code)),
        }
    }

    // Stamped on every write, including the one that creates the link, so an
    // edit is always distinguishable from the original.
    let mut set = doc! {
        "code": &body.code,
        "url": &body.url,
        "updated_at": bson::DateTime::now(),
    };
    // `$unset` rather than a stored null: the TTL index is partial on
    // `expires_at` existing, and a null in it is a date the reaper cannot read.
    let mut unset = Document::new();
    match expires {
        Expiry::Leave => {}
        Expiry::Clear => {
            unset.insert("expires_at", "");
        }
        Expiry::At(at) => {
            set.insert("expires_at", at);
        }
    }
    // Ownership. A brand new link always belongs to whoever made it, which
    // `$setOnInsert` covers. *Creating* over a link nobody owns claims it as
    // well — `$setOnInsert` does not fire when the document already exists, so
    // without this the author would overwrite the link and then not find it in
    // their own list. Editing (PUT) never reassigns ownership, so an admin
    // fixing someone's link does not take it over, and an anonymous write
    // leaves `owner` absent so unowned links stay freely mutable.
    //
    // `owner` must appear in at most one of the two operators: naming it in
    // both makes Mongo reject the update for a conflicting path.
    let mut on_insert = doc! {"created_at": bson::DateTime::now()};
    if let Some(user) = user {
        if conflict_on_own && existing_owner.is_none() {
            set.insert("owner", &user.id);
        } else {
            on_insert.insert("owner", &user.id);
        }
    }
    let mut update = doc! {"$set": set, "$setOnInsert": on_insert};
    if !unset.is_empty() {
        update.insert("$unset", unset);
    }
    urls.update_one(doc! {"code": code}, update)
        .upsert(true)
        .await?;
    Ok(Json(fetch_entry(state, &body.code).await?))
}

pub async fn create_url(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    AppJson(body): AppJson<UrlUpsertRequest>,
) -> Result<Json<UrlEntry>, AppError> {
    let code = body.code.clone();
    upsert(&state, &code, &body, user.as_ref(), true).await
}

pub async fn update_url(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    Path(code): Path<String>,
    AppJson(body): AppJson<UrlUpsertRequest>,
) -> Result<Json<UrlEntry>, AppError> {
    upsert(&state, &code, &body, user.as_ref(), false).await
}

pub async fn delete_url(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    Path(code): Path<String>,
) -> Result<Json<DeleteResponse>, AppError> {
    let urls = state.db.collection::<Document>("urls");
    let existing = urls
        .find_one(doc! {"code": &code})
        .await?
        .ok_or_else(|| AppError::not_found("code not found"))?;
    if !may_write(&existing, user.as_ref()) {
        return Err(owned_error(&code));
    }
    urls.delete_one(doc! {"code": &code}).await?;
    Ok(Json(DeleteResponse {
        code,
        deleted: true,
    }))
}

pub async fn list_urls(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<UrlListResponse>, AppError> {
    let parsed = ListParams::from_query(&params)?;
    // Admins see every link; everyone else sees only their own. `mine=true`
    // asks for the ordinary view regardless, which is what the account page
    // needs: closing an account takes its owner's links with it, and for an
    // admin the unscoped total is the whole site's.
    let mine = params.get("mine").is_some_and(|v| v == "true");
    let owner = (!user.is_admin || mine).then_some(user.id.as_str());
    let filter = parsed.filter(owner);

    let urls = state.db.collection::<Document>("urls");
    let total = urls.count_documents(filter.clone()).await?;
    let rows: Vec<Document> = urls
        .aggregate(url_aggregate_pipeline(
            filter,
            parsed.limit,
            parsed.skip,
            parsed.sort,
        ))
        .await?
        .try_collect()
        .await?;
    let items = rows
        .into_iter()
        .map(|doc| bson::from_document(doc).map_err(AppError::internal))
        .collect::<Result<Vec<UrlEntry>, AppError>>()?;

    Ok(Json(UrlListResponse { items, total }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_expiry_distinguishes_absent_from_empty() {
        assert!(matches!(parse_expiry(None).unwrap(), Expiry::Leave));
        assert!(matches!(parse_expiry(Some("")).unwrap(), Expiry::Clear));
        assert!(matches!(parse_expiry(Some("   ")).unwrap(), Expiry::Clear));
    }

    #[test]
    fn parse_expiry_accepts_future_rfc3339_with_offset() {
        let future = chrono::Utc::now() + chrono::Duration::days(7);
        for raw in [
            future.to_rfc3339(),
            future.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            future
                .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap())
                .to_rfc3339(),
        ] {
            let Expiry::At(parsed) = parse_expiry(Some(&raw)).unwrap() else {
                panic!("a future stamp is an expiry");
            };
            assert_eq!(parsed.timestamp_millis(), future.timestamp_millis());
        }
    }

    #[test]
    fn parse_expiry_rejects_past_and_garbage() {
        assert!(parse_expiry(Some("2001-01-01T00:00:00Z")).is_err());
        assert!(parse_expiry(Some("not-a-date")).is_err());
        assert!(parse_expiry(Some("2030-01-01")).is_err()); // date without time is not RFC3339
        assert!(parse_expiry(Some("2030-01-01T00:00:00")).is_err());
    }

    #[test]
    fn owner_id_reads_only_string_owners() {
        assert_eq!(owner_id(&doc! {"code": "a"}), None);
        assert_eq!(owner_id(&doc! {"code": "a", "owner": Bson::Null}), None);
        assert_eq!(owner_id(&doc! {"code": "a", "owner": "u1"}), Some("u1"));
    }

    #[test]
    fn may_write_permission_matrix() {
        let owner = User {
            id: "u1".into(),
            username: "owner".into(),
            display_name: None,
            email: None,
            avatar_url: None,
            hash_passwd: None,
            is_admin: false,
            promoted_by: None,
            token_version: 0,
            github_id: None,
            google_id: None,
            facebook_id: None,
        };
        let other = User {
            id: "u2".into(),
            ..owner.clone()
        };
        let admin = User {
            id: "u3".into(),
            is_admin: true,
            ..owner.clone()
        };
        let unowned = doc! {"code": "a"};
        let owned = doc! {"code": "a", "owner": "u1"};

        assert!(may_write(&unowned, None), "anonymous may write unowned");
        assert!(may_write(&unowned, Some(&other)));
        assert!(!may_write(&owned, None), "anonymous may not touch owned");
        assert!(may_write(&owned, Some(&owner)));
        assert!(!may_write(&owned, Some(&other)));
        assert!(may_write(&owned, Some(&admin)), "admins may edit anything");
    }

    #[test]
    fn owned_error_is_403_and_never_contains_url() {
        let err = owned_error("abc");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert!(err.message.contains("'abc'"));
        assert!(!err.message.contains("http"));
    }

    #[test]
    fn own_code_conflict_is_409_and_shows_the_url() {
        let err = own_code_conflict("abc", "https://example.com");
        assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
        assert!(err.message.contains("https://example.com"));
    }
}
