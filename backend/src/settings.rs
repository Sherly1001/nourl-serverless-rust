use mongodb::Database;
use mongodb::bson::{Document, doc};
use serde::{Deserialize, Serialize};
use shared::AuthMethods;

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

/// The single `settings` document (`_id: "auth"`). Every field defaults, so a
/// fresh database needs no seeding — and a partially filled document (say,
/// github configured but google never touched) still loads.
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
    /// The public view: which methods a login page should offer. An OAuth
    /// provider counts as enabled only once it also has credentials —
    /// advertising one without them would send users into a broken redirect.
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
