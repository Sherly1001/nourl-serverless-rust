//! The two ends of an OAuth sign-in: away to the provider, and back again.
//!
//! Both answer with a redirect whatever happens. The browser is mid-navigation
//! through a chain the provider started, and an error document rendered into
//! that navigation is a dead end — the login page is the only place with
//! anything useful to say.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::extract::OptionalUser;
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

/// Everything that can go wrong on the way in, as the code the login page
/// knows how to word. The provider's own message is never forwarded: it is not
/// ours to show, and it has been known to echo request parameters back.
fn refuse(jar: CookieJar, config: &crate::config::Config, code: &str) -> Response {
    // Only when there is one to clear. A browser that never started a flow has
    // no reason to be handed a `Set-Cookie` for a cookie it does not hold.
    let jar = match jar.get(state::COOKIE) {
        Some(_) => jar.add(state::cleared(config)),
        None => jar,
    };
    (jar, found(&format!("/#/login?error={code}"))).into_response()
}

/// The session, if the cookie names a live one.
///
/// A rejected session — revoked, expired, or naming an account that is gone —
/// reads as anonymous here rather than as a 401. Elsewhere that downgrade
/// would be wrong, but this flow can only ever reach the provider identity's
/// own account: without a session there is nothing to link *to*, so the worst
/// it can do is sign somebody in as themselves.
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
    // The flag rides in the signed cookie rather than the query string: it is
    // what decides between "sign me in" and "attach this identity to the
    // account I am holding", and a caller must not get to choose.
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
    // The access token lives for the rest of this function and is never
    // stored, never logged, and never sent anywhere but the provider.
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

/// Which account this profile belongs to, creating one if it belongs to
/// nobody. The rule itself is [`decide`]; this only does the lookups it needs
/// and the writes it asks for.
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
    // Only a verified address may be matched: an unverified one is a claim
    // anybody could make about somebody else's inbox.
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

/// The unique index on the provider field is the last word on who holds an
/// identity: two callbacks racing to claim the same one both pass [`decide`],
/// and the loser is told the same thing it would have been told a moment
/// later.
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

/// A brand-new account, under the first derived username nobody has taken.
///
/// Running out of candidates is a refusal rather than a panic: fifty names
/// derived from the same handle are all taken only if something is very wrong,
/// and the browser still has to be sent somewhere.
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

/// Re-read after the write, so the session is minted from what is actually
/// stored rather than from what we believe we stored.
async fn reload(app: &AppState, id: &str) -> Result<User, &'static str> {
    users::find_by_id(&app.db, id)
        .await
        .map_err(|_| "oauth_exchange")?
        .ok_or("oauth_exchange")
}

/// The same session cookie a password login issues, so revocation, expiry and
/// `token_version` all keep working unchanged.
fn sign_in(app: &AppState, jar: CookieJar, user: &User) -> Result<Response, AppError> {
    let token = jwt::encode(
        &app.config.jwt_secret,
        &user.id,
        user.token_version,
        app.config.session_days,
    )?;
    Ok((jar.add(cookie::session(&app.config, token)), found("/")).into_response())
}

fn method(settings: &settings::AuthSettings, kind: ProviderKind) -> MethodConfig {
    match kind {
        ProviderKind::Github => settings.github.clone(),
        ProviderKind::Google => settings.google.clone(),
        ProviderKind::Facebook => settings.facebook.clone(),
    }
}

/// Enabled is not enough: without both halves of the credential the provider
/// would answer the redirect with its own error page.
fn usable(cfg: &MethodConfig) -> bool {
    cfg.enabled
        && cfg.client_id.as_deref().is_some_and(|v| !v.is_empty())
        && cfg.client_secret.as_deref().is_some_and(|v| !v.is_empty())
}

/// Configured rather than taken from the request: the value has to match what
/// was registered at the provider, and a `Host` header is not ours to trust.
///
/// The fallback is production, not localhost. An unset `PUBLIC_BASE_URL` is a
/// misconfiguration either way, and the one that sends live users to a
/// callback on someone's laptop is the worse of the two.
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
