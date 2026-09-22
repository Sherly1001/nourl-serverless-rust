use axum::Json;
use axum::extract::{Path, State};
use axum_extra::extract::Query;
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use shared::{
    BulkUrlsRequest, BulkUrlsResponse, DeleteResponse, RejectedId, UrlBulkAction, UrlEntry,
    UrlListResponse, UrlUpsertRequest, validate_code, validate_url,
};

use crate::app::AppState;
use crate::auth::extract::{AdminUser, CurrentUser, OptionalUser};
use crate::db::url_aggregate_pipeline;
use crate::error::AppError;
use crate::extract::AppJson;
use crate::query::{ListParams, QueryPairs, UrlFilters};
use crate::users::{self, User};

/// Three answers, not two: `Option<String>` cannot tell "not talking about the
/// expiry" from "remove it", and the row editor sends every field every save.
#[derive(Debug)]
enum Expiry {
    /// The field was absent: whatever is stored stays.
    Leave,
    /// An explicit empty string: the link stops expiring.
    Clear,
    At(bson::DateTime),
}

fn parse_expiry(raw: Option<&str>) -> Result<Expiry, AppError> {
    let Some(raw) = raw else {
        return Ok(Expiry::Leave);
    };
    if raw.trim().is_empty() {
        return Ok(Expiry::Clear);
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(raw).map_err(|_| {
        AppError::validation("expires_at must be an RFC3339 datetime").on_field("expires_at")
    })?;
    let millis = parsed.timestamp_millis();
    if millis <= bson::DateTime::now().timestamp_millis() {
        return Err(AppError::validation("expires_at must be in the future").on_field("expires_at"));
    }
    Ok(Expiry::At(bson::DateTime::from_millis(millis)))
}

/// Whether `user` may take `existing` off whoever has it. Unowned is anyone's;
/// an owned link follows [`crate::users::may_manage`], so an admin reaches only
/// down their own branch. An owner id pointing nowhere reads as unowned.
async fn may_claim(
    state: &AppState,
    session: &mut mongodb::ClientSession,
    user: &User,
    existing: &Document,
) -> Result<bool, AppError> {
    if !user.is_admin {
        return Ok(false);
    }
    let Some(owner) = owner_id(existing) else {
        return Ok(true);
    };
    let Some(owner) = users::find_by_id_in(&state.db, &mut *session, owner).await? else {
        return Ok(true);
    };
    users::may_manage(&state.db, session, user, &owner).await
}

/// A legacy `owner: null` means unowned, as an absent field does.
fn owner_id(doc: &Document) -> Option<&str> {
    match doc.get("owner") {
        Some(Bson::String(id)) => Some(id.as_str()),
        _ => None,
    }
}

/// Stays generic: naming the url would make this a lookup service.
fn owned_error(code: &str) -> AppError {
    AppError::forbidden(format!(
        "code '{code}' is already taken by a registered user"
    ))
    .on_field("code")
}

/// A taken code, reached only after [`may_write`] passes — which is what keeps
/// the attached link out of an answer the caller may not see. The message names
/// the flag because only API callers read it, and a repeat returns this again.
fn code_in_use(entry: UrlEntry, yours: bool) -> AppError {
    let message = if yours {
        format!(
            "you already use code '{}' for {} — edit it, or resend with overwrite=true to replace it",
            entry.code, entry.url
        )
    } else {
        format!(
            "code '{}' belongs to someone else — resend with overwrite=true to replace it",
            entry.code
        )
    };
    AppError::conflict(message)
        .on_field("code")
        .on_conflict(entry)
}

/// Nobody owns it, the caller owns it, or the caller is an admin the owner
/// answers to. An admin's reach runs down their own branch for a link as it
/// does for an account, so destroying one upwards is refused as taking it is.
async fn may_write(
    state: &AppState,
    session: &mut mongodb::ClientSession,
    existing: &Document,
    user: Option<&User>,
) -> Result<bool, AppError> {
    let Some(owner) = owner_id(existing) else {
        return Ok(true);
    };
    let Some(user) = user else {
        return Ok(false);
    };
    if owner == user.id {
        return Ok(true);
    }
    if !user.is_admin {
        return Ok(false);
    }
    // An owner id pointing nowhere reads as unowned.
    let Some(owner) = users::find_by_id_in(&state.db, &mut *session, owner).await? else {
        return Ok(true);
    };
    users::may_manage(&state.db, session, user, &owner).await
}

/// Answers, for each row, whether the caller may edit it and may take it.
/// Two queries whatever the page holds: the owners repeat far more often than
/// they differ, so both lookups are done once for the set.
async fn into_entries(
    state: &AppState,
    user: Option<&User>,
    rows: Vec<Document>,
) -> Result<Vec<UrlEntry>, AppError> {
    let owners: Vec<String> = rows
        .iter()
        .filter_map(|row| row.get_str("owner_id").ok().map(str::to_string))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let mut session = state.db.client().start_session().await?;
    let admins: std::collections::HashSet<String> = if owners.is_empty() {
        std::collections::HashSet::new()
    } else {
        users::find_many_in(&state.db, &mut session, &owners)
            .await?
            .into_iter()
            .filter(|owner| owner.is_admin)
            .map(|owner| owner.id)
            .collect()
    };
    let chains = if owners.is_empty() {
        std::collections::HashMap::new()
    } else {
        users::ancestors_of_many(&state.db, &mut session, &owners).await?
    };

    rows.into_iter()
        .map(|row| {
            let owner = row.get_str("owner_id").ok().map(str::to_string);
            let mut entry: UrlEntry = bson::from_document(row).map_err(AppError::internal)?;
            let (editable, claimable) = match (user, owner.as_deref()) {
                (None, None) => (true, false),
                (None, Some(_)) => (false, false),
                (Some(user), None) => (true, user.is_admin),
                (Some(user), Some(owner)) if owner == user.id => (true, false),
                (Some(user), Some(owner)) => {
                    // Not an admin means in nobody's subtree; not found means gone.
                    let reaches = !admins.contains(owner)
                        || chains.get(owner).is_some_and(|up| up.contains(&user.id));
                    (user.is_admin && reaches, user.is_admin && reaches)
                }
            };
            entry.editable = editable;
            entry.claimable = claimable;
            Ok(entry)
        })
        .collect()
}

async fn fetch_entry(
    state: &AppState,
    user: Option<&User>,
    code: &str,
) -> Result<UrlEntry, AppError> {
    let rows: Vec<Document> = state
        .db
        .collection::<Document>("urls")
        .aggregate(url_aggregate_pipeline(
            doc! {"code": code},
            1,
            0,
            doc! {"_id": 1},
        ))
        .await?
        .try_collect()
        .await?;
    into_entries(state, user, rows)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::internal("upserted url not found"))
}

