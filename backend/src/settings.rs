use mongodb::Database;
use mongodb::bson::{Document, doc};
use serde::{Deserialize, Serialize};
use shared::{AdminSettings, AuthMethods, MethodUpdate, MethodView, UpdateSettingsRequest};

use crate::error::AppError;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MethodConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
}

/// The single `settings` document. Every field defaults, so a fresh or
/// half-filled database needs no seeding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    #[serde(default = "password_default")]
    pub password: MethodConfig,
    #[serde(default)]
    pub github: MethodConfig,
    #[serde(default)]
    pub google: MethodConfig,
    #[serde(default)]
    pub facebook: MethodConfig,
}

fn password_default() -> MethodConfig {
    MethodConfig {
        enabled: true,
        client_id: None,
        client_secret: None,
    }
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            password: password_default(),
            github: MethodConfig::default(),
            google: MethodConfig::default(),
            facebook: MethodConfig::default(),
        }
    }
}

impl AuthSettings {
    /// What a login page should offer. A provider counts only once it has
    /// credentials; advertising one without them breaks the redirect.
    pub fn methods(&self) -> AuthMethods {
        let configured =
            |m: &MethodConfig| m.enabled && m.client_id.is_some() && m.client_secret.is_some();
        AuthMethods {
            password: self.password.enabled,
            github: configured(&self.github),
            google: configured(&self.google),
            facebook: configured(&self.facebook),
        }
    }
}

pub async fn load(db: &Database) -> Result<AuthSettings, AppError> {
    match db
        .collection::<Document>("settings")
        .find_one(doc! {"_id": "auth"})
        .await?
    {
        Some(doc) => bson::from_document(doc).map_err(AppError::internal),
        None => Ok(AuthSettings::default()),
    }
}

impl MethodConfig {
    fn to_view(&self) -> MethodView {
        MethodView {
            enabled: self.enabled,
            client_id: self.client_id.clone(),
            // Whether one is set, never the secret itself.
            has_secret: self.client_secret.is_some(),
        }
    }

    /// An omitted `client_secret` keeps the stored one — the page never
    /// receives it, so absent would otherwise wipe it on every save. An empty
    /// string retires one deliberately.
    fn merged(&self, update: &MethodUpdate) -> Self {
        let client_secret = match update.client_secret.as_deref() {
            None => self.client_secret.clone(),
            Some("") => None,
            Some(secret) => Some(secret.to_string()),
        };
        Self {
            enabled: update.enabled,
            client_id: update
                .client_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(String::from),
            client_secret,
        }
    }
}

impl AuthSettings {
    /// What an admin may see: everything except the secrets themselves.
    pub fn to_admin_view(&self) -> AdminSettings {
        AdminSettings {
            password: self.password.to_view(),
            github: self.github.to_view(),
            google: self.google.to_view(),
            facebook: self.facebook.to_view(),
        }
    }

    pub fn merged(&self, update: &UpdateSettingsRequest) -> Self {
        Self {
            password: self.password.merged(&update.password),
            github: self.github.merged(&update.github),
            google: self.google.merged(&update.google),
            facebook: self.facebook.merged(&update.facebook),
        }
    }
}

/// Writes the whole document, upserting so nothing needs seeding.
pub async fn save(db: &Database, settings: &AuthSettings) -> Result<(), AppError> {
    let doc = bson::to_document(settings).map_err(AppError::internal)?;
    db.collection::<Document>("settings")
        .update_one(doc! {"_id": "auth"}, doc! {"$set": doc})
        .upsert(true)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(secret: Option<&str>) -> MethodConfig {
        MethodConfig {
            enabled: true,
            client_id: Some("id".into()),
            client_secret: secret.map(String::from),
        }
    }

    fn update(enabled: bool, client_id: Option<&str>, secret: Option<&str>) -> MethodUpdate {
        MethodUpdate {
            enabled,
            client_id: client_id.map(String::from),
            client_secret: secret.map(String::from),
        }
    }

    /// The rule the whole settings page depends on: it renders `has_secret`,
    /// never the secret, so it cannot echo one back.
    #[test]
    fn an_omitted_secret_is_kept_and_an_empty_one_retires_it() {
        let stored = configured(Some("s3cret"));

        // Absent: unchanged.
        let same = stored.merged(&update(true, Some("id"), None));
        assert_eq!(same.client_secret.as_deref(), Some("s3cret"));

        // Empty string: deliberately cleared.
        let cleared = stored.merged(&update(true, Some("id"), Some("")));
        assert!(cleared.client_secret.is_none());

        // A new value replaces it.
        let rotated = stored.merged(&update(true, Some("id"), Some("newer")));
        assert_eq!(rotated.client_secret.as_deref(), Some("newer"));

        // Off keeps the credentials, so it can go back on without re-entry.
        let disabled = stored.merged(&update(false, Some("id"), None));
        assert!(!disabled.enabled);
        assert_eq!(disabled.client_secret.as_deref(), Some("s3cret"));
    }

    #[test]
    fn a_blank_client_id_reads_as_absent() {
        let merged = configured(None).merged(&update(true, Some("   "), None));
        assert!(merged.client_id.is_none(), "whitespace is not an id");
        let trimmed = configured(None).merged(&update(true, Some(" abc "), None));
        assert_eq!(trimmed.client_id.as_deref(), Some("abc"));
    }

    #[test]
    fn the_admin_view_reports_the_secret_without_carrying_it() {
        let view = configured(Some("s3cret")).to_view();
        assert!(view.has_secret);
        assert_eq!(view.client_id.as_deref(), Some("id"));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
    }

    /// A provider with no credentials must not be offered: the login page
    /// would send people into a redirect that cannot work.
    #[test]
    fn a_provider_counts_as_available_only_once_it_is_usable() {
        let with_github = |github: MethodConfig| AuthSettings {
            github,
            ..AuthSettings::default()
        };

        assert!(with_github(configured(Some("s"))).methods().github);
        assert!(
            !with_github(configured(None)).methods().github,
            "enabled but no secret"
        );
        assert!(
            !with_github(MethodConfig {
                enabled: false,
                ..configured(Some("s"))
            })
            .methods()
            .github,
            "fully configured but off"
        );

        // Password needs no credentials, so `enabled` is the whole story.
        assert!(AuthSettings::default().methods().password);
    }
}
