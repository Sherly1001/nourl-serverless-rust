#![allow(dead_code)] // each integration-test binary uses a subset of these helpers

use backend::app::{AppState, build_app};
use backend::config::Config;

pub const FALLBACK: &str = "https://fallback.example";

/// Fresh throwaway database (`nourl_test_<uuid>`) on the local test mongo.
pub async fn test_db() -> mongodb::Database {
    let url = std::env::var("MONGO_URL").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into());
    let name = format!("nourl_test_{}", uuid::Uuid::new_v4().simple());
    backend::db::connect(&url, &name).await.unwrap()
}

pub async fn test_app_with(fallback: Option<&str>) -> (axum::Router, mongodb::Database) {
    let db = test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();
    let config = Config {
        mongo_url: String::new(), // handlers never reconnect; only the pool in `db` is used
        db_name: db.name().to_string(),
        port: 0,
        notfound_fallback_url: fallback.map(String::from),
        jwt_secret: "test-secret-not-used-in-production".into(),
        session_days: 60,
        cookie_secure: false,
    };
    (
        build_app(AppState {
            db: db.clone(),
            config,
        }),
        db,
    )
}

pub async fn test_app() -> (axum::Router, mongodb::Database) {
    test_app_with(Some(FALLBACK)).await
}

/// The `Set-Cookie` value for the session, trimmed to `name=value` so it can be
/// sent straight back as a `Cookie` header.
pub fn session_cookie(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("nourl_session="))
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
}

pub fn json_request(
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

/// Same, with a session attached.
pub fn authed_request(
    method: &str,
    uri: &str,
    cookie: &str,
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

/// A GET carrying a session cookie.
pub fn authed_get(uri: &str, cookie: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .body(axum::body::Body::empty())
        .unwrap()
}

pub async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 64)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}
