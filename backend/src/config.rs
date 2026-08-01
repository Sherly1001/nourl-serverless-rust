#[derive(Clone, Debug)]
pub struct Config {
    pub mongo_url: String,
    pub db_name: String,
    pub port: u16,
    /// Explicit override for the not-found redirect target. When `None`, the
    /// redirect falls back to the request's own host (`https://{host}`).
    pub notfound_fallback_url: Option<String>,
    /// HMAC secret for session tokens.
    pub jwt_secret: String,
    /// Lifetime of a session token and its cookie. Both must agree, or the
    /// browser would keep sending a cookie the server has already rejected.
    pub session_days: i64,
    /// `Secure` attribute on the session cookie. Browsers treat localhost as a
    /// secure context, so this only needs turning off for exotic local setups.
    pub cookie_secure: bool,
}

fn parse_cookie_secure(raw: Option<String>) -> bool {
    !raw.is_some_and(|v| v.eq_ignore_ascii_case("false"))
}

/// Falls back to 60 days for anything unparseable or non-positive — a zero or
/// negative lifetime would mint tokens that are already expired.
fn parse_session_days(raw: Option<String>) -> i64 {
    raw.and_then(|v| v.parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(60)
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            mongo_url: std::env::var("MONGO_URL")
                .map_err(|_| "MONGO_URL environment variable is required".to_string())?,
            db_name: std::env::var("MONGO_DB").unwrap_or_else(|_| "nourl".into()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(9669),
            notfound_fallback_url: std::env::var("NOTFOUND_FALLBACK_URL")
                .ok()
                .filter(|v| !v.is_empty()),
            jwt_secret: std::env::var("JWT_SECRET")
                .map_err(|_| "JWT_SECRET environment variable is required".to_string())?,
            session_days: parse_session_days(std::env::var("JWT_SESSION_DAYS").ok()),
            cookie_secure: parse_cookie_secure(std::env::var("COOKIE_SECURE").ok()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_secure_defaults_on_and_is_opt_out_only() {
        assert!(parse_cookie_secure(None));
        assert!(parse_cookie_secure(Some("true".into())));
        assert!(parse_cookie_secure(Some("anything".into())));
        assert!(!parse_cookie_secure(Some("false".into())));
        assert!(!parse_cookie_secure(Some("FALSE".into())));
    }

    #[test]
    fn session_days_defaults_and_rejects_nonsense() {
        assert_eq!(parse_session_days(None), 60);
        assert_eq!(parse_session_days(Some("7".into())), 7);
        assert_eq!(parse_session_days(Some("abc".into())), 60);
        assert_eq!(parse_session_days(Some("0".into())), 60);
        assert_eq!(parse_session_days(Some("-5".into())), 60);
    }
}
