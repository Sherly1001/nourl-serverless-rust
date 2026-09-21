use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::bson::{Document, doc};
use mongodb::{ClientSession, Database};
use serde::{Deserialize, Serialize};
use shared::{AdminUserInfo, LinkDisposition, UpdateProfileRequest, UserInfo};

use crate::chain::{Forest, Plan};
use crate::query::UserListParams;

use crate::error::AppError;

/// A row of the `users` collection. `id`, not the Mongo `_id`, is what
/// `urls.owner` joins against, for parity with the old app's documents.
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
    /// All the chain is stored as; depth and ancestry are derived by following
    /// it. `None` for an admin flipped on in the database — a root, which
    /// nothing above it can touch through the API.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// Bumped on logout and password change to revoke outstanding tokens. i64
    /// because `$inc` silently promotes past int32 and would stop deserializing.
    #[serde(default)]
    pub token_version: i64,
    /// Each has a partial unique index, and [`create`] serialises this struct
    /// straight into Mongo — so `skip_serializing_if` matters: explicit nulls
    /// would collide on the second account made without providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facebook_id: Option<String>,
}

impl User {
    /// Always in the same order. The ids stay here: knowing somebody's GitHub
    /// id is a step towards finding their account.
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

/// The duplicate-username index violation, so a rename race reads 409 not 500.
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

/// [`find_by_id`] inside a session, seeing the same snapshot as the writes
/// around it. Separate rather than a parameter: the plain one is called by the
/// auth extractor on every request and should not carry a session for this.
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

/// The single account holding this address, case-insensitively. `None` when
/// nobody holds it and when several do: picking one of several would decide an
/// account takeover by document order.
pub async fn find_by_email(db: &Database, email: &str) -> Result<Option<User>, AppError> {
    let lowered = email.trim().to_lowercase();
    if lowered.is_empty() {
        return Ok(None);
    }
    // Escaped: this match decides who gets signed in.
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

/// A 409 when the identity is already on another account: the unique index
/// settles a race between two callbacks, and the loser is told what it would
/// have been told arriving a moment later.
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

/// Everything a new account can be born with. All optional: an OAuth account
/// has no password, and a password signup has no profile.
#[derive(Debug, Clone, Default)]
pub struct NewUser {
    pub username: String,
    /// A PHC string. This module never hashes.
    pub hash_passwd: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

impl NewUser {
    /// Takes an **already hashed** password from
    /// [`crate::auth::password::hash`]. Passing plaintext stores it in the clear.
    pub fn with_password_hash(username: &str, hash_passwd: String) -> Self {
        Self {
            username: username.to_string(),
            hash_passwd: Some(hash_passwd),
            ..Default::default()
        }
    }

    /// No password: these sign in through the provider until one is set.
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
        // So the UI always has something to render.
        display_name: Some(new.display_name.unwrap_or_else(|| new.username.clone())),
        username: new.username,
        email: new.email,
        avatar_url: new.avatar_url,
        hash_passwd: new.hash_passwd,
        is_admin: false,
        promoted_by: None,
        token_version: 0,
        // Left off the document rather than written as nulls; see the fields.
        github_id: None,
        google_id: None,
        facebook_id: None,
    };
    let mut doc = bson::to_document(&user).map_err(AppError::internal)?;
    doc.insert("created_at", bson::DateTime::now());
    collection(db).insert_one(doc).await?;
    Ok(user)
}

/// Stores an **already hashed** password and revokes outstanding tokens in one
/// update — two would leave a window where the new password and the old
/// sessions are both live, which is the window this is meant to close.
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

/// `None` leaves a field untouched. The caller validates `username`; the
/// unique index keeps it unique, and a collision surfaces here as a 409.
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

/// What deleting an account did to the links it owned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkOutcome {
    /// Left in place, unowned, on a deadline.
    pub orphaned: u64,
    /// Removed with the account.
    pub deleted: u64,
}

/// Deletes the account and disposes of its links. Orphaning leaves them
/// working, unowned, expiring in `grace_days` — an earlier expiry is kept, so
/// a deadline only ever moves closer.
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
        LinkDisposition::Orphan => LinkOutcome {
            orphaned: orphan_links(db, &mut *session, &[id.to_string()], grace_days).await?,
            deleted: 0,
        },
    };
    collection(db)
        .delete_one(doc! {"id": id})
        .session(&mut *session)
        .await?;
    Ok(outcome)
}

