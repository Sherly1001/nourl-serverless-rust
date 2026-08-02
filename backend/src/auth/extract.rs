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

/// A `CurrentUser` that also carries the admin flag.
///
/// Rejecting here rather than inside each handler means a new admin route
/// cannot forget the check: the type is the permission.
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

/// An [`AdminUser`] who is also the root of the chain — the admin nobody
/// promoted, seeded directly in the database.
///
/// Reserved for what is deployment configuration rather than day-to-day
/// administration: the sign-in settings decide how *everyone* authenticates,
/// including whether password login exists at all, so turning them off is a way
/// to lock the system rather than to run it. Promoted admins manage accounts;
/// the root decides how accounts get in.
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
