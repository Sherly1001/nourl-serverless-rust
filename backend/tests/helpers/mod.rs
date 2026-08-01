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
