use axum::Json;
use axum::extract::{Path, Query, State};
use shared::{
    AdminOrphans, AdminSettings, AdminUserListResponse, BulkAction, BulkUsersRequest,
    BulkUsersResponse, DeleteUserParams, DeleteUserResponse, RejectedId, SetAdminRequest,
    SetAdminResponse, UpdateSettingsRequest,
};

use mongodb::ClientSession;

use crate::app::AppState;
use crate::auth::extract::{AdminUser, RootAdmin};
use crate::chain::{self, Forest, Plan};
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
    forest: &Forest,
    actor: &User,
    orphans: AdminOrphans,
) -> Result<Json<SetAdminResponse>, AppError> {
    if is_root(actor) {
        return Err(AppError::validation(
            "you are the top admin so cannot give up the flag — it can only be removed directly in the database",
        ));
    }
    let plan = demotion(state, session, forest, actor, &actor.id, orphans).await?;
    Ok(Json(SetAdminResponse {
        id: actor.id.clone(),
        is_admin: false,
        promoted_by: None,
        // Everyone the branch took with it, the account itself included.
        demoted: plan.demoted.len() as u64,
        reparented: plan.moved(),
    }))
}

/// Takes the flag off one account and disposes of the branch per `orphans`,
/// which is [`chain::plan`] over a selection of one.
async fn demotion(
    state: &AppState,
    session: &mut ClientSession,
    forest: &Forest,
    actor: &User,
    target: &str,
    orphans: AdminOrphans,
) -> Result<Plan, AppError> {
    let plan = chain::plan(
        forest,
        &actor.id,
        &[target.to_string()],
        BulkAction::Demote,
        orphans,
    );
    users::apply(&state.db, session, &plan, state.config.orphan_grace_days).await?;
    Ok(plan)
}