/// `conflict_on_own` separates create from edit: re-creating a code you own is
/// a mistake worth a 409, while a PUT at it is the edit itself.
async fn upsert(
    state: &AppState,
    code: &str,
    body: &UrlUpsertRequest,
    user: Option<&User>,
    conflict_on_own: bool,
) -> Result<Json<UrlEntry>, AppError> {
    validate_code(code).map_err(|e| AppError::validation(e).on_field("code"))?;
    validate_code(&body.code).map_err(|e| AppError::validation(e).on_field("code"))?;
    validate_url(&body.url).map_err(|e| AppError::validation(e).on_field("url"))?;
    let expires = parse_expiry(body.expires_at.as_deref())?;
    let overwrite = body.overwrite.unwrap_or(false);
    let claiming = body.claim.unwrap_or(false);

    let urls = state.db.collection::<Document>("urls");
    let existing = urls.find_one(doc! {"code": code}).await?;
    let existing_owner = existing
        .as_ref()
        .and_then(|doc| owner_id(doc).map(str::to_string));
    let mut session = state.db.client().start_session().await?;
    if let Some(existing) = &existing {
        // Permission first: not-yours answers 403 with nothing in it.
        if !may_write(state, &mut session, existing, user).await? {
            return Err(owned_error(code));
        }
        // Writing and taking are different powers; only the second needs this.
        if claiming {
            let allowed = match user {
                Some(user) => may_claim(state, &mut session, user, existing).await?,
                None => false,
            };
            if !allowed {
                return Err(AppError::forbidden(
                    "that link belongs to an admin outside your part of the chain",
                )
                .on_field("claim"));
            }
        }
        // Only worth asking when someone owns it; an unowned link is anyone's.
        if conflict_on_own && !overwrite && existing_owner.is_some() {
            let yours = existing_owner.as_deref() == user.map(|u| u.id.as_str());
            return Err(code_in_use(fetch_entry(state, user, code).await?, yours));
        }
    }

    // Without this the unique index answers 500 and nobody is consulted.
    if body.code != code
        && let Some(target) = urls.find_one(doc! {"code": &body.code}).await?
    {
        match owner_id(&target) {
            // No more than a DELETE followed by this same rename.
            None => {
                urls.delete_one(doc! {"code": &body.code}).await?;
            }
            // Deleting somebody's link is allowed, but never as a side effect.
            Some(owner) => {
                if !may_write(state, &mut session, &target, user).await? {
                    return Err(owned_error(&body.code));
                }
                if !overwrite {
                    let yours = user.is_some_and(|u| u.id == owner);
                    return Err(code_in_use(
                        fetch_entry(state, user, &body.code).await?,
                        yours,
                    ));
                }
                urls.delete_one(doc! {"code": &body.code}).await?;
            }
        }
    }

    // Stamped on creation too, so an edit is distinguishable from the original.
    let mut set = doc! {
        "code": &body.code,
        "url": &body.url,
        "updated_at": bson::DateTime::now(),
    };
    // `$unset`, not null: the TTL index is partial on the field existing.
    let mut unset = Document::new();
    // The last visit goes with the count it belongs to.
    if body.reset_hits.unwrap_or(false) {
        set.insert("hits", 0);
        unset.insert("last_hit_at", "");
    }
    match expires {
        Expiry::Leave => {}
        Expiry::Clear => {
            unset.insert("expires_at", "");
        }
        Expiry::At(at) => {
            set.insert("expires_at", at);
        }
    }
    // `$setOnInsert` misses an existing document, and `owner` takes one operator.
    let mut on_insert = doc! {"created_at": bson::DateTime::now()};
    if let Some(user) = user {
        // The only way that takes a link off somebody, so it is asked for.
        if claiming || (conflict_on_own && existing_owner.is_none()) {
            set.insert("owner", &user.id);
        } else {
            on_insert.insert("owner", &user.id);
        }
    }
    let mut update = doc! {"$set": set, "$setOnInsert": on_insert};
    if !unset.is_empty() {
        update.insert("$unset", unset);
    }
    urls.update_one(doc! {"code": code}, update)
        .upsert(true)
        .await?;
    Ok(Json(fetch_entry(state, user, &body.code).await?))
}

