use axum::Json;
use axum::extract::{Path, Query, State};
use shared::{
    AdminSettings, AdminUserListResponse, DeleteUserResponse, LinkDisposition, SetAdminRequest,
    SetAdminResponse, UpdateSettingsRequest,
};

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
async fn resign(state: &AppState, actor: &User) -> Result<Json<SetAdminResponse>, AppError> {
    if is_root(actor) {
        return Err(AppError::validation(
            "you are the top admin so cannot give up the flag — it can only be removed directly in the database",
        ));
    }
    let demoted = users::revoke_admin(&state.db, &actor.id).await?;
    Ok(Json(SetAdminResponse {
        id: actor.id.clone(),
        is_admin: false,
        promoted_by: None,
        demoted,
    }))
}

/// Whether `actor` may act on `target`.
///
/// An admin owns the subtree below them and nothing else: the chain of
/// `promoted_by` pointers above the target must pass through the actor. Being
/// higher up in general is not enough — an admin cannot reach sideways into a
/// branch grown by one of their peers.
///
/// An account with no flag is in no subtree at all, so any admin may act on it.
/// That is not a hole but the only way the tree can grow: a fresh signup has
/// nobody above them, so requiring ancestry would make the first promotion
/// impossible.
async fn may_manage(state: &AppState, actor: &User, target: &User) -> Result<bool, AppError> {
    if !target.is_admin {
        return Ok(true);
    }
    Ok(users::ancestor_ids(&state.db, &target.id)
        .await?
        .contains(&actor.id))
}

/// Loads the target and checks the actor is allowed to touch it. Ordered so a
/// typo'd id reads as "no such user" rather than a silent no-op reported as
/// success, and so no write happens before every check has passed.
async fn target_user(state: &AppState, actor: &User, id: &str) -> Result<User, AppError> {
    not_yourself(actor, id)?;
    let target = users::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| AppError::not_found("no such user"))?;
    if !may_manage(state, actor, &target).await? {
        return Err(AppError::forbidden(
            "that admin is not in your part of the chain, so you cannot change their account",
        ));
    }
    Ok(target)
}

/// Where `target` will hang, and whether the actor is allowed to hang them
/// there.
///
/// Defaults to the caller, which is the ordinary promotion. Naming someone else
/// is a move, and moves are the operation that can break the tree, so each way
/// of breaking it is refused separately:
///
/// - under an ordinary account, which would leave an admin outside the chain;
/// - under themselves, or under one of their own descendants, which would cut
///   the whole subtree loose from the root;
/// - under an admin the caller does not control, which would either graft the
///   target somewhere unreachable or quietly hand it to a stranger.
async fn parent_for(
    state: &AppState,
    actor: &User,
    target: &User,
    requested: Option<&str>,
) -> Result<User, AppError> {
    let Some(parent_id) = requested else {
        return Ok(actor.clone());
    };
    if parent_id == actor.id {
        return Ok(actor.clone());
    }
    if parent_id == target.id {
        return Err(AppError::validation(
            "an account cannot be promoted by itself",
        ));
    }
    let parent = users::find_by_id(&state.db, parent_id)
        .await?
        .ok_or_else(|| AppError::validation("no such admin to place them under"))?;
    if !parent.is_admin {
        return Err(AppError::validation(
            "they can only be placed under an admin",
        ));
    }
    if !users::ancestor_ids(&state.db, &parent.id)
        .await?
        .contains(&actor.id)
    {
        return Err(AppError::forbidden(
            "you can only place someone under yourself or an admin below you",
        ));
    }
    if target.is_admin
        && users::descendant_ids(&state.db, &target.id)
            .await?
            .contains(&parent.id)
    {
        return Err(AppError::validation(
            "that would put them under one of their own admins",
        ));
    }
    Ok(parent)
}

/// Grants, moves, or revokes — all three are one write to the parent pointer,
/// because "who vouches for them" is the only thing the chain stores.
pub async fn set_user_admin(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<String>,
    AppJson(body): AppJson<SetAdminRequest>,
) -> Result<Json<SetAdminResponse>, AppError> {
    // Resigning is the one thing you may do to your own standing.
    if actor.id == id && !body.is_admin {
        return resign(&state, &actor).await;
    }
    let target = target_user(&state, &actor, &id).await?;
    if !body.is_admin {
        let demoted = users::revoke_admin(&state.db, &target.id).await?;
        return Ok(Json(SetAdminResponse {
            id: target.id,
            is_admin: false,
            promoted_by: None,
            demoted,
        }));
    }
    let parent = parent_for(&state, &actor, &target, body.promoted_by.as_deref()).await?;
    users::grant_admin(&state.db, &target.id, &parent.id).await?;
    Ok(Json(SetAdminResponse {
        id: target.id,
        is_admin: true,
        promoted_by: Some(parent.id),
        demoted: 0,
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
) -> Result<Json<DeleteUserResponse>, AppError> {
    let target = target_user(&state, &actor, &id).await?;
    // Counts the target as well, but the target is being deleted rather than
    // demoted, so only the branch below them is worth reporting.
    let demoted = users::revoke_admin(&state.db, &target.id)
        .await?
        .saturating_sub(1);
    let links = users::delete_with_cascade(
        &state.db,
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
