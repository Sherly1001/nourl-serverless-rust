//! Signing in with GitHub, Google or Facebook, all behind [`Provider`] so the
//! routes never learn which, and a test can hand the router a stub.

pub mod account;
pub mod facebook;
pub mod github;
pub mod google;
pub mod state;

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::AppError;
use crate::settings::MethodConfig;

/// Which provider, and the `users` field its id lives in. One enum, because
/// every lookup is the same query with a different field — and that field
/// never comes from user input.
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

/// What every provider boils down to. `email_verified` is false unless one
/// says otherwise: it is all that stands between signing in and taking over an
/// account by claiming its address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub avatar_url: Option<String>,
}

/// The three calls an OAuth sign-in needs. A trait so a test can stub it; the
/// alternative is a live GitHub app in CI.
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

/// One per kind, so a path segment maps straight to its provider.
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

/// Hand-rolled: `reqwest`'s encoder only covers bodies it builds itself.
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

/// One message for every failure. A provider's own wording is dropped — it has
/// been known to echo request parameters back.
pub fn failed() -> AppError {
    AppError::validation("could not complete the sign-in with that provider")
}

/// Parsed but named nobody. Differs from [`failed`] only in wording.
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
        // Anything else must not become a Mongo field name.
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

    /// Fixed strings, so a provider cannot put words in our mouth.
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