pub async fn create_url(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    AppJson(body): AppJson<UrlUpsertRequest>,
) -> Result<Json<UrlEntry>, AppError> {
    let code = body.code.clone();
    upsert(&state, &code, &body, user.as_ref(), true).await
}

pub async fn update_url(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    Path(code): Path<String>,
    AppJson(body): AppJson<UrlUpsertRequest>,
) -> Result<Json<UrlEntry>, AppError> {
    upsert(&state, &code, &body, user.as_ref(), false).await
}

pub async fn delete_url(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    Path(code): Path<String>,
) -> Result<Json<DeleteResponse>, AppError> {
    let urls = state.db.collection::<Document>("urls");
    let existing = urls
        .find_one(doc! {"code": &code})
        .await?
        .ok_or_else(|| AppError::not_found("code not found"))?;
    let mut session = state.db.client().start_session().await?;
    if !may_write(&state, &mut session, &existing, user.as_ref()).await? {
        return Err(owned_error(&code));
    }
    urls.delete_one(doc! {"code": &code}).await?;
    Ok(Json(DeleteResponse {
        code,
        deleted: true,
    }))
}

/// Takes ownership. Claiming an unowned link clears its expiry — that deadline
/// is what an orphan is dying of, and a deliberate one is indistinguishable.
/// An owned link is taken off somebody, which [`may_claim`] governs.
pub async fn claim_url(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Path(code): Path<String>,
) -> Result<Json<UrlEntry>, AppError> {
    let urls = state.db.collection::<Document>("urls");
    let existing = urls
        .find_one(doc! {"code": &code})
        .await?
        .ok_or_else(|| AppError::not_found("code not found"))?;
    if owner_id(&existing) == Some(user.id.as_str()) {
        return Err(AppError::conflict(format!("you already own '{code}'")));
    }
    let unowned = owner_id(&existing).is_none();

    let mut session = state.db.client().start_session().await?;
    if !may_claim(&state, &mut session, &user, &existing).await? {
        return Err(AppError::forbidden(
            "that link belongs to an admin outside your part of the chain",
        ));
    }

    let mut update = doc! {
        "$set": {"owner": &user.id, "updated_at": bson::DateTime::now()},
    };
    if unowned {
        update.insert("$unset", doc! {"expires_at": ""});
    }
    urls.update_one(doc! {"code": &code}, update).await?;
    Ok(Json(fetch_entry(&state, Some(&user), &code).await?))
}

