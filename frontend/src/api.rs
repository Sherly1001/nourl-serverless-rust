use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use shared::{
    AdminOrphans, AdminSettings, AdminUserListResponse, ApiError, ApiErrorBody, AuthMethods,
    ChangePasswordRequest, DeleteAccountRequest, DeleteUserResponse, LoginRequest, RegisterRequest,
    SetAdminRequest, SetAdminResponse, UpdateProfileRequest, UpdateSettingsRequest, UrlEntry,
    UrlListResponse, UrlUpsertRequest, UserInfo,
};

fn net_err(err: impl std::fmt::Display) -> ApiErrorBody {
    ApiErrorBody {
        code: "network".into(),
        message: err.to_string(),
        field: None,
        conflict: None,
        rejected: None,
    }
}

/// The server's own error body if it sent one, else a synthesised network
/// error — a 502 from in front of the app has no JSON to parse.
async fn error_of(resp: Response) -> ApiErrorBody {
    let status = resp.status();
    match resp.json::<ApiError>().await {
        Ok(e) => e.error,
        Err(_) => net_err(format!("request failed with status {status}")),
    }
}

async fn read<T: DeserializeOwned>(resp: Response) -> Result<T, ApiErrorBody> {
    if resp.ok() {
        resp.json::<T>().await.map_err(net_err)
    } else {
        Err(error_of(resp).await)
    }
}

async fn send_json<B: Serialize, T: DeserializeOwned>(
    request: RequestBuilder,
    body: &B,
) -> Result<T, ApiErrorBody> {
    let resp = request
        .json(body)
        .map_err(net_err)?
        .send()
        .await
        .map_err(net_err)?;
    read(resp).await
}

async fn get_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiErrorBody> {
    let resp = Request::get(path).send().await.map_err(net_err)?;
    read(resp).await
}

/// A request with no body of its own whose response *is* worth reading — a
/// `DELETE` that reports what it did, for instance.
async fn read_json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, ApiErrorBody> {
    let resp = request.send().await.map_err(net_err)?;
    read(resp).await
}

/// A request whose response body we do not care about, only its success.
async fn send_empty(request: RequestBuilder) -> Result<(), ApiErrorBody> {
    let resp = request.send().await.map_err(net_err)?;
    if resp.ok() {
        Ok(())
    } else {
        Err(error_of(resp).await)
    }
}

pub async fn create_url(req: &UrlUpsertRequest) -> Result<UrlEntry, ApiErrorBody> {
    send_json(Request::post("/api/urls"), req).await
}

pub async fn update_url(code: &str, req: &UrlUpsertRequest) -> Result<UrlEntry, ApiErrorBody> {
    send_json(Request::put(&format!("/api/urls/{code}")), req).await
}

pub async fn delete_url(code: &str) -> Result<(), ApiErrorBody> {
    send_empty(Request::delete(&format!("/api/urls/{code}"))).await
}

/// `params` are the query parameters, which `gloo_net` encodes and appends —
/// see [`crate::list::list_params`] for why they are not spliced into the path.
///
/// Takes an abort signal because the search box refires on every pause in
/// typing: without one, a slow early request can land after a later one and
/// overwrite the newer results.
pub async fn list_urls(
    params: Vec<(&'static str, String)>,
    signal: Option<&web_sys::AbortSignal>,
) -> Result<UrlListResponse, ApiErrorBody> {
    let resp = Request::get("/api/urls")
        .query(params)
        .abort_signal(signal)
        .send()
        .await
        .map_err(net_err)?;
    read(resp).await
}

pub async fn register(body: &RegisterRequest) -> Result<UserInfo, ApiErrorBody> {
    send_json(Request::post("/api/auth/register"), body).await
}

pub async fn login(body: &LoginRequest) -> Result<UserInfo, ApiErrorBody> {
    send_json(Request::post("/api/auth/login"), body).await
}

/// The session cookie rides along automatically: `fetch` sends cookies for
/// same-origin requests, and the Trunk dev proxy keeps the API same-origin.
pub async fn logout() -> Result<(), ApiErrorBody> {
    send_empty(Request::post("/api/auth/logout")).await
}

pub async fn me() -> Result<UserInfo, ApiErrorBody> {
    get_json("/api/auth/me").await
}

pub async fn auth_methods() -> Result<AuthMethods, ApiErrorBody> {
    get_json("/api/auth/methods").await
}

/// Where a provider flow starts. A plain URL rather than a request: the browser
/// has to navigate there itself, since `fetch` cannot follow a cross-origin
/// redirect and the provider needs a top-level page to show its consent screen.
pub fn oauth_start(provider: &str) -> String {
    format!("/api/auth/{provider}")
}

pub async fn update_me(body: &UpdateProfileRequest) -> Result<UserInfo, ApiErrorBody> {
    send_json(Request::put("/api/auth/me"), body).await
}

pub async fn change_password(body: &ChangePasswordRequest) -> Result<UserInfo, ApiErrorBody> {
    send_json(Request::put("/api/auth/password"), body).await
}

/// Carries a body — the caller says what should happen to the links the account
/// owns, and confirms with their password if they have one.
pub async fn close_account(
    body: &DeleteAccountRequest,
) -> Result<DeleteUserResponse, ApiErrorBody> {
    send_json(Request::delete("/api/auth/me"), body).await
}

pub async fn disconnect_provider(provider: &str) -> Result<UserInfo, ApiErrorBody> {
    read_json(Request::delete(&format!("/api/auth/{provider}"))).await
}

/// `params` page and search the ordinary accounts only — the admins come back
/// whole in the same response, because the tree cannot be paged without losing
/// interior nodes.
pub async fn admin_users(
    params: Vec<(&'static str, String)>,
) -> Result<AdminUserListResponse, ApiErrorBody> {
    read_json(Request::get("/api/admin/users").query(params)).await
}

/// Promotes, demotes, or moves — the server treats all three as one write to
/// the parent pointer. `parent` names who they hang under; `None` means the
/// caller, which is the ordinary promotion.
/// `orphans` decides what happens to the admins this account promoted, and is
/// read by the server only on a demotion.
pub async fn set_user_admin(
    id: &str,
    is_admin: bool,
    parent: Option<String>,
    orphans: AdminOrphans,
) -> Result<SetAdminResponse, ApiErrorBody> {
    send_json(
        Request::put(&format!("/api/admin/users/{id}")),
        &SetAdminRequest {
            is_admin,
            promoted_by: parent,
            orphans,
        },
    )
    .await
}

/// The response says how many links were orphaned and how long they have left,
/// which is what the confirmation reports back. `orphans` decides what happens
/// to the admins this account had promoted.
pub async fn delete_user(
    id: &str,
    orphans: AdminOrphans,
) -> Result<DeleteUserResponse, ApiErrorBody> {
    let choice = match orphans {
        AdminOrphans::Demote => "demote",
        AdminOrphans::Reparent => "reparent",
    };
    read_json(Request::delete(&format!("/api/admin/users/{id}")).query([("orphans", choice)])).await
}

pub async fn admin_settings() -> Result<AdminSettings, ApiErrorBody> {
    get_json("/api/admin/settings").await
}

pub async fn save_admin_settings(
    body: &UpdateSettingsRequest,
) -> Result<AdminSettings, ApiErrorBody> {
    send_json(Request::put("/api/admin/settings"), body).await
}
