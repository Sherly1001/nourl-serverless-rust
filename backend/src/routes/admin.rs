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

/// The account tree: every admin whole so the client can draw the chain, and
/// the unflagged accounts paged underneath it.
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

/// A session for one request's work on the chain. Every helper takes one so a
/// caller can start a transaction on it; without one it reads and writes
/// exactly as the bare database did.
async fn session(state: &AppState) -> Result<ClientSession, AppError> {
    Ok(state.db.client().start_session().await?)
}

fn is_root(user: &User) -> bool {
    user.is_admin && user.promoted_by.is_none()
}

/// Refuses acting on your own account: anything else would be a way to change
/// your own standing. Resigning is the exception, handled in
/// [`set_user_admin`].
fn not_yourself(actor: &User, target_id: &str) -> Result<(), AppError> {
    if actor.id == target_id {
        return Err(AppError::validation(
            "you cannot do that to your own account",
        ));
    }
    Ok(())
}

/// Giving up your own flag, which needs nobody's permission — whoever granted
/// it can grant it again. The branch below goes too, as in any demotion. A
/// root is refused: it would demote everyone and lock the admin pages.
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

/// Takes the flag off `target` and disposes of the branch per `orphans`.
/// Re-parenting runs first: once the children hang elsewhere, revoking finds
/// nothing below and takes only the target.
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

/// Loads the target and checks the actor may touch it. Ordered so a typo reads
/// as "no such user" and no write happens before every check has passed.
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

/// Which admin `target` hangs under, and whether the actor may put them there.
/// Omitting a parent leaves an existing admin where they are, so a bare
/// `{"is_admin": true}` cannot restructure the tree by accident.
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

/// One write to the parent pointer, since that is all the chain stores.
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

/// Removes the account, orphaning its links: only the owner may ask for
/// [`LinkDisposition::Delete`]. The demotion runs first, or `promoted_by`
/// would point at an account that no longer exists.
pub async fn delete_user(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<String>,
    Query(params): Query<DeleteUserParams>,
) -> Result<Json<DeleteUserResponse>, AppError> {
    let mut session = session(&state).await?;
    let target = target_user(&state, &mut session, &actor, &id).await?;
    let (demoted, reparented) = match params.orphans {
        // The count includes the target, which is being deleted, not demoted.
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

/// The cap on one request; a client ticking more than this sends several.
const BULK_MAX: usize = 100;

/// Resolves every named account, collecting refusals rather than stopping at
/// the first — otherwise the caller fixes them one round trip at a time.
async fn bulk_targets(
    state: &AppState,
    session: &mut ClientSession,
    actor: &User,
    ids: &[String],
) -> Result<Vec<User>, AppError> {
    // Deduplicated: a repeat would inflate the counts and delete twice.
    let mut seen = std::collections::HashSet::new();
    let mut targets = Vec::with_capacity(ids.len());
    let mut rejected = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        // The same check the single-account routes make, refusal and all.
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

/// Promote, demote or delete a selection in one transaction: checked whole,
/// written whole. Deepest-first, which decides the counts rather than the
/// outcome — a parent first would report ticked rows as collateral.
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
    // Inside the transaction: no promotion slips between check and write.
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
                // A cascade may already have taken it; that is still success.
                if current.is_admin {
                    let (demoted, reparented) =
                        demote(&state, &mut session, &current, body.orphans).await?;
                    // `demote` counts the target, which `affected` reports.
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
                // Never `Delete`: not an admin's call to break shared URLs.
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

/// Answers with `get_settings`'s view, so the page re-renders from storage.
pub async fn update_settings(
    State(state): State<AppState>,
    RootAdmin(_): RootAdmin,
    AppJson(body): AppJson<UpdateSettingsRequest>,
) -> Result<Json<AdminSettings>, AppError> {
    // The request cannot carry the secrets — see `MethodConfig::merged`.
    let merged = settings::load(&state.db).await?.merged(&body);
    settings::save(&state.db, &merged).await?;
    Ok(Json(merged.to_admin_view()))
}