/// The cap on one request; a client ticking more sends several.
const BULK_MAX: usize = 100;

/// Which of `links` the caller may act on, from sets fetched once — the same
/// rules the single routes apply, so an ordinary owner is in no subtree and an
/// admin's chain must reach the caller.
async fn permitted(
    state: &AppState,
    session: &mut mongodb::ClientSession,
    user: &User,
    links: &[Document],
    action: UrlBulkAction,
) -> Result<std::collections::HashSet<String>, AppError> {
    let owners: Vec<String> = links
        .iter()
        .filter_map(|doc| owner_id(doc).map(str::to_string))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let admins: std::collections::HashSet<String> =
        users::find_many_in(&state.db, &mut *session, &owners)
            .await?
            .into_iter()
            .filter(|owner| owner.is_admin)
            .map(|owner| owner.id)
            .collect();
    let chains = users::ancestors_of_many(&state.db, &mut *session, &owners).await?;
    // A non-admin owner is in nobody's subtree; one not found no longer exists.
    let reaches = |owner: &str| {
        !admins.contains(owner) || chains.get(owner).is_some_and(|up| up.contains(&user.id))
    };

    Ok(links
        .iter()
        .filter(|doc| match (action, owner_id(doc)) {
            (UrlBulkAction::Delete, None) => true,
            (UrlBulkAction::Delete, Some(owner)) => {
                owner == user.id || (user.is_admin && reaches(owner))
            }
            (UrlBulkAction::Claim, None) => true,
            (UrlBulkAction::Claim, Some(owner)) => owner != user.id && reaches(owner),
        })
        .filter_map(|doc| doc.get_str("code").ok().map(str::to_string))
        .collect())
}

/// Resolves every named link and checks `action` against it, collecting
/// refusals rather than stopping at the first. One query for the links, and
/// for a claim two more, however many codes were named.
async fn bulk_targets(
    state: &AppState,
    session: &mut mongodb::ClientSession,
    user: &User,
    codes: &[String],
    action: UrlBulkAction,
) -> Result<Vec<Document>, AppError> {
    let mut cursor = state
        .db
        .collection::<Document>("urls")
        .find(doc! {"code": {"$in": codes}})
        .session(&mut *session)
        .await?;
    let mut found: std::collections::HashMap<String, Document> = std::collections::HashMap::new();
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        if let Ok(code) = row.get_str("code") {
            found.insert(code.to_string(), row);
        }
    }

    let links: Vec<Document> = found.values().cloned().collect();
    let allowed = permitted(state, session, user, &links, action).await?;

    // In the order they were sent, so the refusals read that way too.
    let mut seen = std::collections::HashSet::new();
    let mut targets = Vec::with_capacity(codes.len());
    let mut rejected = Vec::new();
    for code in codes {
        if !seen.insert(code.as_str()) {
            continue;
        }
        let Some(existing) = found.get(code) else {
            rejected.push(RejectedId {
                id: code.clone(),
                code: "not_found".into(),
                message: "code not found".into(),
            });
            continue;
        };
        if allowed.contains(code) {
            targets.push(existing.clone());
        } else {
            let err = owned_error(code);
            rejected.push(RejectedId {
                id: code.clone(),
                code: err.code.to_string(),
                message: err.message,
            });
        }
    }
    if rejected.is_empty() {
        return Ok(targets);
    }
    Err(AppError::validation("some of those links cannot be changed").on_rejected(rejected))
}

