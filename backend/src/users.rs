use futures::TryStreamExt;
use mongodb::bson::{Document, doc};
use mongodb::{ClientSession, Database};
use serde::{Deserialize, Serialize};
use shared::{AdminUserInfo, LinkDisposition, UpdateProfileRequest, UserInfo};

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
    /// Id of the admin who granted the flag — the only thing the admin chain is
    /// stored as. Depth and ancestry are both derived from following it, so
    /// moving a branch cannot leave a stale rank behind.
    ///
    /// `None` on an admin flipped on directly in the database, which is exactly
    /// how the first one is made. Such an account is a root: nobody is above
    /// it, so nobody can touch it through the API.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// Bumped on logout and password change to revoke outstanding tokens.
    /// i64 because Mongo's `$inc` silently promotes an int32 past its max into
    /// an int64, which would then no longer deserialize into the struct.
    #[serde(default)]
    pub token_version: i64,
    /// Provider identities, each with its own partial unique index. `None` when
    /// that provider was never connected.
    ///
    /// `skip_serializing_if` matters: [`create`] serialises this struct straight
    /// into Mongo, and three explicit nulls would collide on those indexes the
    /// second time an account is made without providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facebook_id: Option<String>,
}

impl User {
    /// Connected providers, always in the same order — the account page renders
    /// a row per provider from this. The ids themselves stay here: knowing
    /// somebody's GitHub id is a step towards finding their account.
    pub fn providers(&self) -> Vec<String> {
        crate::oauth::ProviderKind::ALL
            .into_iter()
            .filter(|kind| match kind {
                crate::oauth::ProviderKind::Github => self.github_id.is_some(),
                crate::oauth::ProviderKind::Google => self.google_id.is_some(),
                crate::oauth::ProviderKind::Facebook => self.facebook_id.is_some(),
            })
            .map(|kind| kind.as_str().to_string())
            .collect()
    }