/// Leaves the links of `owners` working but unowned, expiring in `grace_days`;
/// an earlier expiry is kept, so a deadline only ever moves closer. A pipeline,
/// because `$unset` and a computed `$min` cannot share a plain update.
async fn orphan_links(
    db: &Database,
    session: &mut ClientSession,
    owners: &[String],
    grace_days: i64,
) -> Result<u64, AppError> {
    let cutoff = bson::DateTime::from_millis(
        bson::DateTime::now().timestamp_millis() + grace_days * 24 * 60 * 60 * 1000,
    );
    Ok(db
        .collection::<Document>("urls")
        .update_many(
            doc! {"owner": {"$in": owners}},
            vec![
                doc! {"$set": {
                    "expires_at": {"$min": [{"$ifNull": ["$expires_at", cutoff]}, cutoff]},
                }},
                doc! {"$unset": "owner"},
            ],
        )
        .session(&mut *session)
        .await?
        .modified_count)
}

/// Every admin as a pointer to whoever vouches for them, in one query. Refused
/// past [`MAX_ADMINS`]: a tree that size is a corrupt one, and cascading over a
/// guess at its shape is worse than not acting.
pub async fn admin_forest(db: &Database, session: &mut ClientSession) -> Result<Forest, AppError> {
    let mut cursor = collection(db)
        .find(doc! {"is_admin": true})
        .projection(doc! {"_id": 0, "id": 1, "promoted_by": 1})
        .limit(MAX_ADMINS + 1)
        .session(&mut *session)
        .await?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        let Ok(id) = row.get_str("id") else { continue };
        rows.push((
            id.to_string(),
            row.get_str("promoted_by").ok().map(str::to_string),
        ));
    }
    if rows.len() as i64 > MAX_ADMINS {
        return Err(AppError::internal(format!(
            "more than {MAX_ADMINS} admins, so the chain cannot be worked out"
        )));
    }
    Ok(Forest::new(rows))
}

/// Carries out a [`Plan`], orphaning the links of whatever it deletes. The
/// writes are grouped by the plan, so the count follows from the shape of the
/// chain rather than from how many accounts were named.
pub async fn apply(
    db: &Database,
    session: &mut ClientSession,
    plan: &Plan,
    grace_days: i64,
) -> Result<LinkOutcome, AppError> {
    // Before the demotion, which unsets the pointers they climb.
    for (parent, ids) in &plan.reparented {
        let update = match parent {
            Some(parent) => doc! {"$set": {"promoted_by": parent}},
            // A root's children become roots, as their admin was.
            None => doc! {"$unset": {"promoted_by": ""}},
        };
        collection(db)
            .update_many(doc! {"id": {"$in": ids}}, update)
            .session(&mut *session)
            .await?;
    }
    if !plan.demoted.is_empty() {
        collection(db)
            .update_many(
                doc! {"id": {"$in": &plan.demoted}},
                doc! {"$set": {"is_admin": false}, "$unset": {"promoted_by": ""}},
            )
            .session(&mut *session)
            .await?;
    }
    for (parent, ids) in &plan.promoted {
        collection(db)
            .update_many(
                doc! {"id": {"$in": ids}},
                doc! {"$set": {"is_admin": true, "promoted_by": parent}},
            )
            .session(&mut *session)
            .await?;
    }
    if plan.deleted.is_empty() {
        return Ok(LinkOutcome::default());
    }
    let orphaned = orphan_links(db, &mut *session, &plan.deleted, grace_days).await?;
    collection(db)
        .delete_many(doc! {"id": {"$in": &plan.deleted}})
        .session(&mut *session)
        .await?;
    Ok(LinkOutcome {
        orphaned,
        deleted: 0,
    })
}

/// The accounts holding `ids`, in one query.
pub async fn find_many_in(
    db: &Database,
    session: &mut ClientSession,
    ids: &[String],
) -> Result<Vec<User>, AppError> {
    let mut cursor = collection(db)
        .find(doc! {"id": {"$in": ids}})
        .session(&mut *session)
        .await?;
    let mut found = Vec::new();
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        found.push(bson::from_document(row).map_err(AppError::internal)?);
    }
    Ok(found)
}

