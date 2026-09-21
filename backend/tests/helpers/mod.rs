#![allow(dead_code)] // each integration-test binary uses a subset of these helpers

use std::sync::Arc;

use backend::app::{AppState, build_app};
use backend::config::Config;
use backend::oauth::Providers;

pub const FALLBACK: &str = "https://fallback.example";
pub const JWT_SECRET: &str = "test-secret-not-used-in-production";
pub const SESSION_DAYS: i64 = 60;

/// Fresh throwaway database (`nourl_test_<uuid>`) on the local test mongo.
pub async fn test_db() -> mongodb::Database {
    let url = std::env::var("MONGO_URL").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into());
    let name = format!("nourl_test_{}", uuid::Uuid::new_v4().simple());
    backend::db::connect(&url, &name).await.unwrap()
}

/// The router with a chosen set of providers — the OAuth tests hand it stubs,
/// so nothing in the suite ever calls a real provider.
pub async fn test_app_with_providers(
    fallback: Option<&str>,
    providers: Providers,
) -> (axum::Router, mongodb::Database) {
    let db = test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();
    let config = Config {
        mongo_url: String::new(), // handlers never reconnect; only the pool in `db` is used
        db_name: db.name().to_string(),
        port: 0,
        notfound_fallback_url: fallback.map(String::from),
        public_base_url: Some("https://test.example".into()),
        jwt_secret: JWT_SECRET.into(),
        session_days: SESSION_DAYS,
        cookie_secure: false,
        // The production default, so tests assert on a real deployment's number.
        orphan_grace_days: 7,
    };
    (
        build_app(AppState {
            db: db.clone(),
            config,
            providers: Arc::new(providers),
        }),
        db,
    )
}

/// The router with the real providers. Nothing in the suite drives a flow
/// through them, so they are never actually called.
pub async fn test_app_with(fallback: Option<&str>) -> (axum::Router, mongodb::Database) {
    test_app_with_providers(fallback, Providers::production()).await
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

/// A session for a user inserted straight into the database, bypassing the
/// login flow — the only way to get one for a passwordless account until
/// phase 2b's OAuth callbacks exist.
pub fn cookie_for(user: &backend::users::User) -> String {
    let token =
        backend::auth::jwt::encode(JWT_SECRET, &user.id, user.token_version, SESSION_DAYS).unwrap();
    format!("nourl_session={token}")
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

/// An anonymous request with no body — GET, or a DELETE that carries none.
pub fn request(method: &str, uri: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .body(axum::body::Body::empty())
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
