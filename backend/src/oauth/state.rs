//! The cookie that ties a callback to the browser that started the flow.

use axum_extra::extract::cookie::{Cookie, SameSite};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::AppError;

pub const COOKIE: &str = "oauth_state";
/// Long enough to sign in, short enough that an abandoned flow goes stale.
const TTL_SECONDS: i64 = 600;

/// What the cookie remembers between the two requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    /// Echoed as the `state` parameter and compared on the way back.
    pub nonce: String,
    /// Whether this means "attach to my account" rather than "sign me in".
    pub link: bool,
    pub exp: i64,
}

/// Mints a nonce and the cookie remembering it. Signed, not stored: Lambda
/// keeps nothing between requests, and a plain cookie would let a neighbouring
/// subdomain forge `link` and graft its identity onto someone's account.
pub fn issue(config: &Config, link: bool) -> Result<(String, Cookie<'static>), AppError> {
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let flow = Flow {
        nonce: nonce.clone(),
        link,
        exp: (chrono::Utc::now() + chrono::Duration::seconds(TTL_SECONDS)).timestamp(),
    };
    let token = jsonwebtoken::encode(
        &Header::default(),
        &flow,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(AppError::internal)?;
    Ok((nonce, build(config, token, TTL_SECONDS)))
}

/// The flow this cookie describes, if it verifies, is unexpired, and names the
/// nonce handed back. Any doubt answers `None`. The nonce comparison is what
/// ties the callback to the browser that started it.
pub fn verify(config: &Config, cookie_value: &str, state_param: &str) -> Option<Flow> {
    let mut validation = Validation::new(Algorithm::HS256);
    // No leeway: both timestamps come from this server's own clock.
    validation.leeway = 0;
    let flow = jsonwebtoken::decode::<Flow>(
        cookie_value,
        &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
        &validation,
    )
    .ok()?
    .claims;
    (!state_param.is_empty() && flow.nonce == state_param).then_some(flow)
}

/// Same attributes, immediate expiry: the replacement must match to drop it.
pub fn cleared(config: &Config) -> Cookie<'static> {
    build(config, String::new(), 0)
}

fn build(config: &Config, value: String, max_age: i64) -> Cookie<'static> {
    Cookie::build((COOKIE, value))
        .http_only(true)
        .secure(config.cookie_secure)
        // Lax: Strict withholds the cookie on the provider's own redirect.
        .same_site(SameSite::Lax)
        // Narrower than the session cookie; nothing else needs it.
        .path("/api/auth")
        .max_age(time::Duration::seconds(max_age))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(secret: &str) -> Config {
        Config {
            mongo_url: String::new(),
            db_name: "test".into(),
            port: 0,
            notfound_fallback_url: None,
            public_base_url: None,
            jwt_secret: secret.into(),
            session_days: 60,
            cookie_secure: false,
            orphan_grace_days: 7,
        }
    }

    #[test]
    fn a_state_round_trips_and_carries_the_link_flag() {
        let config = config("secret-a");
        let (nonce, cookie) = issue(&config, true).unwrap();

        let flow = verify(&config, cookie.value(), &nonce).expect("its own state must verify");
        assert!(
            flow.link,
            "the flag decides whether this attaches to a session"
        );

        // Proves this browser started the flow.
        assert!(verify(&config, cookie.value(), "some-other-nonce").is_none());
    }

    #[test]
    fn a_cookie_from_elsewhere_is_refused() {
        let ours = config("secret-a");
        let theirs = config("secret-b");
        let (nonce, cookie) = issue(&theirs, true).unwrap();
        // A cookie from a neighbouring subdomain must not claim `link`.
        assert!(verify(&ours, cookie.value(), &nonce).is_none());
        assert!(verify(&ours, "not-a-token", &nonce).is_none());
        assert!(verify(&ours, "", "").is_none());
    }

    /// One second past is past: `jsonwebtoken` defaults to sixty seconds of
    /// leeway, which [`verify`] turns off.
    #[test]
    fn an_expired_state_is_refused_even_with_the_right_nonce() {
        let config = config("secret-a");
        let nonce = "still-the-right-nonce";
        let expired_by = |seconds: i64| {
            let stale = Flow {
                nonce: nonce.into(),
                link: true,
                exp: (chrono::Utc::now() - chrono::Duration::seconds(seconds)).timestamp(),
            };
            jsonwebtoken::encode(
                &Header::default(),
                &stale,
                &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
            )
            .unwrap()
        };
        assert!(
            verify(&config, &expired_by(1), nonce).is_none(),
            "expiry is checked without the crate's default grace period"
        );
        assert!(verify(&config, &expired_by(3600), nonce).is_none());
    }

    #[test]
    fn every_flow_gets_its_own_nonce() {
        let config = config("secret-a");
        let (first, _) = issue(&config, false).unwrap();
        let (second, _) = issue(&config, false).unwrap();
        assert_ne!(first, second);
        // Long enough that guessing one is not a way in.
        assert!(first.len() >= 48, "{first} is too short to be unguessable");
    }

    #[test]
    fn the_cookie_is_scoped_and_short_lived() {
        let config = config("secret-a");
        let (_, cookie) = issue(&config, false).unwrap();
        assert_eq!(cookie.name(), COOKIE);
        assert_eq!(cookie.path(), Some("/api/auth"));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.max_age(), Some(time::Duration::seconds(600)));

        // Clearing keeps name and path, or the browser keeps the old one.
        let gone = cleared(&config);
        assert_eq!(gone.name(), COOKIE);
        assert_eq!(gone.path(), Some("/api/auth"));
        assert_eq!(gone.max_age(), Some(time::Duration::ZERO));
    }

    /// Secure follows the session cookie, so a local setup that turned it off
    /// does not end up with a state cookie the browser refuses.
    #[test]
    fn secure_follows_the_deployments_own_setting() {
        let mut config = config("secret-a");
        assert_eq!(issue(&config, false).unwrap().1.secure(), Some(false));
        config.cookie_secure = true;
        assert_eq!(issue(&config, false).unwrap().1.secure(), Some(true));
    }
}
