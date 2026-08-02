use axum::Json;
use axum::extract::{Path, Query, State};
use shared::{AdminUserListResponse, SetAdminRequest, SetAdminResponse};

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::error::AppError;
use crate::extract::AppJson;
use crate::query::UserListParams;
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

/// Refusing to act on your own account is what keeps an admin from demoting
/// themselves — which, for a root, would lock the pages away for everyone.
fn not_yourself(actor: &User, target_id: &str) -> Result<(), AppError> {
    if actor.id == target_id {
        return Err(AppError::validation(
            "you cannot do that to your own account",
        ));
    }
    Ok(())
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
