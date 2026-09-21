use async_trait::async_trait;
use serde::Deserialize;

use super::{Profile, Provider, ProviderKind, failed, form_encode, unusable};
use crate::error::AppError;
use crate::settings::MethodConfig;

const AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
const TOKEN: &str = "https://github.com/login/oauth/access_token";
const USER: &str = "https://api.github.com/user";
const EMAILS: &str = "https://api.github.com/user/emails";
/// GitHub rejects API requests that arrive without one.
const AGENT: &str = "nourl";

pub struct Github;

#[derive(Deserialize)]
struct User {
    id: Option<i64>,
    login: Option<String>,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct Email {
    email: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    verified: bool,
}

/// `/user` gives everything but a trustworthy email.
pub fn parse_user(body: &str) -> Result<Profile, AppError> {
    let user: User = serde_json::from_str(body).map_err(|_| unusable())?;
    let id = user.id.ok_or_else(unusable)?;
    Ok(Profile {
        id: id.to_string(),
        username: user.login,
        display_name: user.name,
        email: None,
        email_verified: false,
        avatar_url: user.avatar_url,
    })
}

/// The primary verified address, else the first verified, else nothing.
/// Unverified is a claim, not a fact, and this decides whether an identity may
/// join an account that already exists.
pub fn pick_verified_email(body: &str) -> Option<String> {
    let emails: Vec<Email> = serde_json::from_str(body).ok()?;
    let verified = || emails.iter().filter(|e| e.verified);
    verified()
        .find(|e| e.primary)
        .or_else(|| verified().next())
        .map(|e| e.email.clone())
}

#[async_trait]
impl Provider for Github {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Github
    }

    fn authorize_url(&self, cfg: &MethodConfig, redirect_uri: &str, state: &str) -> String {
        format!(
            "{AUTHORIZE}?client_id={}&redirect_uri={}&scope={}&state={}",
            form_encode(cfg.client_id.as_deref().unwrap_or_default()),
            form_encode(redirect_uri),
            form_encode("read:user user:email"),
            form_encode(state),
        )
    }

    async fn exchange(
        &self,
        cfg: &MethodConfig,
        redirect_uri: &str,
        code: &str,
    ) -> Result<String, AppError> {
        #[derive(Deserialize)]
        struct Token {
            access_token: Option<String>,
        }
        let token: Token = reqwest::Client::new()
            .post(TOKEN)
            // Without this GitHub answers form-encoded, whatever the docs imply.
            .header("accept", "application/json")
            .header("user-agent", AGENT)
            .form(&[
                ("client_id", cfg.client_id.clone().unwrap_or_default()),
                (
                    "client_secret",
                    cfg.client_secret.clone().unwrap_or_default(),
                ),
                ("code", code.to_string()),
                ("redirect_uri", redirect_uri.to_string()),
            ])
            .send()
            .await
            .map_err(|_| failed())?
            .json()
            .await
            .map_err(|_| failed())?;
        token.access_token.ok_or_else(failed)
    }

    async fn profile(&self, access_token: &str) -> Result<Profile, AppError> {
        let client = reqwest::Client::new();
        let user = client
            .get(USER)
            .bearer_auth(access_token)
            .header("user-agent", AGENT)
            .send()
            .await
            .map_err(|_| failed())?
            .text()
            .await
            .map_err(|_| failed())?;
        let mut profile = parse_user(&user)?;

        // A second call: /user's email is public, absent, or unverified.
        if let Ok(response) = client
            .get(EMAILS)
            .bearer_auth(access_token)
            .header("user-agent", AGENT)
            .send()
            .await
            && let Ok(body) = response.text().await
            && let Some(email) = pick_verified_email(&body)
        {
            profile.email = Some(email);
            profile.email_verified = true;
        }
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MethodConfig {
        MethodConfig {
            enabled: true,
            client_id: Some("client-123".into()),
            client_secret: Some("secret-456".into()),
        }
    }

    #[test]
    fn the_authorize_url_carries_the_client_state_and_redirect() {
        let url = Github.authorize_url(
            &cfg(),
            "https://nourl.space/api/auth/github/callback",
            "nonce-789",
        );
        assert!(url.starts_with("https://github.com/login/oauth/authorize?"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("state=nonce-789"));
        assert!(url.contains("scope=read%3Auser%20user%3Aemail"));
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fnourl.space%2Fapi%2Fauth%2Fgithub%2Fcallback")
        );
        // The secret is for the exchange only and must never reach the browser.
        assert!(!url.contains("secret-456"));
    }

    #[test]
    fn a_user_response_becomes_a_profile() {
        let body = r#"{
            "id": 4242,
            "login": "octocat",
            "name": "The Octocat",
            "avatar_url": "https://avatars.example/octocat.png"
        }"#;
        let profile = parse_user(body).unwrap();
        assert_eq!(profile.id, "4242", "the numeric id is stored as a string");
        assert_eq!(profile.username.as_deref(), Some("octocat"));
        assert_eq!(profile.display_name.as_deref(), Some("The Octocat"));
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://avatars.example/octocat.png")
        );
        // /user does not carry a verified email; that comes from /user/emails.
        assert_eq!(profile.email, None);
        assert!(!profile.email_verified);

        // A missing id is unusable: there is nothing to key an account on.
        assert!(parse_user(r#"{"login": "nobody"}"#).is_err());
        assert!(parse_user("not json").is_err());
    }

    #[test]
    fn only_a_primary_verified_email_counts() {
        let body = r#"[
            {"email": "old@example.com", "primary": false, "verified": true},
            {"email": "me@example.com", "primary": true, "verified": true}
        ]"#;
        assert_eq!(pick_verified_email(body).as_deref(), Some("me@example.com"));

        // Primary but unverified is exactly the case that must not link.
        let unverified = r#"[{"email": "me@example.com", "primary": true, "verified": false}]"#;
        assert_eq!(pick_verified_email(unverified), None);

        // A verified address is preferred over an unverified primary one.
        let mixed = r#"[
            {"email": "unconfirmed@example.com", "primary": true, "verified": false},
            {"email": "confirmed@example.com", "primary": false, "verified": true}
        ]"#;
        assert_eq!(
            pick_verified_email(mixed).as_deref(),
            Some("confirmed@example.com")
        );

        // No primary flag anywhere: fall back to the first verified one.
        let no_primary = r#"[{"email": "me@example.com", "primary": false, "verified": true}]"#;
        assert_eq!(
            pick_verified_email(no_primary).as_deref(),
            Some("me@example.com")
        );

        assert_eq!(pick_verified_email("not json"), None);
        assert_eq!(pick_verified_email("[]"), None);
    }
}
