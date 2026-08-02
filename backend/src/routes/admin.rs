use axum::Json;
use axum::extract::{Query, State};
use shared::{AdminUserInfo, AdminUserListResponse};

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::error::AppError;
use crate::query::UserListParams;
use crate::users;

/// Every account, newest first. `AdminUser` is the whole authorisation check —
/// see `auth::extract::AdminUser`.
pub async fn list_users(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<AdminUserListResponse>, AppError> {
    let parsed = UserListParams::from_query(&params)?;
    let items: Vec<AdminUserInfo> = users::list(&state.db, &parsed).await?;
    let total = users::count(&state.db, parsed.filter()).await?;
    Ok(Json(AdminUserListResponse { items, total }))
}
