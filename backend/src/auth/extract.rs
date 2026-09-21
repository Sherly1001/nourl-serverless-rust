use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;

use crate::app::AppState;
use crate::auth::{cookie, jwt};
use crate::error::AppError;
use crate::users::{self, User};

/// Reloads the user every request, so `token_version` can revoke a token that
/// has not expired — the price of a long-lived session.
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

/// `None` when no cookie was sent. A present-but-invalid one is still an
/// error: downgrading to anonymous would let a revoked user keep writing.
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

/// A `CurrentUser` with the admin flag. The type is the permission, so a new
/// admin route cannot forget the check.
pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err(AppError::forbidden("this account is not an admin"));
        }
        Ok(AdminUser(user))
    }
}

/// An [`AdminUser`] nobody promoted. Reserved for deployment configuration
/// rather than administration: promoted admins manage accounts, the root
/// decides how accounts get in at all.
pub struct RootAdmin(pub User);

impl FromRequestParts<AppState> for RootAdmin {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AdminUser(user) = AdminUser::from_request_parts(parts, state).await?;
        if user.promoted_by.is_some() {
            return Err(AppError::forbidden(
                "only the top admin can see or change the sign-in settings",
            ));
        }
        Ok(RootAdmin(user))
    }
}
