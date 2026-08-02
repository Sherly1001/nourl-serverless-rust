use axum_extra::extract::cookie::{Cookie, SameSite};

use crate::config::Config;

pub const NAME: &str = "nourl_session";

/// SameSite=Lax rather than Strict so the OAuth callback redirect in phase 2b
/// still carries the freshly set cookie.
///
/// `Max-Age` comes from the same `session_days` the token's `exp` is built
/// from, so the browser stops sending the cookie exactly when the server would
/// start rejecting it.
pub fn session(config: &Config, token: String) -> Cookie<'static> {
    Cookie::build((NAME, token))
        .http_only(true)
        .secure(config.cookie_secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::days(config.session_days))
        .build()
}

/// Same attributes with an immediate expiry — browsers only drop a cookie when
/// the replacement matches on name, path and domain.
pub fn cleared(config: &Config) -> Cookie<'static> {
    Cookie::build((NAME, ""))
        .http_only(true)
        .secure(config.cookie_secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(secure: bool, session_days: i64) -> Config {
        Config {
            mongo_url: String::new(),
            db_name: "t".into(),
            port: 0,
            notfound_fallback_url: None,
            jwt_secret: "s".into(),
            session_days,
            cookie_secure: secure,
            orphan_grace_days: 7,
        }
    }

    #[test]
    fn session_cookie_carries_the_spec_attributes() {
        let cookie = session(&config(true, 60), "token-value".into());
        assert_eq!(cookie.name(), NAME);
        assert_eq!(cookie.value(), "token-value");
        assert!(cookie.http_only().unwrap());
        assert!(cookie.secure().unwrap());
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.max_age().unwrap().whole_days(), 60);
    }

    #[test]
    fn max_age_tracks_the_configured_lifetime() {
        let cookie = session(&config(true, 7), "t".into());
        assert_eq!(cookie.max_age().unwrap().whole_days(), 7);
    }

    #[test]
    fn secure_flag_follows_config() {
        assert!(!session(&config(false, 60), "t".into()).secure().unwrap());
    }

    #[test]
    fn cleared_cookie_expires_immediately_and_keeps_the_path() {
        let cookie = cleared(&config(true, 60));
        assert_eq!(cookie.value(), "");
        assert_eq!(cookie.max_age().unwrap().whole_seconds(), 0);
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
    }
}
