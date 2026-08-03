use async_trait::async_trait;
use serde::Deserialize;

use super::{Profile, Provider, ProviderKind, failed, form_encode, unusable};
use crate::error::AppError;
use crate::settings::MethodConfig;

const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";

pub struct Google;

#[derive(Deserialize)]
struct UserInfo {
    sub: Option<String>,
    name: Option<String>,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    picture: Option<String>,
}

/// Reads the `userinfo` endpoint rather than the `id_token`: decoding the
/// token properly means fetching and caching Google's JWKS inside a Lambda,
/// and the access token is already proof enough for a one-shot read.
pub fn parse_user(body: &str) -> Result<Profile, AppError> {
    let info: UserInfo = serde_json::from_str(body).map_err(|_| unusable())?;
    let id = info
        .sub
        .filter(|sub| !sub.is_empty())
        .ok_or_else(unusable)?;
    Ok(Profile {
        id,
        // Google has no handle to offer; the username is derived from the
        // display name instead.
        username: None,
        display_name: info.name,
        email: info.email,
        email_verified: info.email_verified,
        avatar_url: info.picture,
    })
}

#[async_trait]
impl Provider for Google {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Google
    }

    fn authorize_url(&self, cfg: &MethodConfig, redirect_uri: &str, state: &str) -> String {
        format!(
            "{AUTHORIZE}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
            form_encode(cfg.client_id.as_deref().unwrap_or_default()),
            form_encode(redirect_uri),
            form_encode("openid email profile"),
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
            .form(&[
                ("client_id", cfg.client_id.clone().unwrap_or_default()),
                (
                    "client_secret",
                    cfg.client_secret.clone().unwrap_or_default(),
                ),
                ("code", code.to_string()),
                ("redirect_uri", redirect_uri.to_string()),
                ("grant_type", "authorization_code".to_string()),
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
        let body = reqwest::Client::new()
            .get(USERINFO)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| failed())?
            .text()
            .await
            .map_err(|_| failed())?;
        parse_user(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorize_url_asks_for_the_openid_scopes() {
        let cfg = MethodConfig {
            enabled: true,
            client_id: Some("goog-1".into()),
            client_secret: Some("shh".into()),
        };
        let url = Google.authorize_url(&cfg, "https://nourl.space/api/auth/google/callback", "n1");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=openid%20email%20profile"));
        assert!(url.contains("state=n1"));
        assert!(!url.contains("shh"));
    }

    #[test]
    fn userinfo_becomes_a_profile_and_honours_the_verified_flag() {
        let verified = r#"{
            "sub": "10101",
            "name": "A Person",
            "email": "person@example.com",
            "email_verified": true,
            "picture": "https://pics.example/p.png"
        }"#;
        let profile = parse_user(verified).unwrap();
        assert_eq!(profile.id, "10101");
        assert_eq!(profile.email.as_deref(), Some("person@example.com"));
        assert!(profile.email_verified);
        assert_eq!(profile.display_name.as_deref(), Some("A Person"));
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://pics.example/p.png")
        );
        // Google has no handle; the username is derived from the display name.
        assert_eq!(profile.username, None);

        let unverified = r#"{"sub": "2", "email": "x@example.com", "email_verified": false}"#;
        assert!(!parse_user(unverified).unwrap().email_verified);
        // Absent is not verified either.
        let silent = r#"{"sub": "3", "email": "x@example.com"}"#;
        assert!(!parse_user(silent).unwrap().email_verified);

        assert!(parse_user(r#"{"email": "no-sub@example.com"}"#).is_err());
        assert!(parse_user(r#"{"sub": ""}"#).is_err());
    }
}
