use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use shared::{LoginRequest, RegisterRequest, validate_password, validate_username};

use crate::app::AppState;
use crate::auth::{cookie, jwt, password};
use crate::error::AppError;
use crate::extract::AppJson;
use crate::settings;
use crate::users::{self, NewUser};

/// Mints a session for `user` and attaches it to the response.
fn logged_in(state: &AppState, jar: CookieJar, user: &users::User) -> Result<Response, AppError> {
    let token = jwt::encode(
        &state.config.jwt_secret,
        &user.id,
        user.token_version,
        state.config.session_days,
    )?;
    let jar = jar.add(cookie::session(&state.config, token));
    Ok((jar, Json(user.to_info())).into_response())
}

pub async fn register(
    State(state): State<AppState>,
    jar: CookieJar,
    AppJson(body): AppJson<RegisterRequest>,
) -> Result<Response, AppError> {
    if !settings::load(&state.db).await?.password.enabled {
        return Err(AppError::forbidden("password registration is disabled"));
    }
    validate_username(&body.username).map_err(AppError::validation)?;
    validate_password(&body.password).map_err(AppError::validation)?;

    // Friendlier than waiting for the unique index to reject it; the index is
    // still what actually prevents a race between two simultaneous signups.
    if users::find_by_username(&state.db, &body.username)
        .await?
        .is_some()
    {
        return Err(AppError::conflict("that username is already taken"));
    }

    // password::hash is the only thing that hashes; users::create stores
    // whatever it is handed, verbatim.
    let hash = password::hash(&body.password)?;
    let user = users::create(&state.db, NewUser::with_password_hash(&body.username, hash)).await?;
    logged_in(&state, jar, &user)
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    AppJson(body): AppJson<LoginRequest>,
) -> Result<Response, AppError> {
    if !settings::load(&state.db).await?.password.enabled {
        return Err(AppError::forbidden("password login is disabled"));
    }

    // One message for both "no such user" and "wrong password", so the
    // endpoint cannot be used to enumerate accounts.
    let invalid = || AppError::unauthorized("incorrect username or password");
    let user = users::find_by_username(&state.db, &body.username)
        .await?
        .ok_or_else(invalid)?;
    let stored = user.hash_passwd.as_deref().ok_or_else(invalid)?;
    if !password::verify(&body.password, stored) {
        return Err(invalid());
    }

    logged_in(&state, jar, &user)
}
