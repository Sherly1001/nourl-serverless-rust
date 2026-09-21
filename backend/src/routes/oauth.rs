//! Both ends of an OAuth sign-in, always answering with a redirect: the
//! browser is mid-navigation, where an error document is a dead end.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use shared::UserInfo;

use crate::app::AppState;
use crate::auth::extract::{CurrentUser, OptionalUser};
use crate::auth::{cookie, jwt};
use crate::error::AppError;
use crate::oauth::account::{Decision, decide, username_candidates};
use crate::oauth::{Profile, ProviderKind, state};
use crate::routes::redirect::found;
use crate::settings::{self, MethodConfig};
use crate::users::{self, NewUser, User};

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// The provider's own refusal — `access_denied` when the user cancels.
    #[serde(default)]
    pub error: Option<String>,
}

/// Failures as codes the login page words itself. A provider's own message is
/// never forwarded — it has been known to echo request parameters back.
fn refuse(jar: CookieJar, config: &crate::config::Config, code: &str) -> Response {
    // No `Set-Cookie` for a browser that never started a flow.
    let jar = match jar.get(state::COOKIE) {
        Some(_) => jar.add(state::cleared(config)),
        None => jar,
    };
    (jar, found(&format!("/#/login?error={code}"))).into_response()
}

/// A rejected session reads as anonymous rather than 401. Safe only here:
/// without one there is nothing to link to, so the worst this can do is sign
/// somebody in as themselves.
fn session_of(session: Result<OptionalUser, AppError>) -> Option<User> {
    session.ok().and_then(|OptionalUser(user)| user)
}

/// `GET /api/auth/{provider}` — 302 to the provider, or back to the login page.
pub async fn start(
    State(app): State<AppState>,
    jar: CookieJar,
    session: Result<OptionalUser, AppError>,
    Path(provider): Path<String>,
) -> Response {
    let Some(kind) = ProviderKind::parse(&provider) else {
        return refuse(jar, &app.config, "oauth_disabled");
    };
    let Ok(settings) = settings::load(&app.db).await else {
        return refuse(jar, &app.config, "oauth_disabled");
    };
    let cfg = method(&settings, kind);
    if !usable(&cfg) {
        return refuse(jar, &app.config, "oauth_disabled");
    }
    // In the signed cookie, not the query: a caller must not choose between
    // signing in and attaching an identity to the account they hold.
    let link = session_of(session).is_some();
    let Ok((nonce, cookie)) = state::issue(&app.config, link) else {
        return refuse(jar, &app.config, "oauth_disabled");
    };
    let url = app
        .providers
        .get(kind)
        .authorize_url(&cfg, &redirect_uri(&app, kind), &nonce);
    (jar.add(cookie), found(&url)).into_response()
}

/// `GET /api/auth/{provider}/callback` — the only place an account is created
/// or linked.
pub async fn callback(
    State(app): State<AppState>,
    jar: CookieJar,
    session: Result<OptionalUser, AppError>,
    Path(provider): Path<String>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(kind) = ProviderKind::parse(&provider) else {
        return refuse(jar, &app.config, "oauth_disabled");
    };
    if query.error.is_some() {
        return refuse(jar, &app.config, "oauth_denied");
    }
    let cookie_value = jar
        .get(state::COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();
    let Some(flow) = state::verify(
        &app.config,
        &cookie_value,
        query.state.as_deref().unwrap_or_default(),
    ) else {
        return refuse(jar, &app.config, "oauth_state");
    };
    let (Some(code), Ok(settings)) = (query.code.as_deref(), settings::load(&app.db).await) else {
        return refuse(jar, &app.config, "oauth_exchange");
    };
    let cfg = method(&settings, kind);
    if !usable(&cfg) {
        return refuse(jar, &app.config, "oauth_disabled");
    }

    let client = app.providers.get(kind);
    let redirect_uri = redirect_uri(&app, kind);
    // Never stored, logged, or sent anywhere but the provider.
    let Ok(token) = client.exchange(&cfg, &redirect_uri, code).await else {
        return refuse(jar, &app.config, "oauth_exchange");
    };
    let Ok(profile) = client.profile(&token).await else {
        return refuse(jar, &app.config, "oauth_exchange");
    };
    if profile.id.is_empty() {
        return refuse(jar, &app.config, "oauth_exchange");
    }

    let session = session_of(session);
    match resolve(&app, kind, &profile, session.as_ref(), flow.link).await {
        // The state is spent either way, so it is cleared on the way out.
        Ok(user) => match sign_in(&app, jar.clone().add(state::cleared(&app.config)), &user) {
            Ok(response) => response,
            Err(_) => refuse(jar, &app.config, "oauth_exchange"),
        },
        Err(code) => refuse(jar, &app.config, code),
    }
}

/// Which account this profile belongs to, creating one if none. [`decide`]
/// holds the rule; this only does the lookups and writes it asks for.
async fn resolve(
    app: &AppState,
    kind: ProviderKind,
    profile: &Profile,
    session: Option<&User>,
    link: bool,
) -> Result<User, &'static str> {
    let existing = users::find_by_provider(&app.db, kind, &profile.id)
        .await
        .map_err(|_| "oauth_exchange")?;
    // Unverified is a claim anybody could make about somebody else's inbox.
    let by_email = match (profile.email_verified, profile.email.as_deref()) {
        (true, Some(email)) => users::find_by_email(&app.db, email)
            .await
            .map_err(|_| "oauth_exchange")?,
        _ => None,
    };

    match decide(
        existing.as_ref().map(|u| u.id.as_str()),
        session.map(|u| u.id.as_str()),
        link,
        by_email.as_ref().map(|u| u.id.as_str()),
    ) {
        Decision::Taken => Err("oauth_taken"),
        Decision::SignIn(id) => reload(app, &id).await,
        Decision::AttachTo(id) => {
            link_identity(app, &id, kind, &profile.id).await?;
            reload(app, &id).await
        }
        Decision::Create => {
            let user = create(app, kind, profile).await?;
            link_identity(app, &user.id, kind, &profile.id).await?;
            reload(app, &user.id).await
        }
    }
}

