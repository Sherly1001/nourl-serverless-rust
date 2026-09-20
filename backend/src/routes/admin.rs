use axum::Json;
use axum::extract::{Path, Query, State};
use shared::{
    AdminOrphans, AdminSettings, AdminUserListResponse, BulkAction, BulkUsersRequest,
    BulkUsersResponse, DeleteUserParams, DeleteUserResponse, LinkDisposition, RejectedId,
    SetAdminRequest, SetAdminResponse, UpdateSettingsRequest,
};

use mongodb::ClientSession;

use crate::app::AppState;
use crate::auth::extract::{AdminUser, RootAdmin};
use crate::error::AppError;
use crate::extract::AppJson;
use crate::query::UserListParams;
use crate::settings;
use crate::users::{self, User};

/// The account tree, in two parts.
///
/// Every admin comes back whole so the client can draw the chain, and the
/// accounts with no flag are paged and searched underneath it. `AdminUser` is
/// the whole authorisation check — see `auth::extract::AdminUser`.
pub async fn list_users(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<AdminUserListResponse>, AppError> {
    let parsed = UserListParams::from_query(&params)?;
    let admins = users::admins(&state.db, &parsed).await?;
    let items = users::list(&state.db, &parsed).await?;
    let total = users::count(&state.db, parsed.filter()).await?;
    Ok(Json(AdminUserListResponse {
        admins,
        items,
        total,
    }))
}

/// A root is an admin nobody promoted — the one seeded directly in the
/// database. Nothing sits above them, so nothing can restore what they give up.
/// A session for one request's worth of work on the chain.
///
/// Every helper below takes one so that a caller which needs a transaction can
/// start one on it. On its own it changes nothing: a session without a
/// transaction reads and writes exactly as the bare database did.
async fn session(state: &AppState) -> Result<ClientSession, AppError> {
    Ok(state.db.client().start_session().await?)
}

fn is_root(user: &User) -> bool {
    user.is_admin && user.promoted_by.is_none()
}

/// Refuses acting on your own account. Resigning is the one exception and is
/// handled separately in [`set_user_admin`]: everything else here — promoting
/// yourself, moving yourself somewhere you were not put — is a way to change
/// your own standing, which is exactly what the chain exists to prevent.
fn not_yourself(actor: &User, target_id: &str) -> Result<(), AppError> {
    if actor.id == target_id {
        return Err(AppError::validation(
            "you cannot do that to your own account",
        ));
    }
    Ok(())
}

/// Giving up your own flag.
///
/// Allowed, because an admin who no longer wants the responsibility should not
/// have to ask permission for it, and whoever promoted them can put it back.
/// The cascade applies as it does to any demotion: the branch below was vouched
/// for through this account, so it goes too.
///
/// A root is refused. Nobody is above them to restore anything, so a root
/// resigning would demote every admin on the system at once and leave the admin
/// pages unreachable — recoverable only by editing the collection, which is
/// where the root came from in the first place.
async fn resign(
    state: &AppState,
    session: &mut ClientSession,
    actor: &User,
    orphans: AdminOrphans,
) -> Result<Json<SetAdminResponse>, AppError> {
    if is_root(actor) {
        return Err(AppError::validation(
            "you are the top admin so cannot give up the flag — it can only be removed directly in the database",
        ));
    }
    let (demoted, reparented) = demote(state, session, actor, orphans).await?;
    Ok(Json(SetAdminResponse {
        id: actor.id.clone(),
        is_admin: false,
        promoted_by: None,
        demoted,
        reparented: reparented.len() as u64,
    }))
}

/// Takes the flag off `target`, and does with the branch below them whatever
/// `orphans` says. Returns how many lost the flag and how many kept it by
/// moving up.
///
/// Re-parenting runs first: once the children hang from the target's own
/// parent, revoking finds nothing below and takes only the target itself.
async fn demote(
    state: &AppState,
    session: &mut ClientSession,
    target: &User,
    orphans: AdminOrphans,
) -> Result<(u64, Vec<String>), AppError> {
    let reparented = match orphans {
        AdminOrphans::Demote => Vec::new(),
        AdminOrphans::Reparent => {
            users::reparent_children(
                &state.db,
                &mut *session,
                &target.id,
                target.promoted_by.as_deref(),
            )
            .await?
        }
    };
    let demoted = users::revoke_admin(&state.db, session, &target.id).await?;
    Ok((demoted, reparented))
}

/// Loads the target and checks the actor is allowed to touch it. Ordered so a
/// typo'd id reads as "no such user" rather than a silent no-op reported as
/// success, and so no write happens before every check has passed.
async fn target_user(
    state: &AppState,
    session: &mut ClientSession,
    actor: &User,
    id: &str,
) -> Result<User, AppError> {
    not_yourself(actor, id)?;
    let target = users::find_by_id_in(&state.db, &mut *session, id)
        .await?
        .ok_or_else(|| AppError::not_found("no such user"))?;
    if !users::may_manage(&state.db, session, actor, &target).await? {
        return Err(AppError::forbidden(
            "that admin is not in your part of the chain, so you cannot change their account",
        ));
    }
    Ok(target)
}

/// Which admin `target` will hang under, and whether the actor is allowed to
/// put them there. Returns the parent's id.
///
/// An omitted parent means *leave them where they are*: for someone who is
/// already an admin that is their current parent, and for everyone else it is
/// the caller, which is the ordinary promotion. Defaulting to the caller in
/// both cases would make a bare `{"is_admin": true}` quietly re-parent an
/// existing admin — and their whole branch — onto whoever sent it, so a stray
/// toggle of an admin switch would restructure the tree. Moving is worth
/// asking for explicitly.
///
/// Naming a parent is that explicit move, and moves are the operation that can
/// break the tree, so each way of breaking it is refused separately:
///
/// - under an ordinary account, which would leave an admin outside the chain;
/// - under themselves, or under one of their own descendants, which would cut
///   the whole subtree loose from the root;
/// - under an admin the caller does not control, which would either graft the
///   target somewhere unreachable or quietly hand it to a stranger.
async fn parent_for(
    state: &AppState,
    session: &mut ClientSession,
    actor: &User,
    target: &User,
    requested: Option<&str>,
) -> Result<String, AppError> {
    let Some(parent_id) = requested else {
        return Ok(target
            .promoted_by
            .clone()
            .unwrap_or_else(|| actor.id.clone()));
    };
    if parent_id == actor.id {
        return Ok(actor.id.clone());
    }
    if parent_id == target.id {
        return Err(AppError::validation(
            "an account cannot be promoted by itself",
        ));
    }
    let parent = users::find_by_id_in(&state.db, &mut *session, parent_id)
        .await?
        .ok_or_else(|| AppError::validation("no such admin to place them under"))?;
    if !parent.is_admin {
        return Err(AppError::validation(
            "they can only be placed under an admin",
        ));
    }
    if !users::ancestor_ids(&state.db, &mut *session, &parent.id)
        .await?
        .contains(&actor.id)
    {
        return Err(AppError::forbidden(
            "you can only place someone under yourself or an admin below you",
        ));
    }
    if target.is_admin
        && users::descendant_ids(&state.db, session, &target.id)
            .await?
            .contains(&parent.id)
    {
        return Err(AppError::validation(
            "that would put them under one of their own admins",
        ));
    }
    Ok(parent.id)
}

/// Grants, moves, or revokes — all three are one write to the parent pointer,
/// because "who vouches for them" is the only thing the chain stores.
pub async fn set_user_admin(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<String>,
    AppJson(body): AppJson<SetAdminRequest>,
) -> Result<Json<SetAdminResponse>, AppError> {
    let mut session = session(&state).await?;
    // Resigning is the one thing you may do to your own standing.
    if actor.id == id && !body.is_admin {
        return resign(&state, &mut session, &actor, body.orphans).await;
    }
    let target = target_user(&state, &mut session, &actor, &id).await?;
    if !body.is_admin {
        let (demoted, reparented) = demote(&state, &mut session, &target, body.orphans).await?;
        return Ok(Json(SetAdminResponse {
            id: target.id,
            is_admin: false,
            promoted_by: None,
            demoted,
            reparented: reparented.len() as u64,
        }));
    }
    let parent = parent_for(
        &state,
        &mut session,
        &actor,
        &target,
        body.promoted_by.as_deref(),
    )
    .await?;
    users::grant_admin(&state.db, &mut session, &target.id, &parent).await?;
    Ok(Json(SetAdminResponse {
        id: target.id,
        is_admin: true,
        promoted_by: Some(parent),
        demoted: 0,
        reparented: 0,
    }))
}

/// Removes the account, orphaning its links rather than destroying them.
///
/// An admin never gets the choice the account's owner gets: someone may be
/// following those links, and taking a stranger's account off the system is not
/// a reason to break every URL they ever shared. Only the owner may ask for
/// [`LinkDisposition::Delete`], via `DELETE /api/auth/me`.
///
/// The demotion runs first and for a stronger reason than it does on its own:
/// the account those admins hang from is about to stop existing, so leaving
/// `promoted_by` pointing at it would strand the whole branch outside the tree,
/// where nothing walking upward could reach them again.
pub async fn delete_user(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<String>,
    Query(params): Query<DeleteUserParams>,
) -> Result<Json<DeleteUserResponse>, AppError> {
    let mut session = session(&state).await?;
    let target = target_user(&state, &mut session, &actor, &id).await?;
    let (demoted, reparented) = match params.orphans {
        // Counts the target as well, but the target is being deleted rather
        // than demoted, so only the branch below them is worth reporting.
        AdminOrphans::Demote => (
            users::revoke_admin(&state.db, &mut session, &target.id)
                .await?
                .saturating_sub(1),
            0,
        ),
        AdminOrphans::Reparent => (
            0,
            users::reparent_children(
                &state.db,
                &mut session,
                &target.id,
                target.promoted_by.as_deref(),
            )
            .await?
            .len() as u64,
        ),
    };
    let links = users::delete_with_cascade(
        &state.db,
        &mut session,
        &target.id,
        LinkDisposition::Orphan,
        state.config.orphan_grace_days,
    )
    .await?;
    Ok(Json(DeleteUserResponse {
        id: target.id,
        deleted: true,
        orphaned: links.orphaned,
        links_deleted: links.deleted,
        grace_days: state.config.orphan_grace_days,
        demoted,
        reparented,
    }))
}

/// The cap on one request. The list endpoint never returns more than a hundred
/// rows in its paged half, so a longer selection is not something anybody
/// ticked by hand — the admin half can be larger, and a client that ticks all
/// of it sends more than one request.
const BULK_MAX: usize = 100;

/// Resolves every named account, collecting the refusals rather than stopping
/// at the first.
///
/// A selection is judged as a unit, so naming only the first bad id would have
/// the caller fixing them one round trip at a time.
async fn bulk_targets(
    state: &AppState,
    session: &mut ClientSession,
    actor: &User,
    ids: &[String],
) -> Result<Vec<User>, AppError> {
    // Deduplicated, so a repeated id cannot make the counts claim more than
    // happened — and cannot try to delete the same account twice.
    let mut seen = std::collections::HashSet::new();
    let mut targets = Vec::with_capacity(ids.len());
    let mut rejected = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        // The same check the single-account routes make, refusal and all: own
        // account, unknown id, someone else's branch.
        match target_user(state, &mut *session, actor, id).await {
            Ok(target) => targets.push(target),
            Err(err) => rejected.push(RejectedId {
                id: id.clone(),
                code: err.code.to_string(),
                message: err.message,
            }),
        }
    }
    if rejected.is_empty() {
        return Ok(targets);
    }
    Err(AppError::validation("some of those accounts cannot be changed").on_rejected(rejected))
}