/// Delete or claim a selection in one request. All-or-nothing, as the users
/// endpoint is: checked whole, written whole. Links do not cascade, so the
/// reason here is consistency rather than correctness.
pub async fn bulk_urls(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    AppJson(body): AppJson<BulkUrlsRequest>,
) -> Result<Json<BulkUrlsResponse>, AppError> {
    if body.codes.is_empty() {
        return Err(AppError::validation("no links were named"));
    }
    if body.codes.len() > BULK_MAX {
        return Err(AppError::validation(format!(
            "no more than {BULK_MAX} links at a time"
        )));
    }
    if body.action == UrlBulkAction::Claim && !user.is_admin {
        return Err(AppError::forbidden("only an admin can claim a link"));
    }

    let urls = state.db.collection::<Document>("urls");
    let mut session = state.db.client().start_session().await?;
    session.start_transaction().await?;
    let targets = bulk_targets(&state, &mut session, &user, &body.codes, body.action).await?;

    let codes: Vec<String> = targets
        .iter()
        .filter_map(|doc| doc.get_str("code").ok().map(str::to_string))
        .collect();
    let mut result = BulkUrlsResponse {
        affected: codes.len() as u64,
        ..Default::default()
    };
    match body.action {
        UrlBulkAction::Delete => {
            urls.delete_many(doc! {"code": {"$in": &codes}})
                .session(&mut session)
                .await?;
        }
        UrlBulkAction::Claim => {
            // Two writes: rescuing an orphan takes its deadline off.
            let (mut orphans, mut owned) = (Vec::new(), Vec::new());
            for doc in &targets {
                let Ok(code) = doc.get_str("code") else {
                    continue;
                };
                if owner_id(doc).is_none() {
                    orphans.push(code.to_string());
                } else {
                    owned.push(code.to_string());
                }
            }
            let set = doc! {"owner": &user.id, "updated_at": bson::DateTime::now()};
            if !orphans.is_empty() {
                urls.update_many(
                    doc! {"code": {"$in": &orphans}},
                    doc! {"$set": &set, "$unset": {"expires_at": ""}},
                )
                .session(&mut session)
                .await?;
            }
            if !owned.is_empty() {
                urls.update_many(doc! {"code": {"$in": &owned}}, doc! {"$set": &set})
                    .session(&mut session)
                    .await?;
            }
        }
    }
    session.commit_transaction().await?;

    // Outside the transaction, so the entries carry the joined owner.
    if body.action == UrlBulkAction::Claim {
        let rows: Vec<Document> = urls
            .aggregate(url_aggregate_pipeline(
                doc! {"code": {"$in": &codes}},
                codes.len() as i64,
                0,
                doc! {"_id": 1},
            ))
            .await?
            .try_collect()
            .await?;
        result.entries = into_entries(&state, Some(&user), rows).await?;
    }
    Ok(Json(result))
}

