use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;

use crate::app::AppState;
use crate::auth::{cookie, jwt};
use crate::error::AppError;
use crate::users::{self, User};

/// Resolves the session cookie to a live user, or explains why not.
///
/// The user is reloaded on every request so `token_version` can revoke tokens
/// that have not expired — the price of a long-lived session.
async fn current_user(state: &AppState, parts: &Parts) -> Result<Option<User>, AppError> {
    let jar = CookieJar::from_headers(&parts.headers);
    let Some(raw) = jar.get(cookie::NAME).map(|c| c.value().to_string()) else {
        return Ok(None);
    };
    let claims = jwt::decode(&state.config.jwt_secret, &raw)?;
    let Some(user) = users::find_by_id(&state.db, &claims.sub).await? else {
        return Err(AppError::unauthorized("account no longer exists"));
    };
    if user.token_version != claims.ver {
        return Err(AppError::unauthorized("session has been revoked"));
    }
    Ok(Some(user))
}

/// Rejects with 401 unless a valid session is present.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        current_user(state, parts)
            .await?
            .map(CurrentUser)
            .ok_or_else(|| AppError::unauthorized("authentication required"))
    }
}

/// `None` when no cookie was sent. A cookie that is present but invalid is
/// still an error — silently downgrading a rejected session to anonymous
/// would let a revoked user keep writing to unowned links.
pub struct OptionalUser(pub Option<User>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        current_user(state, parts).await.map(OptionalUser)
    }
}
