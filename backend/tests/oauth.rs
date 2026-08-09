mod helpers;

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use backend::oauth::{Profile, Provider, ProviderKind, Providers};
use backend::settings::MethodConfig;
use helpers::*;
use mongodb::bson::{Document, doc};
use serde_json::json;
use tower::ServiceExt;

/// A provider that answers from memory. The real ones are exercised by their
/// own unit tests; what matters here is the flow around them.
struct Stub {
    kind: ProviderKind,
    profile: Profile,
}

#[async_trait]
impl Provider for Stub {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn authorize_url(&self, _cfg: &MethodConfig, redirect_uri: &str, state: &str) -> String {
        format!("https://provider.example/auth?redirect_uri={redirect_uri}&state={state}")
    }

    async fn exchange(
        &self,
        _cfg: &MethodConfig,
        _redirect_uri: &str,
        code: &str,
    ) -> Result<String, backend::error::AppError> {
        if code == "bad-code" {
            return Err(backend::error::AppError::validation("nope"));
        }
        Ok("access-token".into())
    }

    async fn profile(&self, _token: &str) -> Result<Profile, backend::error::AppError> {
        Ok(self.profile.clone())
    }
}

fn stubbed(profile: Profile) -> Providers {
    Providers::from_parts(
        Arc::new(Stub {
            kind: ProviderKind::Github,
            profile: profile.clone(),
        }),
        Arc::new(Stub {
            kind: ProviderKind::Google,
            profile: profile.clone(),
        }),
        Arc::new(Stub {
            kind: ProviderKind::Facebook,
            profile,
        }),
    )
}

fn verified(id: &str, username: &str, email: &str) -> Profile {
    Profile {
        id: id.into(),
        username: Some(username.into()),
        display_name: Some(username.into()),
        email: Some(email.into()),
        email_verified: true,
        avatar_url: None,
    }
}

/// Turns a provider on with credentials, the way the settings page would.
async fn enable(db: &mongodb::Database, provider: &str) {
    db.collection::<Document>("settings")
        .update_one(
            doc! {"_id": "auth"},
            doc! {"$set": {
                provider: {"enabled": true, "client_id": "id", "client_secret": "secret"},
            }},
        )
        .upsert(true)
        .await
        .unwrap();
}

fn location(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The whole `Set-Cookie` line for the state cookie, attributes included —
/// clearing it is a `Set-Cookie` too, so the tests need to tell the two apart.
fn state_set_cookie(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("oauth_state="))
        .map(String::from)
}

/// Trimmed to `name=value`, ready to send straight back as a `Cookie` header.
fn state_cookie(response: &axum::response::Response) -> Option<String> {
    state_set_cookie(response).map(|v| v.split(';').next().unwrap_or(&v).to_string())
}

/// The nonce the browser would hand back, read out of the cookie the start
/// route set.
fn nonce_of(cookie: &str) -> String {
    let token = cookie.trim_start_matches("oauth_state=");
    let payload = token.split('.').nth(1).expect("a jwt has three parts");
    let value: serde_json::Value = serde_json::from_slice(&base64_decode(payload)).unwrap();
    value["nonce"].as_str().unwrap().to_string()
}

