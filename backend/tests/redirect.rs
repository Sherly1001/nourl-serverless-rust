mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use helpers::{FALLBACK, test_app, test_app_with};
use mongodb::bson::doc;
use tower::ServiceExt;

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn redirects_302_to_stored_url() {
    let (app, db) = test_app().await;
    db.collection("urls")
        .insert_one(doc! {"code": "hi", "url": "https://target.example/x"})
        .await
        .unwrap();
    let resp = app.oneshot(get("/hi")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://target.example/x");
    db.drop().await.unwrap();
}

#[tokio::test]
async fn missing_code_falls_back_to_env_override() {
    let (app, db) = test_app().await;
    let resp = app.oneshot(get("/nope")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], FALLBACK);
    db.drop().await.unwrap();
}

#[tokio::test]
async fn missing_code_without_override_falls_back_to_request_host() {
    let (app, db) = test_app_with(None).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/nope")
                .header("host", "nourl.space")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.headers()["location"], "https://nourl.space");

    // x-forwarded-host (set by CloudFront) wins over host (set by API GW)
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/nope")
                .header("host", "abc123.execute-api.amazonaws.com")
                .header("x-forwarded-host", "dev.nourl.space")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.headers()["location"], "https://dev.nourl.space");
    db.drop().await.unwrap();
}

#[tokio::test]
async fn expired_code_falls_back() {
    let (app, db) = test_app().await;
    let past = bson::DateTime::from_millis(bson::DateTime::now().timestamp_millis() - 60_000);
    db.collection("urls")
        .insert_one(doc! {"code": "old", "url": "https://target.example", "expires_at": past})
        .await
        .unwrap();
    let resp = app.oneshot(get("/old")).await.unwrap();
    assert_eq!(resp.headers()["location"], FALLBACK);
    db.drop().await.unwrap();
}

#[tokio::test]
async fn root_redirects_to_index_html() {
    let (app, db) = test_app().await;
    let resp = app.oneshot(get("/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "/index.html");
    db.drop().await.unwrap();
}

#[tokio::test]
async fn multi_segment_path_falls_back_and_api_404s() {
    let (app, db) = test_app().await;
    let resp = app.clone().oneshot(get("/a/b/c")).await.unwrap();
    assert_eq!(resp.headers()["location"], FALLBACK);
    let resp = app.oneshot(get("/api/nope")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    db.drop().await.unwrap();
}

#[tokio::test]
async fn unknown_api_route_returns_json_error_shape() {
    let (app, db) = test_app().await;
    let resp = app.oneshot(get("/api/does/not/exist")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"], "not_found");
    db.drop().await.unwrap();
}

#[tokio::test]
async fn wrong_method_returns_json_405() {
    let (app, db) = test_app().await;
    // DELETE /api/urls has no handler (only POST/GET on that path)
    let resp = app
        .oneshot(
            Request::builder()
                .method(axum::http::Method::DELETE)
                .uri("/api/urls")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"], "method_not_allowed");
    db.drop().await.unwrap();
}