    pub fn to_info(&self) -> UserInfo {
        UserInfo {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            email: self.email.clone(),
            avatar_url: self.avatar_url.clone(),
            is_admin: self.is_admin,
            is_root: self.is_admin && self.promoted_by.is_none(),
            has_password: self.hash_passwd.is_some(),
            providers: self.providers(),
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

/// [`find_by_id`] inside a session, so the read joins whatever transaction the
/// session is running and sees the same snapshot as the writes around it.
///
/// A separate function rather than a parameter on `find_by_id`: that one is
/// called by the auth extractor on every request, and threading a session
/// through the busiest path in the app to serve the rarest one is a poor
/// trade.
pub async fn find_by_id_in(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
) -> Result<Option<User>, AppError> {
    match collection(db)
        .find_one(doc! {"id": id})
        .session(&mut *session)
        .await?
    {
        Some(doc) => Ok(Some(bson::from_document(doc).map_err(AppError::internal)?)),
        None => Ok(None),
    }
}

/// The account holding this provider identity, if any. `kind.field()` is the
/// only source of the field name — it never comes from a request.
pub async fn find_by_provider(
    db: &Database,
    kind: crate::oauth::ProviderKind,
    provider_id: &str,
) -> Result<Option<User>, AppError> {
    find_one(db, doc! {kind.field(): provider_id}).await
}

/// The single account holding this address, compared case-insensitively.
///
/// `None` when nobody holds it *and* when more than one account does: email is
/// not unique in this collection, and picking one of several would decide an
/// account takeover by document order.
pub async fn find_by_email(db: &Database, email: &str) -> Result<Option<User>, AppError> {
    let lowered = email.trim().to_lowercase();
    if lowered.is_empty() {
        return Ok(None);
    }
    // Escaped, or an address containing regex syntax would match addresses it
    // does not equal — and the match is what decides who gets signed in.
    let pattern = format!("^{}$", regex::escape(&lowered));
    let mut matches: Vec<Document> = collection(db)
        .find(doc! {"email": {"$regex": pattern, "$options": "i"}})
        // Two is enough to know it is not one.
        .limit(2)
        .await?
        .try_collect()
        .await?;
    if matches.len() == 1 {
        Ok(Some(
            bson::from_document(matches.remove(0)).map_err(AppError::internal)?,
        ))
    } else {
        Ok(None)
    }
}

/// Attaches a provider identity to an account.
///
/// A 409 when the identity is already on another account: the unique index on
/// the provider field is what settles a race between two callbacks claiming it,
/// and the loser has to be told the same thing it would have been told had it
/// arrived a moment later.
pub async fn link_provider(
    db: &Database,
    id: &str,
    kind: crate::oauth::ProviderKind,
    provider_id: &str,
) -> Result<(), AppError> {
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$set": {kind.field(): provider_id}})
        .await
        .map_err(|err| {
            if is_duplicate_key(&err) {
                AppError::conflict("that account is already linked to another user")
            } else {
                err.into()
            }
        })?;
    Ok(())
}

pub async fn unlink_provider(
    db: &Database,
    id: &str,
    kind: crate::oauth::ProviderKind,
) -> Result<(), AppError> {
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$unset": {kind.field(): ""}})
        .await?;
    Ok(())
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

    /// An account born from a provider profile. No password: these accounts
    /// sign in through the provider until their owner sets one.
    pub fn from_profile(username: String, profile: &crate::oauth::Profile) -> Self {
        Self {
            username,
            hash_passwd: None,
            display_name: profile.display_name.clone(),
            email: profile.email.clone(),
            avatar_url: profile.avatar_url.clone(),
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
        promoted_by: None,
        token_version: 0,
        // Left off the document entirely rather than written as nulls — see the
        // `skip_serializing_if` on the fields. The OAuth callback links the
        // identity in a second write.
        github_id: None,
        google_id: None,
        facebook_id: None,
    };
    let mut doc = bson::to_document(&user).map_err(AppError::internal)?;
    doc.insert("created_at", bson::DateTime::now());
    collection(db).insert_one(doc).await?;
    Ok(user)
}

/// Stores an **already hashed** password and revokes every outstanding token.
/// Like [`NewUser`], nothing here hashes — the caller must have run it through
/// [`crate::auth::password::hash`].
///
/// One update, not a store followed by a [`bump_token_version`]. Between two
/// there is a moment where the new password is live and the old sessions are
/// too, which is exactly the window somebody changing their password because it
/// leaked is trying to close. A dropped connection in that gap would leave it
/// open indefinitely, and there are no transactions here to roll it back.
pub async fn set_password(db: &Database, id: &str, hash_passwd: &str) -> Result<(), AppError> {
    collection(db)
        .update_one(
            doc! {"id": id},
            doc! {
                "$set": {"hash_passwd": hash_passwd},
                "$inc": {"token_version": 1},
            },
        )
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

/// How far the chain is followed before giving up. Deep enough that no real
/// hierarchy hits it, shallow enough that a cycle introduced by hand-editing
/// the collection cannot turn a lookup into a long walk.
const MAX_CHAIN_DEPTH: i32 = 32;

/// Hangs `id` under `parent` and gives it the flag.
///
/// The same write covers both promoting a fresh account and moving an existing
/// admin, because they are the same fact: this is who vouches for them now.
/// Whatever hangs below `id` moves with it, since the subtree is described by
/// pointers to `id` rather than by a stored depth.
pub async fn grant_admin(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
    parent_id: &str,
) -> Result<(), AppError> {
    collection(db)
        .update_one(
            doc! {"id": id},
            doc! {"$set": {"is_admin": true, "promoted_by": parent_id}},
        )
        .session(&mut *session)
        .await?;
    Ok(())
}

/// Drops the flag from `id` **and from everyone below them**, returning how
/// many accounts lost it.
///
/// The cascade is the point: an admin only holds the flag because the person
/// above them vouched, so withdrawing that vouching withdraws what it granted.
/// Leaving the subtree in place would instead leave admins hanging off an
/// ordinary account.
pub async fn revoke_admin(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
) -> Result<u64, AppError> {
    let mut ids = descendant_ids(db, &mut *session, id).await?;
    ids.push(id.to_string());
    let result = collection(db)
        .update_many(
            doc! {"id": {"$in": &ids}},
            doc! {
                "$set": {"is_admin": false},
                "$unset": {"promoted_by": ""},
            },
        )
        .session(&mut *session)
        .await?;
    Ok(result.modified_count)
}

/// Hands the admins `id` promoted to whoever promoted `id`, or makes them roots
/// of their own when nobody was above it. Returns how many moved.
///
/// The alternative to [`revoke_admin`] when an admin account is deleted: the
/// branch keeps its standing rather than losing it, one level shallower.
pub async fn reparent_children(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
    parent: Option<&str>,
) -> Result<u64, AppError> {
    let update = match parent {
        Some(parent) => doc! {"$set": {"promoted_by": parent}},
        // A root's children become roots: nobody is above them, so nobody but
        // the database can touch them — the same standing the deleted account
        // itself had.
        None => doc! {"$unset": {"promoted_by": ""}},
    };
    Ok(collection(db)
        .update_many(doc! {"promoted_by": id}, update)
        .session(&mut *session)
        .await?
        .modified_count)
}

/// What deleting an account did to the links it owned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkOutcome {
    /// Left in place, unowned, on a deadline.
    pub orphaned: u64,
    /// Removed with the account.
    pub deleted: u64,
}

/// Deletes the account and disposes of its links.
///
/// [`LinkDisposition::Orphan`] does not delete them: someone may still be
/// following them. They become unowned — so anyone can re-claim or edit them —
/// and expire in `grace_days` unless someone does. An existing earlier expiry
/// is kept, so this can only bring a deadline forward, never push one out.
///
/// The orphan branch runs as an aggregation-pipeline update because `$unset`
/// and a computed `$min` cannot both be expressed in a plain update document.
pub async fn delete_with_cascade(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
    links: LinkDisposition,
    grace_days: i64,
) -> Result<LinkOutcome, AppError> {
    let urls = db.collection::<Document>("urls");
    let outcome = match links {
        LinkDisposition::Delete => LinkOutcome {
            deleted: urls
                .delete_many(doc! {"owner": id})
                .session(&mut *session)
                .await?
                .deleted_count,
            orphaned: 0,
        },
        LinkDisposition::Orphan => {
            let cutoff = bson::DateTime::from_millis(
                bson::DateTime::now().timestamp_millis() + grace_days * 24 * 60 * 60 * 1000,
            );
            LinkOutcome {
                orphaned: urls
                    .update_many(
                        doc! {"owner": id},
                        vec![
                            doc! {"$set": {
                                "expires_at": {"$min": [{"$ifNull": ["$expires_at", cutoff]}, cutoff]},
                            }},
                            doc! {"$unset": "owner"},
                        ],
                    )
                    .session(&mut *session)
                    .await?
                    .modified_count,
                deleted: 0,
            }
        }
    };
    collection(db)
        .delete_one(doc! {"id": id})
        .session(&mut *session)
        .await?;
    Ok(outcome)
}

/// Ids of every account above `id` in the chain. Membership is what callers
/// want — whether the actor is one of them — so the order is not defined.
pub async fn ancestor_ids(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
) -> Result<Vec<String>, AppError> {
    chain_ids(db, session, id, "$promoted_by", "promoted_by", "id").await
}

/// Ids of every account below `id`: the ones it promoted, and so on down.
pub async fn descendant_ids(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
) -> Result<Vec<String>, AppError> {
    chain_ids(db, session, id, "$id", "id", "promoted_by").await
}

/// Walks `promoted_by` in one direction or the other.
///
/// `$graphLookup` rather than a loop of `find_one`s: one round trip instead of
/// one per level, and it tracks what it has already visited, so a cycle left by
/// a hand-edited document terminates instead of hanging.
async fn chain_ids(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
    start_with: &str,
    connect_from: &str,
    connect_to: &str,
) -> Result<Vec<String>, AppError> {
    let pipeline = vec![
        doc! {"$match": {"id": id}},
        doc! {"$graphLookup": {
            "from": "users",
            "startWith": start_with,
            "connectFromField": connect_from,
            "connectToField": connect_to,
            "as": "chain",
            "maxDepth": MAX_CHAIN_DEPTH,
        }},
        doc! {"$project": {"_id": 0, "ids": "$chain.id"}},
    ];
    // A cursor opened with a session is driven by that session rather than by
    // `TryStreamExt`, so the rows are pulled by hand.
    let mut cursor = collection(db)
        .aggregate(pipeline)
        .session(&mut *session)
        .await?;
    let Some(row) = cursor.next(&mut *session).await.transpose()? else {
        return Ok(Vec::new());
    };
    Ok(row
        .get_array("ids")
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

pub async fn bump_token_version(db: &Database, id: &str) -> Result<(), AppError> {
    collection(db)
        .update_one(doc! {"id": id}, doc! {"$inc": {"token_version": 1}})
        .await?;
    Ok(())
}

/// Joins the link count on.
///
/// The inner pipeline counts inside the database rather than returning the
/// documents to be counted here: without it, listing an account that owns ten
/// thousand links would pull all ten thousand across just to call `$size` on
/// them.
fn link_count_stages() -> Vec<Document> {
    vec![
        doc! {"$lookup": {
            "from": "urls",
            "localField": "id",
            "foreignField": "owner",
            "as": "owned",
            "pipeline": [{"$count": "n"}],
        }},
        doc! {"$set": {"url_count": {"$ifNull": [{"$first": "$owned.n"}, 0]}}},
    ]
}

/// The columns every listed account carries, whichever list it came from.
///
/// The shape that drops `hash_passwd` and the provider ids before the documents
/// ever reach this process.
fn presentation_stages() -> Vec<Document> {
    vec![
        doc! {"$set": {
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
            "_id", "owned", "ancestors", "hash_passwd", "token_version",
            "github_id", "google_id", "facebook_id",
        ]},
    ]
}

async fn collect(db: &Database, pipeline: Vec<Document>) -> Result<Vec<AdminUserInfo>, AppError> {
    let rows: Vec<Document> = collection(db)
        .aggregate(pipeline)
        .await?
        .try_collect()
        .await?;
    rows.into_iter()
        .map(|doc| bson::from_document(doc).map_err(AppError::internal))
        .collect()
}

/// One page of the accounts that are *not* admins.
///
/// Admins are excluded because they come back whole from [`admins`] instead:
/// the page they would land on has nothing to do with where they sit in the
/// tree, and paging the tree would cut branches.
///
/// `$match` comes first so a search narrows the set before anything else runs.
/// After that the order of the join and the paging depends on what the sort
/// asks for: normally the twenty rows on the page are joined, but ordering *by*
/// the link count cannot know which twenty those are until every account has
/// been counted.
pub async fn list(db: &Database, params: &UserListParams) -> Result<Vec<AdminUserInfo>, AppError> {
    let mut pipeline = vec![doc! {"$match": params.filter()}];
    let paging = [
        doc! {"$sort": params.sort.clone()},
        doc! {"$skip": params.skip},
        doc! {"$limit": params.limit},
    ];
    if params.sorts_by_join() {
        pipeline.extend(link_count_stages());
        pipeline.extend(paging);
    } else {
        pipeline.extend(paging);
        pipeline.extend(link_count_stages());
    }
    pipeline.extend(presentation_stages());
    // Nothing outside the tree has a rank, and old documents may still carry a
    // stored one from before the level was derived.
    pipeline.push(doc! {"$unset": ["admin_level", "promoted_by"]});
    collect(db, pipeline).await
}

/// Never paged, so the tree cannot lose an interior node. Admins are a bounded
/// set — they exist only by invitation — but the limit is here so a runaway
/// cannot return the whole collection.
const MAX_ADMINS: i64 = 1000;

/// Every admin, with the depth of each one derived from the chain above it.
///
/// Deliberately not searched: hiding an admin whose name does not match would
/// orphan the admins below them, so filtering is the client's job once it has
/// the whole shape. The sort *is* honoured, but as the order siblings appear
/// in — the tree's own structure decides everything above that.
pub async fn admins(
    db: &Database,
    params: &UserListParams,
) -> Result<Vec<AdminUserInfo>, AppError> {
    let mut pipeline = vec![
        doc! {"$match": {"is_admin": true}},
        // Everyone above this account, which is both the depth and — for the
        // permission check elsewhere — who is allowed to touch them.
        doc! {"$graphLookup": {
            "from": "users",
            "startWith": "$promoted_by",
            "connectFromField": "promoted_by",
            "connectToField": "id",
            "as": "ancestors",
            "maxDepth": MAX_CHAIN_DEPTH,
        }},
        doc! {"$set": {"admin_level": {"$size": "$ancestors"}}},
    ];
    // Unconditionally before the sort: this list is never paged, so ordering by
    // the link count costs nothing extra here.
    pipeline.extend(link_count_stages());
    pipeline.push(doc! {"$sort": params.sort.clone()});
    pipeline.push(doc! {"$limit": MAX_ADMINS});
    pipeline.extend(presentation_stages());
    collect(db, pipeline).await
}

/// Counts what the same filter matches, so a searched page can report totals
/// rather than claiming the whole collection.
pub async fn count(db: &Database, filter: Document) -> Result<u64, AppError> {
    Ok(collection(db).count_documents(filter).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_info_lists_every_connected_provider_in_a_stable_order() {
        let mut user = User {
            id: "u1".into(),
            username: "someone".into(),
            display_name: None,
            email: None,
            avatar_url: None,
            hash_passwd: None,
            is_admin: false,
            promoted_by: None,
            token_version: 0,
            github_id: None,
            google_id: None,
            facebook_id: None,
        };
        assert!(user.to_info().providers.is_empty());

        user.google_id = Some("g".into());
        user.github_id = Some("gh".into());
        // Always the same order, whatever order they were connected in: the
        // account page renders one row per provider from this list.
        assert_eq!(user.to_info().providers, vec!["github", "google"]);
    }

    /// The ids themselves are not the account page's business — knowing that
    /// somebody's GitHub id is 1234 is a step towards finding their account.
    #[test]
    fn the_info_never_carries_the_provider_ids_themselves() {
        let user = User {
            id: "u1".into(),
            username: "someone".into(),
            display_name: None,
            email: None,
            avatar_url: None,
            hash_passwd: Some("$argon2id$secret".into()),
            is_admin: false,
            promoted_by: None,
            token_version: 0,
            github_id: Some("gh-1234".into()),
            google_id: None,
            facebook_id: None,
        };
        let json = serde_json::to_string(&user.to_info()).unwrap();
        assert!(!json.contains("gh-1234"), "{json}");
        assert!(!json.contains("argon2"), "{json}");
        assert!(json.contains("github"), "but it says the provider is on");
    }
}
