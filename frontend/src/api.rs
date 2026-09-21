use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use shared::{
    AdminOrphans, AdminSettings, AdminUserListResponse, ApiError, ApiErrorBody, AuthMethods,
    BulkAction, BulkUrlsRequest, BulkUrlsResponse, BulkUsersRequest, BulkUsersResponse,
    ChangePasswordRequest, DeleteAccountRequest, DeleteUserResponse, LoginRequest, RegisterRequest,
    SetAdminRequest, SetAdminResponse, UpdateProfileRequest, UpdateSettingsRequest, UrlBulkAction,
    UrlEntry, UrlListResponse, UrlUpsertRequest, UserInfo,
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

/// The server's error body, else a synthesised one: a 502 from in front of the
/// app has no JSON to parse.
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

/// No request body, but a response worth reading — a `DELETE` that reports.
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

/// Admin-only, and an owned link may only be taken from below the caller in
/// the chain — which the page cannot work out, since the list sends a name and
/// nothing about where they sit. So the server is what refuses.
pub async fn claim_url(code: &str) -> Result<UrlEntry, ApiErrorBody> {
    read_json(Request::post(&format!("/api/urls/{code}/claim"))).await
}

/// `params` are appended by `gloo_net` — see [`crate::list::list_params`].
/// The abort signal matters because the search box refires on every pause: a
/// slow early request would otherwise land after a newer one.
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

/// The cookie rides along: `fetch` sends it same-origin, which the Trunk dev
/// proxy preserves.
pub async fn logout() -> Result<(), ApiErrorBody> {
    send_empty(Request::post("/api/auth/logout")).await
}

pub async fn me() -> Result<UserInfo, ApiErrorBody> {
    get_json("/api/auth/me").await
}

pub async fn auth_methods() -> Result<AuthMethods, ApiErrorBody> {
    get_json("/api/auth/methods").await
}

/// A URL, not a request: `fetch` cannot follow a cross-origin redirect, and
/// the provider needs a top-level page for its consent screen.
pub fn oauth_start(provider: &str) -> String {
    format!("/api/auth/{provider}")
}

pub async fn update_me(body: &UpdateProfileRequest) -> Result<UserInfo, ApiErrorBody> {
    send_json(Request::put("/api/auth/me"), body).await
}

pub async fn change_password(body: &ChangePasswordRequest) -> Result<UserInfo, ApiErrorBody> {
    send_json(Request::put("/api/auth/password"), body).await
}

/// The body says what happens to the links, and carries the password.
pub async fn close_account(
    body: &DeleteAccountRequest,
) -> Result<DeleteUserResponse, ApiErrorBody> {
    send_json(Request::delete("/api/auth/me"), body).await
}

pub async fn disconnect_provider(provider: &str) -> Result<UserInfo, ApiErrorBody> {
    read_json(Request::delete(&format!("/api/auth/{provider}"))).await
}

/// `params` page the ordinary accounts only; the admins come back whole, since
/// paging a tree loses its interior nodes.
pub async fn admin_users(
    params: Vec<(&'static str, String)>,
) -> Result<AdminUserListResponse, ApiErrorBody> {
    read_json(Request::get("/api/admin/users").query(params)).await
}

/// Promote, demote or move: one write to the parent pointer. `None` for
/// `parent` means the caller. `orphans` is read only on a demotion.
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

/// The response counts the orphaned links and their deadline, which the
/// confirmation reports back.
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

/// A whole selection of links, in as few requests as the cap allows. The
/// server is all-or-nothing within a request, so a split selection can land in
/// part — which is why an error carries what the earlier chunks did.
pub async fn bulk_urls(
    codes: Vec<String>,
    action: UrlBulkAction,
) -> Result<BulkUrlsResponse, (BulkUrlsResponse, ApiErrorBody)> {
    let mut total = BulkUrlsResponse::default();
    for chunk in codes.chunks(BULK_CHUNK) {
        let request = BulkUrlsRequest {
            codes: chunk.to_vec(),
            action,
        };
        match send_json::<_, BulkUrlsResponse>(Request::post("/api/urls/bulk"), &request).await {
            Ok(response) => {
                total.affected += response.affected;
                total.entries.extend(response.entries);
            }
            Err(err) => return Err((total, err)),
        }
    }
    Ok(total)
}

/// Mirrors the server's own `BULK_MAX`. A longer selection is sent in several,
/// which is where the all-or-nothing guarantee stops: it holds per request.
pub const BULK_CHUNK: usize = 100;

/// A whole selection, in as few requests as the cap allows. Within a chunk the
/// server is all-or-nothing; across chunks it is not, so an error carries the
/// counts from the chunks that did land.
pub async fn bulk_users(
    ids: Vec<String>,
    action: BulkAction,
    orphans: AdminOrphans,
) -> Result<BulkUsersResponse, (BulkUsersResponse, ApiErrorBody)> {
    let mut total = BulkUsersResponse::default();
    for chunk in ids.chunks(BULK_CHUNK) {
        let request = BulkUsersRequest {
            ids: chunk.to_vec(),
            action,
            orphans,
        };
        match send_json::<_, BulkUsersResponse>(Request::post("/api/admin/users/bulk"), &request)
            .await
        {
            Ok(response) => {
                total.affected += response.affected;
                total.demoted += response.demoted;
                total.reparented += response.reparented;
                total.orphaned += response.orphaned;
                total.grace_days = response.grace_days;
            }
            Err(err) => return Err((total, err)),
        }
    }
    Ok(total)
}

pub async fn admin_settings() -> Result<AdminSettings, ApiErrorBody> {
    get_json("/api/admin/settings").await
}

pub async fn save_admin_settings(
    body: &UpdateSettingsRequest,
) -> Result<AdminSettings, ApiErrorBody> {
    send_json(Request::put("/api/admin/settings"), body).await
}
