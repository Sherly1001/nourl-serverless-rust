use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use mongodb::Database;

use crate::access_log::access_log;
use crate::config::Config;
use crate::error::AppError;
use crate::oauth::Providers;
use crate::routes::admin::{
    delete_user, get_settings, list_users, set_user_admin, update_settings,
};
use crate::routes::auth::{
    change_password, delete_me, login, logout, me, methods, register, update_me,
};
use crate::routes::oauth::{callback, disconnect, start};
use crate::routes::redirect::{fallback_url, found, redirect};
use crate::routes::urls::{create_url, delete_url, list_urls, update_url};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub config: Config,
    /// One client per provider, so a handler goes straight from a path segment
    /// to the thing that talks to it — and the tests can swap in a stub.
    pub providers: Arc<Providers>,
}

pub fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me).put(update_me).delete(delete_me))
        .route("/api/auth/methods", get(methods))
        .route("/api/auth/password", put(change_password))
        // After the fixed segments above. Axum prefers a literal over a
        // capture whatever the order, but the order says so out loud.
        .route("/api/auth/{provider}", get(start).delete(disconnect))
        .route("/api/auth/{provider}/callback", get(callback))
        .route("/api/admin/users", get(list_users))
        .route(
            "/api/admin/settings",
            get(get_settings).put(update_settings),
        )
        .route(
            "/api/admin/users/{id}",
            put(set_user_admin).delete(delete_user),
        )
        .route("/api/urls", post(create_url).get(list_urls))
        .route("/api/urls/{code}", put(update_url).delete(delete_url))
        .route("/", get(|| async { found("/index.html") }))
        .route("/{code}", get(redirect))
        .fallback(fallback)
        .method_not_allowed_fallback(|| async {
            AppError {
                status: axum::http::StatusCode::METHOD_NOT_ALLOWED,
                code: "method_not_allowed",
                message: "method not allowed for this route".into(),
                field: None,
                conflict: None,
            }
        })
        .layer(axum::middleware::from_fn(access_log))
        .with_state(state)
}

async fn fallback(State(state): State<AppState>, uri: Uri, headers: HeaderMap) -> Response {
    if uri.path().starts_with("/api/") {
        AppError::not_found("unknown api route").into_response()
    } else {
        found(&fallback_url(&state.config, &headers))
    }
}
