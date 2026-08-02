mod helpers;

use axum::http::StatusCode;
use helpers::{authed_get, body_json, json_request, request, session_cookie, test_app};
use serde_json::json;
use tower::ServiceExt;

/// Registers an account and returns its session cookie.
async fn account(app: &axum::Router, username: &str) -> String {
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": username, "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    session_cookie(&response).unwrap()
}

#[tokio::test]
async fn admin_routes_are_closed_to_anonymous_and_ordinary_users() {
    let (app, db) = test_app().await;
    let plain = account(&app, "ordinary").await;

    let anonymous = app
        .clone()
        .oneshot(request("GET", "/api/admin/users"))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let ordinary = app
        .oneshot(authed_get("/api/admin/users", &plain))
        .await
        .unwrap();
    assert_eq!(ordinary.status(), StatusCode::FORBIDDEN);
    let body = body_json(ordinary).await;
    assert_eq!(body["error"]["code"], "forbidden");

    db.drop().await.unwrap();
}
