mod helpers;

use axum::http::StatusCode;
use helpers::{body_json, json_request, session_cookie, test_app};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn register_creates_an_account_and_logs_in() {
    let (app, db) = test_app().await;

    let response = app
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "alice", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        session_cookie(&response).is_some(),
        "register must log the user in"
    );
    let body = body_json(response).await;
    assert_eq!(body["username"], "alice");
    assert_eq!(body["is_admin"], false);
    assert!(body.get("hash_passwd").is_none(), "never leak the hash");

    // The stored password must be an argon2 PHC string, not the plaintext.
    let stored = db
        .collection::<mongodb::bson::Document>("users")
        .find_one(mongodb::bson::doc! {"username": "alice"})
        .await
        .unwrap()
        .unwrap();
    let hash = stored.get_str("hash_passwd").unwrap();
    assert!(hash.starts_with("$argon2id$"), "not hashed: {hash}");
    assert!(!hash.contains("hunter2hunter2"));

    db.drop().await.unwrap();
}

#[tokio::test]
async fn register_rejects_short_credentials_and_duplicates() {
    let (app, db) = test_app().await;

    for (username, password) in [("ab", "hunter2hunter2"), ("alice", "short")] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/auth/register",
                json!({"username": username, "password": password}),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{username}/{password}"
        );
    }

    let ok = json!({"username": "dup", "password": "hunter2hunter2"});
    let first = app
        .clone()
        .oneshot(json_request("POST", "/api/auth/register", ok.clone()))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(json_request("POST", "/api/auth/register", ok))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn login_accepts_the_right_password_only() {
    let (app, db) = test_app().await;
    app.clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "bob", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();

    let good = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "bob", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(good.status(), StatusCode::OK);
    assert!(session_cookie(&good).is_some());

    for body in [
        json!({"username": "bob", "password": "wrong-password"}),
        json!({"username": "ghost", "password": "hunter2hunter2"}),
    ] {
        let bad = app
            .clone()
            .oneshot(json_request("POST", "/api/auth/login", body))
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
        let message = body_json(bad).await["error"]["message"].to_string();
        assert!(
            message.contains("username or password"),
            "must not reveal which half was wrong: {message}"
        );
    }

    db.drop().await.unwrap();
}

#[tokio::test]
async fn me_requires_a_session_and_returns_the_account() {
    let (app, db) = test_app().await;

    let anonymous = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/auth/me")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "carol", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let cookie = session_cookie(&registered).unwrap();

    let me = app
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body = body_json(me).await;
    assert_eq!(body["username"], "carol");
    assert_eq!(body["display_name"], "carol");
    assert!(body.get("hash_passwd").is_none());

    db.drop().await.unwrap();
}

#[tokio::test]
async fn logout_revokes_every_outstanding_token() {
    let (app, db) = test_app().await;
    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "dave", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let cookie = session_cookie(&registered).unwrap();

    let logout = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/auth/logout")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::OK);

    // The very same token must now be dead — this is the token_version bump,
    // not merely the cookie being cleared in the browser.
    let reused = app
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn methods_is_public_and_reports_password_only_by_default() {
    let (app, db) = test_app().await;

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/auth/methods")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["password"], true);
    assert_eq!(body["github"], false);
    assert_eq!(body["google"], false);
    assert_eq!(body["facebook"], false);

    db.drop().await.unwrap();
}
