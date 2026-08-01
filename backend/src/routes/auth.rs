use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use shared::{
    AuthMethods, ChangePasswordRequest, LoginRequest, RegisterRequest, UpdateProfileRequest,
    UserInfo, validate_password, validate_username,
};

use crate::app::AppState;
use crate::auth::extract::CurrentUser;
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

pub async fn me(CurrentUser(user): CurrentUser) -> Json<UserInfo> {
    Json(user.to_info())
}

/// `username` is deliberately not editable here: it is the login handle and
/// carries a unique index, so changing it belongs in its own flow.
pub async fn update_me(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    AppJson(body): AppJson<UpdateProfileRequest>,
) -> Result<Json<UserInfo>, AppError> {
    users::update_profile(
        &state.db,
        &user.id,
        body.display_name,
        body.email,
        body.avatar_url,
    )
    .await?;
    let reloaded = users::find_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished mid-update"))?;
    Ok(Json(reloaded.to_info()))
}

/// Clears the cookie *and* bumps `token_version`, so tokens already handed out
/// — on other devices, or copied out of a browser — stop working too. With a
/// 60-day expiry, clearing the cookie alone would revoke nothing.
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    users::bump_token_version(&state.db, &user.id).await?;
    let jar = jar.add(cookie::cleared(&state.config));
    Ok((jar, Json(serde_json::json!({"logged_out": true}))).into_response())
}

/// Rotating the password revokes every outstanding token, then immediately
/// re-authenticates the caller so the device doing the change stays signed in
/// while other devices are logged out.
pub async fn change_password(
    State(state): State<AppState>,
    jar: CookieJar,
    CurrentUser(user): CurrentUser,
    AppJson(body): AppJson<ChangePasswordRequest>,
) -> Result<Response, AppError> {
    let stored = user
        .hash_passwd
        .as_deref()
        .ok_or_else(|| AppError::forbidden("this account has no password to change"))?;
    if !password::verify(&body.current_password, stored) {
        return Err(AppError::unauthorized("current password is incorrect"));
    }
    validate_password(&body.new_password).map_err(AppError::validation)?;

    let hash = password::hash(&body.new_password)?;
    users::set_password(&state.db, &user.id, &hash).await?;
    users::bump_token_version(&state.db, &user.id).await?;

    let reloaded = users::find_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished mid-update"))?;
    logged_in(&state, jar, &reloaded)
}

/// Public: the login page needs to know which buttons to render before anyone
/// is authenticated.
pub async fn methods(State(state): State<AppState>) -> Result<Json<AuthMethods>, AppError> {
    Ok(Json(settings::load(&state.db).await?.methods()))
}
