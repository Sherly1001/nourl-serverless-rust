use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use mongodb::bson::{Document, doc};

use crate::app::AppState;
use crate::config::Config;

pub fn found(location: &str) -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response()
}

/// `NOTFOUND_FALLBACK_URL`, else the request's own host — `x-forwarded-host`
/// (the viewer's) before `host` (rewritten by API Gateway).
pub fn fallback_url(config: &Config, headers: &HeaderMap) -> String {
    if let Some(url) = &config.notfound_fallback_url {
        return url.clone();
    }
    ["x-forwarded-host", "host"]
        .iter()
        .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()))
        .map(|host| format!("https://{host}"))
        .unwrap_or_else(|| "https://nourl.space".into())
}

pub async fn redirect(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let fallback = fallback_url(&state.config, &headers);
    let now = bson::DateTime::now();
    let visited = state
        .db
        .collection::<Document>("urls")
        .find_one_and_update(
            doc! {
                "code": &code,
                "$or": [{"expires_at": null}, {"expires_at": {"$gt": now}}],
            },
            doc! {"$inc": {"hits": 1}, "$set": {"last_hit_at": now}},
        )
        .await;
    match visited {
        Ok(Some(doc)) => match doc.get_str("url") {
            Ok(url) => found(url),
            Err(_) => found(&fallback),
        },
        _ => found(&fallback),
    }
}
