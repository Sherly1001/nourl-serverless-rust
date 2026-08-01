use mongodb::Database;
use mongodb::bson::{Document, doc};
use serde::{Deserialize, Serialize};
use shared::UserInfo;

use crate::error::AppError;

/// A row of the `users` collection. `id` is the string that `urls.owner`
/// joins against — deliberately not the Mongo `_id`, for parity with the
/// documents the old app wrote.
///
/// `username` is required: the pre-Rust accounts, which keyed on email and had
/// no username at all, were deleted rather than migrated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub hash_passwd: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    /// Bumped on logout and password change to revoke outstanding tokens.
    /// i64 because Mongo's `$inc` silently promotes an int32 past its max into
    /// an int64, which would then no longer deserialize into the struct.
    #[serde(default)]
    pub token_version: i64,
}

impl User {
    pub fn to_info(&self) -> UserInfo {
        UserInfo {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            email: self.email.clone(),
            avatar_url: self.avatar_url.clone(),
            is_admin: self.is_admin,
        }
    }
}

fn collection(db: &Database) -> mongodb::Collection<Document> {
    db.collection::<Document>("users")
}

async fn find_one(db: &Database, filter: Document) -> Result<Option<User>, AppError> {
    match collection(db).find_one(filter).await? {
        Some(doc) => Ok(Some(bson::from_document(doc).map_err(AppError::internal)?)),
        None => Ok(None),
    }
}

pub async fn find_by_username(db: &Database, username: &str) -> Result<Option<User>, AppError> {
    find_one(db, doc! {"username": username}).await
}

pub async fn find_by_id(db: &Database, id: &str) -> Result<Option<User>, AppError> {
    find_one(db, doc! {"id": id}).await
}

/// Everything a new account can be born with. `hash_passwd` is optional
/// because phase 2b creates OAuth-only accounts that never have a password,
/// and the profile fields are optional because password signups supply none of
/// them while an OAuth provider supplies all three.
#[derive(Debug, Clone, Default)]
pub struct NewUser {
    pub username: String,
    /// Already-hashed password (a PHC string). This module never hashes.
    pub hash_passwd: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

impl NewUser {
    /// A username/password signup, with no profile data yet.
    ///
    /// Takes an **already hashed** password — the PHC string from
    /// [`crate::auth::password::hash`]. Nothing in this module hashes anything,
    /// so passing a plaintext password here stores it in the clear.
    pub fn with_password_hash(username: &str, hash_passwd: String) -> Self {
        Self {
            username: username.to_string(),
            hash_passwd: Some(hash_passwd),
            ..Default::default()
        }
    }
}

pub async fn create(db: &Database, new: NewUser) -> Result<User, AppError> {
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        // Falls back to the username so the UI always has something to render;
        // the account owner can change it via `PUT /api/auth/me`.
        display_name: Some(new.display_name.unwrap_or_else(|| new.username.clone())),
        username: new.username,
        email: new.email,
        avatar_url: new.avatar_url,
        hash_passwd: new.hash_passwd,
        is_admin: false,
        token_version: 0,
    };
    let mut doc = bson::to_document(&user).map_err(AppError::internal)?;
    doc.insert("created_at", bson::DateTime::now());
    collection(db).insert_one(doc).await?;
    Ok(user)
}

/// Partial profile update: `None` leaves a field untouched, so a caller can
/// change their display name without clearing their avatar.
pub async fn update_profile(
    db: &Database,
    id: &str,
    display_name: Option<String>,
    email: Option<String>,
    avatar_url: Option<String>,
) -> Result<(), AppError> {
    let mut set = Document::new();
    if let Some(value) = display_name {
        set.insert("display_name", value);
    }
    if let Some(value) = email {
        set.insert("email", value);
    }
    if let Some(value) = avatar_url {
        set.insert("avatar_url", value);
    }
    if set.is_empty() {
        return Ok(());
    }
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$set": set})
        .await?;
    Ok(())
}

pub async fn bump_token_version(db: &Database, id: &str) -> Result<(), AppError> {
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$inc": {"token_version": 1}})
        .await?;
    Ok(())
}
