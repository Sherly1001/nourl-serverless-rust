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

#[tokio::test]
async fn changing_password_requires_the_current_one() {
    let (app, db) = test_app().await;
    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "rotator", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let cookie = session_cookie(&registered).unwrap();

    let wrong = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/password",
            &cookie,
            json!({"current_password": "not-it", "new_password": "brandnewpass"}),
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let short = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/password",
            &cookie,
            json!({"current_password": "hunter2hunter2", "new_password": "short"}),
        ))
        .await
        .unwrap();
    assert_eq!(short.status(), StatusCode::BAD_REQUEST);

    // Neither failure may have changed anything.
    let still_works = app
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "rotator", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(still_works.status(), StatusCode::OK);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn changing_password_logs_out_every_other_session() {
    let (app, db) = test_app().await;
    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "rotator2", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let old_cookie = session_cookie(&registered).unwrap();

    let changed = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/password",
            &old_cookie,
            json!({"current_password": "hunter2hunter2", "new_password": "brandnewpass"}),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);

    // This device is re-authenticated, so it keeps working.
    let fresh_cookie = session_cookie(&changed).expect("must hand back a new session");
    assert_ne!(fresh_cookie, old_cookie);
    let with_fresh = app
        .clone()
        .oneshot(helpers::authed_get("/api/auth/me", &fresh_cookie))
        .await
        .unwrap();
    assert_eq!(with_fresh.status(), StatusCode::OK);

    // Any other device holding the old token is locked out.
    let with_old = app
        .clone()
        .oneshot(helpers::authed_get("/api/auth/me", &old_cookie))
        .await
        .unwrap();
    assert_eq!(with_old.status(), StatusCode::UNAUTHORIZED);

    // The new password is the one that works now.
    let old_password = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "rotator2", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(old_password.status(), StatusCode::UNAUTHORIZED);

    let new_password = app
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "rotator2", "password": "brandnewpass"}),
        ))
        .await
        .unwrap();
    assert_eq!(new_password.status(), StatusCode::OK);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn profile_edits_apply_and_survive_a_reload() {
    let (app, db) = test_app().await;
    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "editor", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let cookie = session_cookie(&registered).unwrap();

    let updated = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/me",
            &cookie,
            json!({"display_name": "The Editor", "email": "e@example.com"}),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let body = body_json(updated).await;
    assert_eq!(body["display_name"], "The Editor");
    assert_eq!(body["email"], "e@example.com");
    assert_eq!(body["username"], "editor", "username is not editable here");

    // An omitted field must not be cleared.
    let renamed = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/me",
            &cookie,
            json!({"display_name": "Renamed"}),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(renamed).await["email"], "e@example.com");

    let me = app
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(body_json(me).await["display_name"], "Renamed");

    db.drop().await.unwrap();
}

#[tokio::test]
async fn profile_edit_requires_a_session() {
    let (app, db) = test_app().await;
    let response = app
        .oneshot(json_request(
            "PUT",
            "/api/auth/me",
            json!({"display_name": "nobody"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    db.drop().await.unwrap();
}
