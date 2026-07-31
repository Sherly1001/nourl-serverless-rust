mod helpers;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use helpers::test_app;
use http_body_util::BodyExt;
use mongodb::bson::doc;
use serde_json::{Value, json};
use tower::ServiceExt;

fn req(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(uri);
    match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn create_roundtrip() {
    let (app, db) = test_app().await;
    let resp = app
        .oneshot(req(
            Method::POST,
            "/api/urls",
            Some(json!({"code": "hi", "url": "https://a.com"})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["code"], "hi");
    assert_eq!(v["url"], "https://a.com");
    assert!(v["owner"].is_null());
    let stored = db
        .collection::<mongodb::bson::Document>("urls")
        .find_one(doc! {"code": "hi"})
        .await
        .unwrap();
    assert!(stored.is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
async fn create_rejects_invalid_input() {
    let (app, db) = test_app().await;
    for bad in [
        json!({"code": "h/i", "url": "https://a.com"}),
        json!({"code": "hi", "url": "ftp://a.com"}),
        json!({"code": "", "url": "https://a.com"}),
        json!({"code": "hi", "url": "https://a.com", "expires_at": "2001-01-01T00:00:00Z"}),
        json!({"code": "hi", "url": "https://a.com", "expires_at": "not-a-date"}),
    ] {
        let resp = app
            .clone()
            .oneshot(req(Method::POST, "/api/urls", Some(bad)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "validation");
    }
    db.drop().await.unwrap();
}

#[tokio::test]
async fn put_renames_code() {
    let (app, db) = test_app().await;
    db.collection("urls")
        .insert_one(doc! {"code": "a", "url": "https://a.com"})
        .await
        .unwrap();
    let resp = app
        .oneshot(req(
            Method::PUT,
            "/api/urls/a",
            Some(json!({"code": "b", "url": "https://b.com"})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["code"], "b");
    db.drop().await.unwrap();
}

#[tokio::test]
async fn owned_urls_are_protected() {
    let (app, db) = test_app().await;
    db.collection("urls")
        .insert_one(doc! {"code": "own", "url": "https://a.com", "owner": "u1"})
        .await
        .unwrap();
    for r in [
        req(
            Method::POST,
            "/api/urls",
            Some(json!({"code": "own", "url": "https://x.com"})),
        ),
        req(
            Method::PUT,
            "/api/urls/own",
            Some(json!({"code": "own", "url": "https://x.com"})),
        ),
        req(Method::DELETE, "/api/urls/own", None),
    ] {
        let resp = app.clone().oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "forbidden");
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains("'own'"), "{msg}");
        // target url must not leak to anonymous callers / non-owners
        assert!(!msg.contains("https://a.com"), "{msg}");
    }
    db.drop().await.unwrap();
}

#[tokio::test]
async fn delete_removes_and_404s_on_missing() {
    let (app, db) = test_app().await;
    db.collection("urls")
        .insert_one(doc! {"code": "gone", "url": "https://a.com"})
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(req(Method::DELETE, "/api/urls/gone", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v, json!({"code": "gone", "deleted": true}));
    let resp = app
        .oneshot(req(Method::DELETE, "/api/urls/gone", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    db.drop().await.unwrap();
}

#[tokio::test]
async fn list_returns_501_in_phase_1() {
    let (app, db) = test_app().await;
    let resp = app
        .oneshot(req(Method::GET, "/api/urls", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    db.drop().await.unwrap();
}
