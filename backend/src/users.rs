use futures::TryStreamExt;
use mongodb::Database;
use mongodb::bson::{Document, doc};
use serde::{Deserialize, Serialize};
use shared::{AdminUserInfo, UpdateProfileRequest, UserInfo};

use crate::query::UserListParams;

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
            has_password: self.hash_passwd.is_some(),
        }
    }
}

/// True for the unique-index violation Mongo raises when two accounts would end
/// up sharing a username. Caught so a rename race reads as 409 rather than 500.
fn is_duplicate_key(err: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    match &*err.kind {
        ErrorKind::Write(WriteFailure::WriteError(e)) => e.code == 11000,
        _ => false,
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

/// Stores an **already hashed** password. Like [`NewUser`], nothing here
/// hashes — the caller must have run it through
/// [`crate::auth::password::hash`].
pub async fn set_password(db: &Database, id: &str, hash_passwd: &str) -> Result<(), AppError> {
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$set": {"hash_passwd": hash_passwd}})
        .await?;
    Ok(())
}

/// Partial profile update: `None` leaves a field untouched, so a caller can
/// change their display name without clearing their avatar. `username` is
/// expected to be validated by the caller; the unique index is what actually
/// keeps it unique, and a collision surfaces here as a 409.
pub async fn update_profile(
    db: &Database,
    id: &str,
    update: UpdateProfileRequest,
) -> Result<(), AppError> {
    let mut set = Document::new();
    for (field, value) in [
        ("username", update.username),
        ("display_name", update.display_name),
        ("email", update.email),
        ("avatar_url", update.avatar_url),
    ] {
        if let Some(value) = value {
            set.insert(field, value);
        }
    }
    if set.is_empty() {
        return Ok(());
    }
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$set": set})
        .await
        .map_err(|err| {
            if is_duplicate_key(&err) {
                AppError::conflict("that username is already taken")
            } else {
                err.into()
            }
        })?;
    Ok(())
}

pub async fn bump_token_version(db: &Database, id: &str) -> Result<(), AppError> {
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$inc": {"token_version": 1}})
        .await?;
    Ok(())
}

/// One page of accounts for the admin list.
///
/// An aggregate rather than a find, so the link count comes from the database
/// instead of N+1 round trips, and so `hash_passwd` and the provider ids are
/// dropped before the documents ever reach this process.
///
/// `$match` comes first so a search narrows the set before it is paged, and so
/// the `$lookup` only joins the rows on the page.
pub async fn list(db: &Database, params: &UserListParams) -> Result<Vec<AdminUserInfo>, AppError> {
    let pipeline = vec![
        doc! {"$match": params.filter()},
        doc! {"$sort": params.sort.clone()},
        doc! {"$skip": params.skip},
        doc! {"$limit": params.limit},
        doc! {"$lookup": {
            "from": "urls",
            "localField": "id",
            "foreignField": "owner",
            "as": "owned",
        }},
        doc! {"$set": {
            "url_count": {"$size": "$owned"},
            "has_password": {"$eq": [{"$type": "$hash_passwd"}, "string"]},
            // A BSON date cannot deserialize into the DTO's Option<String>.
            "created_at": crate::db::as_iso_string("created_at"),
            // Says *that* a provider is linked without exposing the id.
            // Spelled out per provider because `$getField` demands a constant
            // field name — a computed one is rejected outright.
            "providers": {"$concatArrays": [
                {"$cond": [{"$eq": [{"$type": "$github_id"}, "string"]}, ["github"], []]},
                {"$cond": [{"$eq": [{"$type": "$google_id"}, "string"]}, ["google"], []]},
                {"$cond": [{"$eq": [{"$type": "$facebook_id"}, "string"]}, ["facebook"], []]},
            ]},
        }},
        doc! {"$unset": [
            "_id", "owned", "hash_passwd", "token_version",
            "github_id", "google_id", "facebook_id",
        ]},
    ];
    let rows: Vec<Document> = db
        .collection::<Document>("users")
        .aggregate(pipeline)
        .await?
        .try_collect()
        .await?;
    rows.into_iter()
        .map(|doc| bson::from_document(doc).map_err(AppError::internal))
        .collect()
}

/// Counts what the same filter matches, so a searched page can report totals
/// rather than claiming the whole collection.
pub async fn count(db: &Database, filter: Document) -> Result<u64, AppError> {
    Ok(collection(db).count_documents(filter).await?)
}