/// The unique index has the last word: two callbacks racing both pass
/// [`decide`], and the loser hears what it would have heard a moment later.
async fn link_identity(
    app: &AppState,
    id: &str,
    kind: ProviderKind,
    provider_id: &str,
) -> Result<(), &'static str> {
    users::link_provider(&app.db, id, kind, provider_id)
        .await
        .map_err(|err| {
            if err.status == axum::http::StatusCode::CONFLICT {
                "oauth_taken"
            } else {
                "oauth_exchange"
            }
        })
}

/// A new account under the first free derived username. Running out is a
/// refusal, not a panic — the browser still has to be sent somewhere.
async fn create(
    app: &AppState,
    kind: ProviderKind,
    profile: &Profile,
) -> Result<User, &'static str> {
    for candidate in username_candidates(profile, kind) {
        if users::find_by_username(&app.db, &candidate)
            .await
            .map_err(|_| "oauth_exchange")?
            .is_none()
        {
            return users::create(&app.db, NewUser::from_profile(candidate, profile))
                .await
                .map_err(|_| "oauth_exchange");
        }
    }
    Err("oauth_exchange")
}

/// Re-read after the write, so the session is minted from what is stored.
async fn reload(app: &AppState, id: &str) -> Result<User, &'static str> {
    users::find_by_id(&app.db, id)
        .await
        .map_err(|_| "oauth_exchange")?
        .ok_or("oauth_exchange")
}

/// The same cookie a password login issues, so revocation still works.
fn sign_in(app: &AppState, jar: CookieJar, user: &User) -> Result<Response, AppError> {
    let token = jwt::encode(
        &app.config.jwt_secret,
        &user.id,
        user.token_version,
        app.config.session_days,
    )?;
    // `/#/`, not `/`: Facebook's `#_=_` fragment survives and routes to 404.
    Ok((jar.add(cookie::session(&app.config, token)), found("/#/")).into_response())
}

fn method(settings: &settings::AuthSettings, kind: ProviderKind) -> MethodConfig {
    match kind {
        ProviderKind::Github => settings.github.clone(),
        ProviderKind::Google => settings.google.clone(),
        ProviderKind::Facebook => settings.facebook.clone(),
    }
}

/// Enabled is not enough: without credentials the provider shows its error.
fn usable(cfg: &MethodConfig) -> bool {
    cfg.enabled
        && cfg.client_id.as_deref().is_some_and(|v| !v.is_empty())
        && cfg.client_secret.as_deref().is_some_and(|v| !v.is_empty())
}

/// Configured, not taken from `Host`, which is not ours to trust. The fallback
/// is production: an unset value that sends live users to a laptop is worse.
fn redirect_uri(app: &AppState, kind: ProviderKind) -> String {
    let base = app
        .config
        .public_base_url
        .clone()
        .unwrap_or_else(|| "https://nourl.space".into());
    format!(
        "{}/api/auth/{}/callback",
        base.trim_end_matches('/'),
        kind.as_str()
    )
}

/// Takes an identity off the caller's account, refused when it is the last way
/// in — the account would outlive anyone's ability to reach it. JSON, not a
/// redirect: this one is called by `fetch`.
pub async fn disconnect(
    State(app): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(provider): Path<String>,
) -> Result<Json<UserInfo>, AppError> {
    let kind =
        ProviderKind::parse(&provider).ok_or_else(|| AppError::not_found("no such provider"))?;
    let connected = user.providers();
    if !connected.iter().any(|p| p == kind.as_str()) {
        return Err(AppError::not_found("that provider is not connected"));
    }
    // Whether anything would be left to sign in with afterwards.
    if user.hash_passwd.is_none() && connected.len() == 1 {
        return Err(AppError {
            status: axum::http::StatusCode::BAD_REQUEST,
            code: "last_login_method",
            message: "that is the only way into this account — set a password first".into(),
            field: None,
            conflict: None,
            rejected: None,
        });
    }
    users::unlink_provider(&app.db, &user.id, kind).await?;
    let reloaded = users::find_by_id(&app.db, &user.id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished mid-update"))?;
    Ok(Json(reloaded.to_info()))
}