pub async fn list_urls(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<UrlListResponse>, AppError> {
    let params = QueryPairs::new(pairs);
    let parsed = ListParams::from_query(&params)?;
    let filters = UrlFilters::from_query(&params)?;
    // Admins see every link; `mine=true` asks for the ordinary view anyway.
    let mine = params.first("mine").is_some_and(|v| v == "true");
    let owner = (!user.is_admin || mine).then_some(user.id.as_str());
    let owner_ids = match filters.owner_names() {
        [] => Vec::new(),
        names => users::ids_by_username(&state.db, names).await?,
    };
    let filter = filters.apply(parsed.filter(owner), &owner_ids);

    let urls = state.db.collection::<Document>("urls");
    let total = urls.count_documents(filter.clone()).await?;
    let rows: Vec<Document> = urls
        .aggregate(url_aggregate_pipeline(
            filter,
            parsed.limit,
            parsed.skip,
            parsed.sort,
        ))
        .await?
        .try_collect()
        .await?;
    let items = into_entries(&state, Some(&user), rows).await?;

    Ok(Json(UrlListResponse { items, total }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_expiry_distinguishes_absent_from_empty() {
        assert!(matches!(parse_expiry(None).unwrap(), Expiry::Leave));
        assert!(matches!(parse_expiry(Some("")).unwrap(), Expiry::Clear));
        assert!(matches!(parse_expiry(Some("   ")).unwrap(), Expiry::Clear));
    }

    #[test]
    fn parse_expiry_accepts_future_rfc3339_with_offset() {
        let future = chrono::Utc::now() + chrono::Duration::days(7);
        for raw in [
            future.to_rfc3339(),
            future.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            future
                .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap())
                .to_rfc3339(),
        ] {
            let Expiry::At(parsed) = parse_expiry(Some(&raw)).unwrap() else {
                panic!("a future stamp is an expiry");
            };
            assert_eq!(parsed.timestamp_millis(), future.timestamp_millis());
        }
    }

    #[test]
    fn parse_expiry_rejects_past_and_garbage() {
        assert!(parse_expiry(Some("2001-01-01T00:00:00Z")).is_err());
        assert!(parse_expiry(Some("not-a-date")).is_err());
        assert!(parse_expiry(Some("2030-01-01")).is_err()); // date without time is not RFC3339
        assert!(parse_expiry(Some("2030-01-01T00:00:00")).is_err());
    }

    #[test]
    fn owner_id_reads_only_string_owners() {
        assert_eq!(owner_id(&doc! {"code": "a"}), None);
        assert_eq!(owner_id(&doc! {"code": "a", "owner": Bson::Null}), None);
        assert_eq!(owner_id(&doc! {"code": "a", "owner": "u1"}), Some("u1"));
    }

    #[test]
    fn owned_error_is_403_and_never_contains_url() {
        let err = owned_error("abc");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert!(err.message.contains("'abc'"));
        assert!(!err.message.contains("http"));
    }

    fn an_entry(code: &str, url: &str) -> UrlEntry {
        UrlEntry {
            code: code.into(),
            url: url.into(),
            owner: None,
            hits: 4,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
            expires_at: None,
            editable: true,
            claimable: false,
        }
    }

    #[test]
    fn a_taken_code_is_409_that_carries_the_link() {
        let err = code_in_use(an_entry("abc", "https://example.com"), true);
        assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
        assert!(err.message.contains("https://example.com"));
        // A client offering to replace it has to show what it is replacing.
        assert_eq!(err.conflict.map(|c| c.hits), Some(4));
    }

    /// Someone else's link still rides along — this error only reaches a caller
    /// allowed to write over it — but the sentence stops claiming it is theirs.
    #[test]
    fn a_taken_code_that_is_not_yours_says_so() {
        let err = code_in_use(an_entry("abc", "https://example.com"), false);
        assert!(err.message.contains("belongs to someone else"));
        assert!(!err.message.contains("you already use"));
        assert!(err.conflict.is_some());
    }

    /// The way out has to be in the message. An API caller who repeats the
    /// request unchanged gets this same error back, so "try again" would be
    /// advice that does not work.
    #[test]
    fn both_conflicts_name_the_flag_that_gets_past_them() {
        for yours in [true, false] {
            let err = code_in_use(an_entry("abc", "https://example.com"), yours);
            assert!(err.message.contains("overwrite=true"), "{}", err.message);
        }
    }
}
