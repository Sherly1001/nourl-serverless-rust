use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use shared::{
    ApiError, ApiErrorBody, AuthMethods, LoginRequest, RegisterRequest, UrlEntry, UrlListResponse,
    UrlUpsertRequest, UserInfo,
};

fn net_err(err: impl std::fmt::Display) -> ApiErrorBody {
    ApiErrorBody {
        code: "network".into(),
        message: err.to_string(),
        field: None,
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

/// `query` is the already-encoded query string, without the leading `?`.
///
/// Takes an abort signal because the search box refires on every pause in
/// typing: without one, a slow early request can land after a later one and
/// overwrite the newer results.
pub async fn list_urls(
    query: &str,
    signal: Option<&web_sys::AbortSignal>,
) -> Result<UrlListResponse, ApiErrorBody> {
    let resp = Request::get(&format!("/api/urls?{query}"))
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
