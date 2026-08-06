#[derive(Clone, Debug)]
pub struct Config {
    pub mongo_url: String,
    pub db_name: String,
    pub port: u16,
    /// Explicit override for the not-found redirect target. When `None`, the
    /// redirect falls back to the request's own host (`https://{host}`).
    pub notfound_fallback_url: Option<String>,
    /// Origin the provider redirects back to, e.g. `https://nourl.space`. The
    /// OAuth callback URL is built from it, and it has to match what is
    /// registered at the provider. Taken from configuration rather than the
    /// request's own `Host` header, which a caller controls. `None` falls back
    /// to `http://127.0.0.1:8080` — the Trunk dev server, which proxies `/api`.
    pub public_base_url: Option<String>,
    /// HMAC secret for session tokens.
    pub jwt_secret: String,
    /// Lifetime of a session token and its cookie. Both must agree, or the
    /// browser would keep sending a cookie the server has already rejected.
    pub session_days: i64,
    /// `Secure` attribute on the session cookie. Browsers treat localhost as a
    /// secure context, so this only needs turning off for exotic local setups.
    pub cookie_secure: bool,
    /// How long a link outlives the account that owned it, when that account is
    /// deleted and its links are left in place. They keep working, unowned,
    /// until this runs out — long enough for someone to notice a link has gone
    /// unowned and claim it, short enough that abandoned links do not
    /// accumulate for ever.
    pub orphan_grace_days: i64,
}

fn parse_cookie_secure(raw: Option<String>) -> bool {
    !raw.is_some_and(|v| v.eq_ignore_ascii_case("false"))
}

/// A positive number of days, or `default` for anything unparseable or
/// non-positive.
///
/// Zero and negative are refused rather than honoured: a session lifetime of
/// zero would mint tokens that are already expired, and a grace period of zero
/// would delete a link the moment its owner left rather than giving anyone the
/// chance to claim it.
fn parse_days(raw: Option<String>, default: i64) -> i64 {
    raw.and_then(|v| v.parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(default)
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
            public_base_url: std::env::var("PUBLIC_BASE_URL")
                .ok()
                .filter(|v| !v.is_empty()),
            jwt_secret: std::env::var("JWT_SECRET")
                .map_err(|_| "JWT_SECRET environment variable is required".to_string())?,
            session_days: parse_days(std::env::var("JWT_SESSION_DAYS").ok(), 60),
            cookie_secure: parse_cookie_secure(std::env::var("COOKIE_SECURE").ok()),
            orphan_grace_days: parse_days(std::env::var("ORPHAN_GRACE_DAYS").ok(), 7),
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
    fn day_counts_default_and_reject_nonsense() {
        assert_eq!(parse_days(None, 60), 60);
        assert_eq!(parse_days(Some("7".into()), 60), 7);
        assert_eq!(parse_days(Some("abc".into()), 60), 60);
        // Zero and negative fall back rather than being honoured.
        assert_eq!(parse_days(Some("0".into()), 60), 60);
        assert_eq!(parse_days(Some("-5".into()), 60), 60);
        // The grace period shares the parser but not the default.
        assert_eq!(parse_days(None, 7), 7);
        assert_eq!(parse_days(Some("30".into()), 7), 30);
    }
}