/// Loads the target and checks the actor may touch it. Ordered so a typo reads
/// as "no such user" and no write happens before every check has passed.
async fn target_user(
    state: &AppState,
    session: &mut ClientSession,
    forest: &Forest,
    actor: &User,
    id: &str,
) -> Result<User, AppError> {
    not_yourself(actor, id)?;
    let target = users::find_by_id_in(&state.db, &mut *session, id)
        .await?
        .ok_or_else(|| AppError::not_found("no such user"))?;
    if !forest.may_manage(&actor.id, &target.id) {
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
    forest: &Forest,
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
    if !forest.is_admin(parent_id) {
        // Only the refusal needs to know whether they exist at all.
        return Err(
            match users::find_by_id_in(&state.db, session, parent_id).await? {
                Some(_) => AppError::validation("they can only be placed under an admin"),
                None => AppError::validation("no such admin to place them under"),
            },
        );
    }
    if !forest.ancestors(parent_id).contains(&actor.id) {
        return Err(AppError::forbidden(
            "you can only place someone under yourself or an admin below you",
        ));
    }
    if target.is_admin && forest.subtree(&target.id).iter().any(|id| id == parent_id) {
        return Err(AppError::validation(
            "that would put them under one of their own admins",
        ));
    }
    Ok(parent_id.to_string())
}

/// A change to one account's standing, which is still several writes when the
/// branch below it moves — so it is a transaction, as the bulk route is.
pub async fn set_user_admin(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<String>,
    AppJson(body): AppJson<SetAdminRequest>,
) -> Result<Json<SetAdminResponse>, AppError> {
    let mut session = session(&state).await?;
    session.start_transaction().await?;
    let answer = set_admin_in(&state, &mut session, &actor, &id, body).await?;
    session.commit_transaction().await?;
    Ok(answer)
}

/// One write to the parent pointer, since that is all the chain stores.
async fn set_admin_in(
    state: &AppState,
    session: &mut ClientSession,
    actor: &User,
    id: &str,
    body: SetAdminRequest,
) -> Result<Json<SetAdminResponse>, AppError> {
    let forest = users::admin_forest(&state.db, &mut *session).await?;
    // Resigning is the one thing you may do to your own standing.
    if actor.id == id && !body.is_admin {
        return resign(state, &mut *session, &forest, actor, body.orphans).await;
    }
    let target = target_user(state, &mut *session, &forest, actor, id).await?;
    if !body.is_admin {
        let plan = demotion(
            state,
            &mut *session,
            &forest,
            actor,
            &target.id,
            body.orphans,
        )
        .await?;
        return Ok(Json(SetAdminResponse {
            id: target.id,
            is_admin: false,
            promoted_by: None,
            demoted: plan.demoted.len() as u64,
            reparented: plan.moved(),
        }));
    }
    let parent = parent_for(
        state,
        &mut *session,
        &forest,
        actor,
        &target,
        body.promoted_by.as_deref(),
    )
    .await?;
    users::apply(
        &state.db,
        &mut *session,
        &chain::promote_under(&parent, std::slice::from_ref(&target.id)),
        state.config.orphan_grace_days,
    )
    .await?;
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
    session.start_transaction().await?;
    let forest = users::admin_forest(&state.db, &mut session).await?;
    let target = target_user(&state, &mut session, &forest, &actor, &id).await?;
    let plan = chain::plan(
        &forest,
        &actor.id,
        std::slice::from_ref(&target.id),
        BulkAction::Delete,
        params.orphans,
    );
    let links = users::apply(
        &state.db,
        &mut session,
        &plan,
        state.config.orphan_grace_days,
    )
    .await?;
    session.commit_transaction().await?;

    Ok(Json(DeleteUserResponse {
        id: target.id,
        deleted: true,
        orphaned: links.orphaned,
        links_deleted: links.deleted,
        grace_days: state.config.orphan_grace_days,
        // The account is being removed, not demoted, so only the branch counts.
        demoted: plan.demoted_below,
        reparented: plan.moved(),
    }))
}

/// The cap on one request; a client ticking more than this sends several.
const BULK_MAX: usize = 100;

/// Resolves every named account, collecting refusals rather than stopping at
/// the first — otherwise the caller fixes them one round trip at a time.
async fn bulk_targets(
    state: &AppState,
    session: &mut ClientSession,
    forest: &Forest,
    actor: &User,
    ids: &[String],
) -> Result<Vec<User>, AppError> {
    let found: std::collections::HashMap<String, User> =
        users::find_many_in(&state.db, session, ids)
            .await?
            .into_iter()
            .map(|user| (user.id.clone(), user))
            .collect();

    // Deduplicated: a repeat would inflate the counts and delete twice.
    let mut seen = std::collections::HashSet::new();
    let mut targets = Vec::with_capacity(ids.len());
    let mut rejected = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        // The same checks the single-account routes make, refusals and all.
        let refusal = if actor.id == *id {
            Some(AppError::validation(
                "you cannot do that to your own account",
            ))
        } else if !found.contains_key(id) {
            Some(AppError::not_found("no such user"))
        } else if !forest.may_manage(&actor.id, id) {
            Some(AppError::forbidden(
                "that admin is not in your part of the chain, so you cannot change their account",
            ))
        } else {
            None
        };
        match refusal {
            None => targets.push(found[id].clone()),
            Some(err) => rejected.push(RejectedId {
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
    let forest = users::admin_forest(&state.db, &mut session).await?;
    let targets: Vec<String> = bulk_targets(&state, &mut session, &forest, &actor, &body.ids)
        .await?
        .into_iter()
        .map(|target| target.id)
        .collect();

    let plan = chain::plan(&forest, &actor.id, &targets, body.action, body.orphans);
    let links = users::apply(
        &state.db,
        &mut session,
        &plan,
        state.config.orphan_grace_days,
    )
    .await?;
    session.commit_transaction().await?;

    Ok(Json(BulkUsersResponse {
        affected: targets.len() as u64,
        demoted: plan.demoted_below,
        reparented: plan.moved(),
        orphaned: links.orphaned,
        grace_days: state.config.orphan_grace_days,
    }))
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
