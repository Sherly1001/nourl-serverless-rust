//! Signing in with GitHub, Google or Facebook.
//!
//! Everything a provider needs sits behind [`Provider`], so the routes never
//! learn which one they are talking to and the integration tests can hand the
//! router a stub instead of a live OAuth app.

pub mod facebook;
pub mod github;
pub mod google;
pub mod state;

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::AppError;
use crate::settings::MethodConfig;

/// Which provider, and the `users` field its id is stored in.
///
/// One enum rather than three code paths: every lookup, link and disconnect is
/// the same query with a different field name, and the field never comes from
/// user input — only from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Github,
    Google,
    Facebook,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 3] = [Self::Github, Self::Google, Self::Facebook];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Google => "google",
            Self::Facebook => "facebook",
        }
    }

    pub fn field(self) -> &'static str {
        match self {
            Self::Github => "github_id",
            Self::Google => "google_id",
            Self::Facebook => "facebook_id",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }
}

/// What every provider boils down to.
///
/// `email_verified` is false unless the provider positively says otherwise: it
/// is the only thing standing between "sign in with a provider" and "take over
/// an account by claiming its email address".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub avatar_url: Option<String>,
}

/// The three calls an OAuth sign-in needs, in the order it needs them.
///
/// A trait rather than free functions so the integration tests can hand the
/// router a stub: the alternative is a live GitHub app in CI.
#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// Where to send the browser. `state` is echoed back to the callback.
    fn authorize_url(&self, cfg: &MethodConfig, redirect_uri: &str, state: &str) -> String;

    /// Trades the callback's `code` for an access token.
    async fn exchange(
        &self,
        cfg: &MethodConfig,
        redirect_uri: &str,
        code: &str,
    ) -> Result<String, AppError>;

    async fn profile(&self, access_token: &str) -> Result<Profile, AppError>;
}

/// One implementation per kind, so a handler can go straight from a path
/// segment to the thing that talks to that provider.
pub struct Providers {
    github: Arc<dyn Provider>,
    google: Arc<dyn Provider>,
    facebook: Arc<dyn Provider>,
}

impl Providers {
    pub fn production() -> Self {
        Self {
            github: Arc::new(github::Github),
            google: Arc::new(google::Google),
            facebook: Arc::new(facebook::Facebook),
        }
    }

    /// Used by the tests to swap any subset for a stub.
    pub fn from_parts(
        github: Arc<dyn Provider>,
        google: Arc<dyn Provider>,
        facebook: Arc<dyn Provider>,
    ) -> Self {
        Self {
            github,
            google,
            facebook,
        }
    }

    pub fn get(&self, kind: ProviderKind) -> Arc<dyn Provider> {
        match kind {
            ProviderKind::Github => Arc::clone(&self.github),
            ProviderKind::Google => Arc::clone(&self.google),
            ProviderKind::Facebook => Arc::clone(&self.facebook),
        }
    }
}

/// Percent-encodes a query value. Hand-rolled rather than pulling a crate for
/// a handful of call sites — and `reqwest`'s encoder only applies to bodies it
/// builds, not to a URL assembled here.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One message for every failure on the way to a profile.
///
/// The provider's own wording is deliberately dropped rather than forwarded: it
/// is not ours to show, and it has been known to echo request parameters back.
pub fn failed() -> AppError {
    AppError::validation("could not complete the sign-in with that provider")
}

/// A response that parsed but named nobody. Separate from [`failed`] only in
/// wording; neither says anything the provider told us.
pub fn unusable() -> AppError {
    AppError::validation("the provider returned no usable account")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_name_maps_to_its_stored_field() {
        assert_eq!(ProviderKind::parse("github"), Some(ProviderKind::Github));
        assert_eq!(ProviderKind::parse("google"), Some(ProviderKind::Google));
        assert_eq!(
            ProviderKind::parse("facebook"),
            Some(ProviderKind::Facebook)
        );
        // Anything else is not a provider, and must not become a Mongo field
        // name — the field is what the lookup queries on.
        assert_eq!(ProviderKind::parse("hash_passwd"), None);
        assert_eq!(ProviderKind::parse("GitHub"), None);

        assert_eq!(ProviderKind::Github.field(), "github_id");
        assert_eq!(ProviderKind::Google.field(), "google_id");
        assert_eq!(ProviderKind::Facebook.field(), "facebook_id");

        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn a_query_value_is_percent_encoded() {
        assert_eq!(
            form_encode("read:user user:email"),
            "read%3Auser%20user%3Aemail"
        );
        assert_eq!(
            form_encode("https://nourl.space/api/auth/github/callback"),
            "https%3A%2F%2Fnourl.space%2Fapi%2Fauth%2Fgithub%2Fcallback"
        );
        assert_eq!(form_encode("plain-value_1.0~"), "plain-value_1.0~");
    }

    /// Nothing a provider says reaches the browser: both messages are fixed
    /// strings, so a provider cannot put words in our mouth.
    #[test]
    fn a_failure_says_nothing_the_provider_told_us() {
        assert_eq!(
            failed().message,
            "could not complete the sign-in with that provider"
        );
        assert_eq!(
            unusable().message,
            "the provider returned no usable account"
        );
    }
}
