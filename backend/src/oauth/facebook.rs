use async_trait::async_trait;
use serde::Deserialize;

use super::{Profile, Provider, ProviderKind, failed, form_encode, unusable};
use crate::error::AppError;
use crate::settings::MethodConfig;

const AUTHORIZE: &str = "https://www.facebook.com/v21.0/dialog/oauth";
const TOKEN: &str = "https://graph.facebook.com/v21.0/oauth/access_token";
const ME: &str = "https://graph.facebook.com/v21.0/me?fields=id,name,email,picture";

pub struct Facebook;

#[derive(Deserialize)]
struct Me {
    id: Option<String>,
    name: Option<String>,
    email: Option<String>,
    picture: Option<Picture>,
}

#[derive(Deserialize)]
struct Picture {
    data: Option<PictureData>,
}

#[derive(Deserialize)]
struct PictureData {
    url: Option<String>,
}

pub fn parse_user(body: &str) -> Result<Profile, AppError> {
    let me: Me = serde_json::from_str(body).map_err(|_| unusable())?;
    let id = me.id.filter(|id| !id.is_empty()).ok_or_else(unusable)?;
    Ok(Profile {
        id,
        username: None,
        display_name: me.name,
        email: me.email,
        // Never true: the Graph API does not say whether the address was
        // confirmed, and guessing would be an account-takeover path.
        email_verified: false,
        avatar_url: me.picture.and_then(|p| p.data).and_then(|d| d.url),
    })
}

#[async_trait]
impl Provider for Facebook {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Facebook
    }

    fn authorize_url(&self, cfg: &MethodConfig, redirect_uri: &str, state: &str) -> String {
        format!(
            "{AUTHORIZE}?client_id={}&redirect_uri={}&response_type=code&scope=email&state={}",
            form_encode(cfg.client_id.as_deref().unwrap_or_default()),
            form_encode(redirect_uri),
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
            .get(TOKEN)
            .query(&[
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
        let body = reqwest::Client::new()
            .get(ME)
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
    fn the_authorize_url_asks_only_for_email() {
        let cfg = MethodConfig {
            enabled: true,
            client_id: Some("fb-1".into()),
            client_secret: Some("shh".into()),
        };
        let url =
            Facebook.authorize_url(&cfg, "https://nourl.space/api/auth/facebook/callback", "n2");
        assert!(url.starts_with("https://www.facebook.com/v21.0/dialog/oauth?"));
        assert!(url.contains("scope=email"));
        assert!(url.contains("state=n2"));
        assert!(!url.contains("shh"));
    }

    /// Facebook does not report whether an address is verified, so every
    /// profile it returns is unverified — which is what keeps it from ever
    /// joining an existing account by email alone.
    #[test]
    fn a_facebook_email_is_never_treated_as_verified() {
        let body = r#"{
            "id": "77",
            "name": "Someone",
            "email": "someone@example.com",
            "picture": {"data": {"url": "https://pics.example/f.png"}}
        }"#;
        let profile = parse_user(body).unwrap();
        assert_eq!(profile.id, "77");
        assert_eq!(profile.email.as_deref(), Some("someone@example.com"));
        assert!(!profile.email_verified);
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://pics.example/f.png")
        );

        // Even if the Graph API were to start claiming it, the field is not
        // read — a Facebook profile stays unverified.
        let claiming = r#"{"id": "78", "email": "x@example.com", "email_verified": true}"#;
        assert!(!parse_user(claiming).unwrap().email_verified);

        // No picture object at all is normal and must not fail the parse.
        let bare = r#"{"id": "79", "name": "Bare"}"#;
        assert_eq!(parse_user(bare).unwrap().avatar_url, None);

        assert!(parse_user(r#"{"name": "No id"}"#).is_err());
        assert!(parse_user(r#"{"id": ""}"#).is_err());
    }
}