/// Promote, demote or delete a selection in one request.
///
/// The per-row alternative the Users page used to send had each call able to
/// move the tree under the next one: demoting a parent takes its children with
/// it, so the call for a child could arrive to find nothing left to demote and
/// report a failure for work that was already done.
///
/// Two rules make that go away. The whole thing runs in one transaction — every
/// id resolved and permission-checked before any write, and the writes
/// committed together — so a selection is applied whole or not at all, and half
/// an admin decision is not a state anybody has to reason about. And the
/// targets are handled deepest-first, so a cascade never reaches a row still
/// waiting its turn; that is about the *counts* rather than the outcome, since
/// each row is re-read before it is acted on. Handling a parent first would
/// have its re-parenting move children that the selection then demotes anyway,
/// reporting rows the admin ticked as though they were collateral.
///
/// A transient abort surfaces as an error for the caller to retry. Bulk actions
/// are rare and human-triggered, so "that failed, try again" is honest, and a
/// retry loop is a concurrency primitive worth its own change.
pub async fn bulk_users(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    AppJson(body): AppJson<BulkUsersRequest>,
) -> Result<Json<BulkUsersResponse>, AppError> {
    if body.ids.is_empty() {
        return Err(AppError::validation("no accounts were named"));
    }
    if body.ids.len() > BULK_MAX {
        return Err(AppError::validation(format!(
            "no more than {BULK_MAX} accounts at a time"
        )));
    }

    let mut session = session(&state).await?;
    session.start_transaction().await?;
    // Inside the transaction, so a concurrent promotion cannot slip between the
    // check and the write it was meant to guard.
    let mut targets = bulk_targets(&state, &mut session, &actor, &body.ids).await?;

    if body.action != BulkAction::Promote {
        // Depth is the number of admins above them, so the deepest sort first.
        let mut depths = Vec::with_capacity(targets.len());
        for target in &targets {
            depths.push(
                users::ancestor_ids(&state.db, &mut session, &target.id)
                    .await?
                    .len(),
            );
        }
        let mut ordered: Vec<(usize, User)> = depths.into_iter().zip(targets).collect();
        ordered.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
        targets = ordered.into_iter().map(|(_, target)| target).collect();
    }

    let mut result = BulkUsersResponse {
        grace_days: state.config.orphan_grace_days,
        ..Default::default()
    };
    // Who kept the flag, not how many times one was handed upwards: an admin
    // below two of the selected accounts climbs a level as each of them goes,
    // and a running total would report it once per level.
    let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
    for target in &targets {
        // Re-read: an earlier row in this same request may have demoted them or
        // moved them, and what happens next depends on where they are now.
        let Some(current) = users::find_by_id_in(&state.db, &mut session, &target.id).await? else {
            continue;
        };
        match body.action {
            BulkAction::Promote => {
                let parent = parent_for(&state, &mut session, &actor, &current, None).await?;
                users::grant_admin(&state.db, &mut session, &current.id, &parent).await?;
            }
            BulkAction::Demote => {
                // A cascade from higher in the selection may already have taken
                // the flag. Nothing left to do is success: the state the admin
                // asked for is the state it is in.
                if current.is_admin {
                    let (demoted, reparented) =
                        demote(&state, &mut session, &current, body.orphans).await?;
                    // `demote` counts the target itself, which `affected`
                    // already reports.
                    result.demoted += demoted.saturating_sub(1);
                    kept.extend(reparented);
                }
            }
            BulkAction::Delete => {
                if current.is_admin {
                    match body.orphans {
                        AdminOrphans::Demote => {
                            result.demoted +=
                                users::revoke_admin(&state.db, &mut session, &current.id)
                                    .await?
                                    .saturating_sub(1);
                        }
                        AdminOrphans::Reparent => {
                            kept.extend(
                                users::reparent_children(
                                    &state.db,
                                    &mut session,
                                    &current.id,
                                    current.promoted_by.as_deref(),
                                )
                                .await?,
                            );
                        }
                    }
                }
                // Never `LinkDisposition::Delete`: an admin removing somebody
                // else's account does not get to break every URL they shared.
                let links = users::delete_with_cascade(
                    &state.db,
                    &mut session,
                    &current.id,
                    LinkDisposition::Orphan,
                    state.config.orphan_grace_days,
                )
                .await?;
                result.orphaned += links.orphaned;
            }
        }
        result.affected += 1;
    }
    session.commit_transaction().await?;
    result.reparented = kept.len() as u64;
    Ok(Json(result))
}

/// `RootAdmin`, not `AdminUser`: see the extractor for why the sign-in
/// settings are the root's alone.
pub async fn get_settings(
    State(state): State<AppState>,
    RootAdmin(_): RootAdmin,
) -> Result<Json<AdminSettings>, AppError> {
    Ok(Json(settings::load(&state.db).await?.to_admin_view()))
}

/// Writes the settings and answers with the same view `get_settings` returns,
/// so the page re-renders from what was stored rather than from what it sent.
pub async fn update_settings(
    State(state): State<AppState>,
    RootAdmin(_): RootAdmin,
    AppJson(body): AppJson<UpdateSettingsRequest>,
) -> Result<Json<AdminSettings>, AppError> {
    // Merged against what is stored, because the request cannot carry the
    // secrets — see `MethodConfig::merged`.
    let merged = settings::load(&state.db).await?.merged(&body);
    settings::save(&state.db, &merged).await?;
    Ok(Json(merged.to_admin_view()))
}
