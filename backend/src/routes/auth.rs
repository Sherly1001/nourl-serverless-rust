use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use shared::{
    AuthMethods, ChangePasswordRequest, DeleteAccountRequest, DeleteUserResponse, LoginRequest,
    RegisterRequest, UpdateProfileRequest, UserInfo, validate_avatar_url, validate_email,
    validate_password, validate_username,
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
    validate_username(&body.username).map_err(|e| AppError::validation(e).on_field("username"))?;
    validate_password(&body.password).map_err(|e| AppError::validation(e).on_field("password"))?;

    // Friendlier than the index, which is still what settles a race.
    if users::find_by_username(&state.db, &body.username)
        .await?
        .is_some()
    {
        return Err(AppError::conflict("that username is already taken").on_field("username"));
    }

    // `users::create` stores whatever it is handed, verbatim.
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

    // One message for both, so this cannot enumerate accounts.
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

/// `username` must clear registration's bar. A rename leaves `token_version`
/// alone: sessions key on the id, so they stay valid.
pub async fn update_me(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    AppJson(mut body): AppJson<UpdateProfileRequest>,
) -> Result<Json<UserInfo>, AppError> {
    match body.username.as_deref() {
        // Re-sending the current username is a no-op, not a self-collision.
        Some(name) if name == user.username => body.username = None,
        Some(name) => {
            validate_username(name).map_err(|e| AppError::validation(e).on_field("username"))?;
            if users::find_by_username(&state.db, name).await?.is_some() {
                return Err(
                    AppError::conflict("that username is already taken").on_field("username")
                );
            }
        }
        None => {}
    }
    // Both are echoed to other people, so neither is taken on trust.
    if let Some(email) = body.email.as_deref().filter(|value| !value.is_empty()) {
        validate_email(email).map_err(|e| AppError::validation(e).on_field("email"))?;
    }
    if let Some(avatar) = body.avatar_url.as_deref().filter(|value| !value.is_empty()) {
        validate_avatar_url(avatar).map_err(|e| AppError::validation(e).on_field("avatar_url"))?;
    }

    users::update_profile(&state.db, &user.id, body).await?;
    let reloaded = users::find_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished mid-update"))?;
    Ok(Json(reloaded.to_info()))
}

/// Clears the cookie and bumps `token_version`: with a 60-day expiry, clearing
/// the cookie alone would revoke nothing.
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    users::bump_token_version(&state.db, &user.id).await?;
    let jar = jar.add(cookie::cleared(&state.config));
    Ok((jar, Json(serde_json::json!({"logged_out": true}))).into_response())
}

/// Rotates the password, revoking outstanding tokens, then re-authenticates
/// this device so only the others are logged out. A provider-only account sets
/// its first password here with no `current_password` to prove.
pub async fn change_password(
    State(state): State<AppState>,
    jar: CookieJar,
    CurrentUser(user): CurrentUser,
    AppJson(body): AppJson<ChangePasswordRequest>,
) -> Result<Response, AppError> {
    if let Some(stored) = user.hash_passwd.as_deref() {
        let current = body.current_password.as_deref().ok_or_else(|| {
            AppError::unauthorized("current password is required").on_field("current_password")
        })?;
        if !password::verify(current, stored) {
            return Err(AppError::unauthorized("current password is incorrect")
                .on_field("current_password"));
        }
    }
    validate_password(&body.new_password)
        .map_err(|e| AppError::validation(e).on_field("new_password"))?;

    let hash = password::hash(&body.new_password)?;
    users::set_password(&state.db, &user.id, &hash).await?;

    let reloaded = users::find_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished mid-update"))?;
    logged_in(&state, jar, &reloaded)
}

/// Public: the login page renders from it before anyone is authenticated.
pub async fn methods(State(state): State<AppState>) -> Result<Json<AuthMethods>, AppError> {
    Ok(Json(settings::load(&state.db).await?.methods()))
}

/// The one place [`LinkDisposition::Delete`] is allowed: only the author may
/// decide nobody should follow their links again. Refused while the account
/// holds the admin flag — resign first, so the cascade is nobody's surprise.
pub async fn delete_me(
    State(state): State<AppState>,
    jar: CookieJar,
    CurrentUser(user): CurrentUser,
    AppJson(body): AppJson<DeleteAccountRequest>,
) -> Result<Response, AppError> {
    // Before the admin check: someone who cannot prove who they are learns
    // nothing about the account's standing.
    if let Some(stored) = user.hash_passwd.as_deref() {
        let current = body.current_password.as_deref().ok_or_else(|| {
            AppError::unauthorized("current password is required").on_field("current_password")
        })?;
        if !password::verify(current, stored) {
            return Err(AppError::unauthorized("current password is incorrect")
                .on_field("current_password"));
        }
    }
    if user.is_admin {
        // Named differently for a root, because "resign first" is advice they
        // cannot act on: only the database can take a root's flag away.
        return Err(AppError::forbidden(if user.promoted_by.is_some() {
            "you are an admin so cannot close your own account — give up the admin flag first"
        } else {
            "you are the top admin so cannot close your own account"
        }));
    }

    let mut session = state.db.client().start_session().await?;
    let links = users::delete_with_cascade(
        &state.db,
        &mut session,
        &user.id,
        body.links,
        state.config.orphan_grace_days,
    )
    .await?;
    // The account is gone, so the cookie has nothing left to authenticate.
    let jar = jar.add(cookie::cleared(&state.config));
    Ok((
        jar,
        Json(DeleteUserResponse {
            id: user.id,
            deleted: true,
            orphaned: links.orphaned,
            links_deleted: links.deleted,
            grace_days: state.config.orphan_grace_days,
            demoted: 0,
            reparented: 0,
        }),
    )
        .into_response())
}
