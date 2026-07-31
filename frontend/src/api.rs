use gloo_net::http::Request;
use shared::{ApiError, ApiErrorBody, UrlEntry, UrlUpsertRequest};

fn net_err(err: impl std::fmt::Display) -> ApiErrorBody {
    ApiErrorBody {
        code: "network".into(),
        message: err.to_string(),
    }
}

pub async fn create_url(req: &UrlUpsertRequest) -> Result<UrlEntry, ApiErrorBody> {
    let resp = Request::post("/api/urls")
        .json(req)
        .map_err(net_err)?
        .send()
        .await
        .map_err(net_err)?;
    if resp.ok() {
        resp.json::<UrlEntry>().await.map_err(net_err)
    } else {
        match resp.json::<ApiError>().await {
            Ok(e) => Err(e.error),
            Err(_) => Err(net_err(format!(
                "request failed with status {}",
                resp.status()
            ))),
        }
    }
}