/// Every account above each of `ids`, in one `$graphLookup` rather than the one
/// per id a selection would otherwise cost. Ids with nothing above them are
/// absent rather than empty.
pub async fn ancestors_of_many(
    db: &Database,
    session: &mut ClientSession,
    ids: &[String],
) -> Result<HashMap<String, Vec<String>>, AppError> {
    let pipeline = vec![
        doc! {"$match": {"id": {"$in": ids}}},
        doc! {"$graphLookup": {
            "from": "users",
            "startWith": "$promoted_by",
            "connectFromField": "promoted_by",
            "connectToField": "id",
            "as": "chain",
            "maxDepth": crate::chain::MAX_CHAIN_DEPTH as i32,
        }},
        doc! {"$project": {"_id": 0, "id": 1, "ids": "$chain.id"}},
    ];
    let mut cursor = collection(db)
        .aggregate(pipeline)
        .session(&mut *session)
        .await?;
    let mut chains = HashMap::new();
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        let Ok(id) = row.get_str("id") else { continue };
        let above = row
            .get_array("ids")
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        chains.insert(id.to_string(), above);
    }
    Ok(chains)
}

/// Whether `actor` may act on `target`: the chain above `target` must pass
/// through `actor`, so nobody reaches sideways into a peer's branch. An account
/// with no flag is in no subtree, which is the only way the tree can grow.
pub async fn may_manage(
    db: &Database,
    session: &mut ClientSession,
    actor: &User,
    target: &User,
) -> Result<bool, AppError> {
    if !target.is_admin {
        return Ok(true);
    }
    Ok(ancestor_ids(db, session, &target.id)
        .await?
        .contains(&actor.id))
}

/// Ids of every account above `id`. Callers want membership, not order.
pub async fn ancestor_ids(
    db: &Database,
    session: &mut ClientSession,
    id: &str,
) -> Result<Vec<String>, AppError> {
    chain_ids(db, session, id, "$promoted_by", "promoted_by", "id").await
}

/// Walks `promoted_by` upwards. `$graphLookup`, not a loop of `find_one`s: one
/// round trip, and it tracks visits, so a hand-edited cycle terminates.
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
            "maxDepth": crate::chain::MAX_CHAIN_DEPTH as i32,
        }},
        doc! {"$project": {"_id": 0, "ids": "$chain.id"}},
    ];
    // A session cursor is driven by the session, not by `TryStreamExt`.
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

/// Joins the link count on, counting inside the database: otherwise listing an
/// account with ten thousand links would pull all ten thousand across.
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

/// The columns every listed account carries, dropping `hash_passwd` and the
/// provider ids before the documents reach this process.
fn presentation_stages() -> Vec<Document> {
    vec![
        doc! {"$set": {
            "has_password": {"$eq": [{"$type": "$hash_passwd"}, "string"]},
            // A BSON date cannot deserialize into the DTO's Option<String>.
            "created_at": crate::db::as_iso_string("created_at"),
            // Per provider, because `$getField` rejects a computed field name.
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

/// One page of the non-admins; admins come back whole from [`admins`], since
/// paging a tree cuts branches. Joined after paging, unless the sort is *by*
/// the join, which cannot know the page until everything is counted.
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
    // Nothing outside the tree has a rank, and old documents may carry one.
    pipeline.push(doc! {"$unset": ["admin_level", "promoted_by"]});
    collect(db, pipeline).await
}

/// Never paged, so the tree keeps its interior nodes; the limit is only a
/// backstop against a runaway.
const MAX_ADMINS: i64 = 1000;

/// Every admin, depth derived from the chain above it. Not searched: hiding
/// one would orphan those below, so the client filters once it has the shape.
/// The sort decides sibling order only; the tree decides the rest.
pub async fn admins(
    db: &Database,
    params: &UserListParams,
) -> Result<Vec<AdminUserInfo>, AppError> {
    let mut pipeline = vec![
        doc! {"$match": {"is_admin": true}},
        // Everyone above: both the depth and who may touch them.
        doc! {"$graphLookup": {
            "from": "users",
            "startWith": "$promoted_by",
            "connectFromField": "promoted_by",
            "connectToField": "id",
            "as": "ancestors",
            "maxDepth": crate::chain::MAX_CHAIN_DEPTH as i32,
        }},
        doc! {"$set": {"admin_level": {"$size": "$ancestors"}}},
    ];
    // Before the sort unconditionally: never paged, so it costs nothing.
    pipeline.extend(link_count_stages());
    pipeline.push(doc! {"$sort": params.sort.clone()});
    pipeline.push(doc! {"$limit": MAX_ADMINS});
    pipeline.extend(presentation_stages());
    collect(db, pipeline).await
}

/// What the same filter matches, so a searched page reports its own total.
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
        // Always the same order, whatever order they were connected in.
        assert_eq!(user.to_info().providers, vec!["github", "google"]);
    }

    /// Knowing somebody's GitHub id is a step towards finding their account.
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