fn base64_decode(input: &str) -> Vec<u8> {
    // base64url, no padding — what jsonwebtoken emits.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for ch in input.bytes() {
        let Some(index) = ALPHABET.iter().position(|c| *c == ch) else {
            continue;
        };
        buffer = (buffer << 6) | index as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

/// Drives a whole flow: start, then hand the callback the cookie it set.
/// `session` is any other cookies the browser would be carrying.
async fn flow(
    app: &axum::Router,
    provider: &str,
    query: &str,
    session: Option<&str>,
) -> axum::response::Response {
    let started = match session {
        Some(session) => app
            .clone()
            .oneshot(authed_get(&format!("/api/auth/{provider}"), session))
            .await
            .unwrap(),
        None => app
            .clone()
            .oneshot(request("GET", &format!("/api/auth/{provider}")))
            .await
            .unwrap(),
    };
    let state = state_cookie(&started).expect("the flow has to be remembered somewhere");
    let nonce = nonce_of(&state);
    let cookies = match session {
        Some(session) => format!("{session}; {state}"),
        None => state.clone(),
    };
    app.clone()
        .oneshot(authed_get(
            &format!("/api/auth/{provider}/callback?state={nonce}&{query}"),
            &cookies,
        ))
        .await
        .unwrap()
}

async fn provider_id(db: &mongodb::Database, username: &str, field: &str) -> Option<String> {
    db.collection::<Document>("users")
        .find_one(doc! {"username": username})
        .await
        .unwrap()
        .and_then(|doc| doc.get_str(field).ok().map(String::from))
}

#[tokio::test]
async fn starting_a_flow_sets_a_state_cookie_and_redirects_to_the_provider() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(verified("gh-1", "octocat", "octo@example.com")),
    )
    .await;
    enable(&db, "github").await;

    let response = app
        .oneshot(request("GET", "/api/auth/github"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
    let cookie = state_cookie(&response).expect("the flow has to be remembered somewhere");
    let target = location(&response);
    assert!(target.starts_with("https://provider.example/auth"));
    assert!(
        target.contains("redirect_uri=https://test.example/api/auth/github/callback"),
        "the redirect uri comes from config, not from the request host: {target}"
    );
    assert!(
        target.contains(&nonce_of(&cookie)),
        "state must be the cookie's nonce"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_provider_that_is_off_never_starts_a_flow() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(verified("gh-1", "octocat", "octo@example.com")),
    )
    .await;
    // Settings untouched: every provider is disabled by default.

    let response = app
        .clone()
        .oneshot(request("GET", "/api/auth/github"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(location(&response), "/#/login?error=oauth_disabled");
    assert!(state_set_cookie(&response).is_none(), "nothing to remember");

    // A path segment that is not a provider at all takes the same exit, and
    // never reaches a database lookup built from it.
    let nonsense = app
        .oneshot(request("GET", "/api/auth/hash_passwd"))
        .await
        .unwrap();
    assert_eq!(location(&nonsense), "/#/login?error=oauth_disabled");

    db.drop().await.unwrap();
}

#[tokio::test]
async fn the_callback_creates_an_account_then_signs_the_same_one_in() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(verified("gh-1", "octocat", "octo@example.com")),
    )
    .await;
    enable(&db, "github").await;

    let landed = flow(&app, "github", "code=ok", None).await;
    assert_eq!(landed.status(), StatusCode::FOUND);
    // Carrying a fragment of its own, so Facebook's `#_=_` on the callback URL
    // is replaced rather than left over the site root as an unknown route.
    assert_eq!(location(&landed), "/#/");
    let session = session_cookie(&landed).expect("a callback signs you in");
    // Single use: the state is spent, so a replayed callback has nothing to
    // match against.
    assert!(
        state_set_cookie(&landed).is_some_and(|c| c.contains("Max-Age=0")),
        "a spent state must be cleared"
    );

    let me = app
        .clone()
        .oneshot(authed_get("/api/auth/me", &session))
        .await
        .unwrap();
    let body = body_json(me).await;
    assert_eq!(body["username"], "octocat");
    assert_eq!(body["email"], "octo@example.com");
    assert_eq!(body["is_admin"], false, "a provider can never grant admin");
    let first_id = body["id"].as_str().unwrap().to_string();

    // Second time through: the same identity must land on the same account.
    let again = flow(&app, "github", "code=ok", None).await;
    let session = session_cookie(&again).unwrap();
    let me = app
        .oneshot(authed_get("/api/auth/me", &session))
        .await
        .unwrap();
    assert_eq!(
        body_json(me).await["id"],
        first_id,
        "a second account was created"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_callback_without_a_matching_state_fails_closed() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(verified("gh-1", "octocat", "octo@example.com")),
    )
    .await;
    enable(&db, "github").await;

    let started = app
        .clone()
        .oneshot(request("GET", "/api/auth/github"))
        .await
        .unwrap();
    let cookie = state_cookie(&started).unwrap();
    let nonce = nonce_of(&cookie);

    // No cookie at all — which is also what a replayed callback looks like,
    // since a successful one clears it.
    let bare = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/auth/github/callback?code=ok&state={nonce}"),
        ))
        .await
        .unwrap();
    assert_eq!(location(&bare), "/#/login?error=oauth_state");
    assert!(session_cookie(&bare).is_none());

    // Cookie present, parameter wrong.
    let mismatched = app
        .clone()
        .oneshot(authed_get(
            "/api/auth/github/callback?code=ok&state=someone-elses-nonce",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(location(&mismatched), "/#/login?error=oauth_state");
    assert!(session_cookie(&mismatched).is_none());

    // The user pressed cancel at the provider.
    let denied = app
        .clone()
        .oneshot(authed_get(
            &format!("/api/auth/github/callback?error=access_denied&state={nonce}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(location(&denied), "/#/login?error=oauth_denied");

    // The exchange itself failed. Whatever the provider said about it stays
    // between us and the provider.
    let broken = app
        .oneshot(authed_get(
            &format!("/api/auth/github/callback?code=bad-code&state={nonce}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(location(&broken), "/#/login?error=oauth_exchange");

    // Nothing was created by any of it.
    assert_eq!(
        db.collection::<Document>("users")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_verified_email_joins_the_account_that_already_has_it() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(verified("gh-9", "someone", "shared@example.com")),
    )
    .await;
    enable(&db, "github").await;

    // A password account, with the same address on it — stored in a different
    // case, which is still the same mailbox.
    app.clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "already-here", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    db.collection::<Document>("users")
        .update_one(
            doc! {"username": "already-here"},
            doc! {"$set": {"email": "Shared@Example.com"}},
        )
        .await
        .unwrap();

    let landed = flow(&app, "github", "code=ok", None).await;
    let session = session_cookie(&landed).unwrap();
    let me = app
        .oneshot(authed_get("/api/auth/me", &session))
        .await
        .unwrap();
    assert_eq!(
        body_json(me).await["username"],
        "already-here",
        "a verified address should join the account that holds it, not fork it"
    );
    assert_eq!(
        provider_id(&db, "already-here", "github_id")
            .await
            .as_deref(),
        Some("gh-9"),
        "and the identity should now be on it"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn an_unverified_email_starts_its_own_account() {
    let unverified = Profile {
        email_verified: false,
        ..verified("fb-1", "faker", "shared@example.com")
    };
    let (app, db) = test_app_with_providers(Some(FALLBACK), stubbed(unverified)).await;
    enable(&db, "facebook").await;

    app.clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "already-here", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    db.collection::<Document>("users")
        .update_one(
            doc! {"username": "already-here"},
            doc! {"$set": {"email": "shared@example.com"}},
        )
        .await
        .unwrap();

    let landed = flow(&app, "facebook", "code=ok", None).await;
    let session = session_cookie(&landed).unwrap();
    let me = app
        .oneshot(authed_get("/api/auth/me", &session))
        .await
        .unwrap();
    assert_ne!(
        body_json(me).await["username"],
        "already-here",
        "an unverified address is a claim, not proof"
    );
    assert!(
        provider_id(&db, "already-here", "facebook_id")
            .await
            .is_none(),
        "and it must not have been linked either"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_signed_in_browser_links_the_identity_instead_of_forking_an_account() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(Profile {
            email: None,
            email_verified: false,
            ..verified("gh-77", "linker", "")
        }),
    )
    .await;
    enable(&db, "github").await;

    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "linkme", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let session = session_cookie(&registered).unwrap();

    let landed = flow(&app, "github", "code=ok", Some(&session)).await;
    assert_eq!(location(&landed), "/#/");

    let me = app
        .clone()
        .oneshot(authed_get("/api/auth/me", &session))
        .await
        .unwrap();
    assert_eq!(body_json(me).await["username"], "linkme");
    assert_eq!(
        provider_id(&db, "linkme", "github_id").await.as_deref(),
        Some("gh-77")
    );
    assert_eq!(
        db.collection::<Document>("users")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1,
        "linking must not also have made an account"
    );

    // A second account trying to claim the same identity is refused.
    let other = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "otherone", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let other_session = session_cookie(&other).unwrap();
    let refused = flow(&app, "github", "code=ok", Some(&other_session)).await;
    assert_eq!(location(&refused), "/#/login?error=oauth_taken");
    assert!(
        provider_id(&db, "otherone", "github_id").await.is_none(),
        "the refusal must not have written anything"
    );

    db.drop().await.unwrap();
}

/// A session cookie the server no longer honours must not turn a sign-in into
/// a 401 page: the browser is mid-navigation, so the only useful answer is a
/// redirect. It falls back to an ordinary sign-in, which can only ever reach
/// the identity's own account.
#[tokio::test]
async fn a_stale_session_does_not_break_the_flow() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(verified("gh-5", "octocat", "octo@example.com")),
    )
    .await;
    enable(&db, "github").await;

    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "gonesoon", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let stale = session_cookie(&registered).unwrap();
    // Logging out bumps `token_version`, so the cookie is now a token the
    // server rejects.
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/auth/logout",
            &stale,
            json!({}),
        ))
        .await
        .unwrap();

    let landed = flow(&app, "github", "code=ok", Some(&stale)).await;
    assert_eq!(landed.status(), StatusCode::FOUND);
    assert_eq!(location(&landed), "/#/");
    let session = session_cookie(&landed).expect("it should still sign in");

    let me = app
        .oneshot(authed_get("/api/auth/me", &session))
        .await
        .unwrap();
    assert_eq!(
        body_json(me).await["username"],
        "octocat",
        "the stale session's account must not have been linked to"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_provider_can_be_disconnected_but_never_the_last_way_in() {
    let (app, db) = test_app_with_providers(
        Some(FALLBACK),
        stubbed(Profile {
            email: None,
            email_verified: false,
            ..verified("gh-55", "solo", "")
        }),
    )
    .await;
    enable(&db, "github").await;

    // An account that exists only through the provider.
    let landed = flow(&app, "github", "code=ok", None).await;
    let session = session_cookie(&landed).unwrap();

    // Removing it would leave no way to sign in at all.
    let refused = app
        .clone()
        .oneshot(authed_request(
            "DELETE",
            "/api/auth/github",
            &session,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(refused).await["error"]["code"],
        "last_login_method"
    );

    // With a password set, it may go. Setting one bumps `token_version`, and
    // the response carries the replacement cookie.
    let with_password = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/auth/password",
            &session,
            json!({"new_password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(with_password.status(), StatusCode::OK);
    let session = session_cookie(&with_password).unwrap();

    let removed = app
        .clone()
        .oneshot(authed_request(
            "DELETE",
            "/api/auth/github",
            &session,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    assert!(
        body_json(removed).await["providers"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the answer should already show it gone"
    );

    assert_eq!(
        provider_id(&db, "solo", "github_id").await,
        None,
        "the identity should be off the account, not merely hidden"
    );

    // Gone means gone: asking again is a 404, not a second success.
    let again = app
        .clone()
        .oneshot(authed_request(
            "DELETE",
            "/api/auth/github",
            &session,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND);

    // And a name that is not a provider at all never reaches the database.
    let nonsense = app
        .oneshot(authed_request(
            "DELETE",
            "/api/auth/myspace",
            &session,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(nonsense.status(), StatusCode::NOT_FOUND);

    db.drop().await.unwrap();
}
