use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use mongodb::Database;

use crate::config::Config;
use crate::error::AppError;
use crate::routes::redirect::{fallback_url, found, redirect};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub config: Config,
}

pub fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { found("/index.html") }))
        .route("/{code}", get(redirect))
        .fallback(fallback)
        .with_state(state)
}

async fn fallback(State(state): State<AppState>, uri: Uri, headers: HeaderMap) -> Response {
    if uri.path().starts_with("/api/") {
        AppError::not_found("unknown api route").into_response()
    } else {
        found(&fallback_url(&state.config, &headers))
    }
}
