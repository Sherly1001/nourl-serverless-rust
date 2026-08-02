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

/// The UI renders a message under the input it names, so which errors carry a
/// `field` — and which deliberately do not — is part of the contract.
#[tokio::test]
async fn errors_name_the_field_they_are_about() {
    let (app, db) = test_app().await;
    let taken = json!({"username": "namesake", "password": "hunter2hunter2"});
    app.clone()
        .oneshot(json_request("POST", "/api/auth/register", taken.clone()))
        .await
        .unwrap();

    for (body, field) in [
        (taken, "username"),
        (
            json!({"username": "sh", "password": "hunter2hunter2"}),
            "username",
        ),
        (
            json!({"username": "goodname", "password": "no"}),
            "password",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(json_request("POST", "/api/auth/register", body))
            .await
            .unwrap();
        assert_eq!(body_json(response).await["error"]["field"], field);
    }

    // A failed login must not say which half was wrong, so it names no field.
    let failed = app
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "namesake", "password": "wrong-password"}),
        ))
        .await
        .unwrap();
    let body = body_json(failed).await;
    assert!(
        body["error"].get("field").is_none(),
        "a failed login must not point at a field: {body}"
    );

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
async fn username_can_be_renamed_but_not_onto_a_taken_one() {
    let (app, db) = test_app().await;
    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "before", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let cookie = session_cookie(&registered).unwrap();
    app.clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": "occupied", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();

    for (body, status) in [
        (json!({"username": "occupied"}), StatusCode::CONFLICT),
        (json!({"username": "no"}), StatusCode::BAD_REQUEST),
        (json!({"username": "has space"}), StatusCode::BAD_REQUEST),
    ] {
        let rejected = app
            .clone()
            .oneshot(helpers::authed_request(
                "PUT",
                "/api/auth/me",
                &cookie,
                body.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), status, "{body}");
    }

    // Re-sending the current name is a no-op, not a collision with itself.
    let unchanged = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/me",
            &cookie,
            json!({"username": "before"}),
        ))
        .await
        .unwrap();
    assert_eq!(unchanged.status(), StatusCode::OK);

    let renamed = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/me",
            &cookie,
            json!({"username": "after"}),
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    assert_eq!(body_json(renamed).await["username"], "after");

    // The session survives the rename, and the new name is the login handle.
    let still_authed = app
        .clone()
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(still_authed.status(), StatusCode::OK);

    let old_name = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "before", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(old_name.status(), StatusCode::UNAUTHORIZED);

    let new_name = app
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "after", "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(new_name.status(), StatusCode::OK);

    db.drop().await.unwrap();
}

/// An account with no password — what phase 2b's OAuth signups produce — sets
/// its first one with the session cookie alone.
#[tokio::test]
async fn a_passwordless_account_can_set_a_first_password() {
    let (app, db) = test_app().await;
    let user = backend::users::create(
        &db,
        backend::users::NewUser {
            username: "providerkid".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let cookie = helpers::cookie_for(&user);

    let me = app
        .clone()
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(body_json(me).await["has_password"], false);

    // No current_password, because there is none to give.
    let set = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/password",
            &cookie,
            json!({"new_password": "firstpassword"}),
        ))
        .await
        .unwrap();
    assert_eq!(set.status(), StatusCode::OK);
    assert_eq!(body_json(set).await["has_password"], true);

    let login = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            json!({"username": "providerkid", "password": "firstpassword"}),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);

    // Now that one exists, the check is back on.
    let second = app
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/auth/password",
            &session_cookie(&login).unwrap(),
            json!({"new_password": "secondpassword"}),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);

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

/// Registers an account, gives it a link, and returns its session cookie.
async fn account_with_a_link(app: &axum::Router, username: &str, code: &str) -> String {
    let registered = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/auth/register",
            json!({"username": username, "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    let cookie = session_cookie(&registered).unwrap();
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": code, "url": "https://example.com"}),
        ))
        .await
        .unwrap();
    cookie
}

/// The owner is the only one who may decide their links should stop resolving.
#[tokio::test]
async fn closing_your_account_can_take_your_links_with_it() {
    let (app, db) = test_app().await;
    let cookie = account_with_a_link(&app, "quitter", "taking-it").await;

    let closed = app
        .clone()
        .oneshot(helpers::authed_request(
            "DELETE",
            "/api/auth/me",
            &cookie,
            json!({"links": "delete", "current_password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let body = body_json(closed).await;
    assert_eq!(body["links_deleted"], 1);
    assert_eq!(body["orphaned"], 0);

    let urls = db.collection::<mongodb::bson::Document>("urls");
    assert!(
        urls.find_one(mongodb::bson::doc! {"code": "taking-it"})
            .await
            .unwrap()
            .is_none(),
        "the link must be gone, not orphaned"
    );
    // The session cannot outlive the account it belonged to.
    let after = app
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);

    db.drop().await.unwrap();
}

/// The other choice: the links keep working for whoever is already using them.
#[tokio::test]
async fn closing_your_account_can_leave_your_links_running() {
    let (app, db) = test_app().await;
    let cookie = account_with_a_link(&app, "leaver", "leaving-it").await;

    let closed = app
        .clone()
        .oneshot(helpers::authed_request(
            "DELETE",
            "/api/auth/me",
            &cookie,
            json!({"links": "orphan", "current_password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let body = body_json(closed).await;
    assert_eq!(body["orphaned"], 1);
    assert_eq!(body["links_deleted"], 0);
    assert_eq!(body["grace_days"], 7);

    // Still resolving, and now claimable by anyone.
    let followed = app
        .clone()
        .oneshot(helpers::request("GET", "/leaving-it"))
        .await
        .unwrap();
    assert_eq!(followed.status(), StatusCode::FOUND);

    let row = db
        .collection::<mongodb::bson::Document>("urls")
        .find_one(mongodb::bson::doc! {"code": "leaving-it"})
        .await
        .unwrap()
        .unwrap();
    assert!(row.get("owner").is_none(), "must be unowned");
    assert!(
        row.get_datetime("expires_at").is_ok(),
        "must be on a deadline"
    );

    db.drop().await.unwrap();
}

/// Irreversible, so the session alone is not enough to trigger it.
#[tokio::test]
async fn closing_your_account_needs_your_password() {
    let (app, db) = test_app().await;
    let cookie = account_with_a_link(&app, "careful", "still-here").await;

    for body in [
        json!({"links": "delete"}),
        json!({"links": "delete", "current_password": "not-my-password"}),
    ] {
        let refused = app
            .clone()
            .oneshot(helpers::authed_request(
                "DELETE",
                "/api/auth/me",
                &cookie,
                body,
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_json(refused).await["error"]["field"],
            "current_password",
            "the UI needs to know which input to mark"
        );
    }

    // Nothing was touched.
    let still_there = app
        .clone()
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(still_there.status(), StatusCode::OK);
    assert!(
        db.collection::<mongodb::bson::Document>("urls")
            .find_one(mongodb::bson::doc! {"code": "still-here"})
            .await
            .unwrap()
            .is_some()
    );

    // And an omitted disposition is a 400, not a guess at what they meant.
    let vague = app
        .oneshot(helpers::authed_request(
            "DELETE",
            "/api/auth/me",
            &cookie,
            json!({"current_password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    assert_eq!(vague.status(), StatusCode::BAD_REQUEST);

    db.drop().await.unwrap();
}

/// A root closing their own account would cascade the demotion through every
/// admin below them and leave nobody able to reach the admin pages.
#[tokio::test]
async fn an_admin_cannot_close_their_own_account() {
    let (app, db) = test_app().await;
    let cookie = account_with_a_link(&app, "the-admin", "admins-link").await;
    // Promoted rather than seeded — `promoted_by` is what marks a root, and a
    // root has a different refusal because it can never resign.
    db.collection::<mongodb::bson::Document>("users")
        .update_one(
            mongodb::bson::doc! {"username": "the-admin"},
            mongodb::bson::doc! {"$set": {"is_admin": true, "promoted_by": "someone-else"}},
        )
        .await
        .unwrap();

    let close = || {
        let (app, cookie) = (app.clone(), cookie.clone());
        async move {
            app.oneshot(helpers::authed_request(
                "DELETE",
                "/api/auth/me",
                &cookie,
                json!({"links": "orphan", "current_password": "hunter2hunter2"}),
            ))
            .await
            .unwrap()
        }
    };

    let refused = close().await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    let still_there = app
        .clone()
        .oneshot(helpers::authed_get("/api/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(still_there.status(), StatusCode::OK);

    // Resigning is the way out, and then the account can be closed. This one
    // was promoted rather than seeded, so it is allowed to resign.
    let id = body_json(still_there).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resigned = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            &format!("/api/admin/users/{id}"),
            &cookie,
            json!({"is_admin": false}),
        ))
        .await
        .unwrap();
    assert_eq!(resigned.status(), StatusCode::OK);
    assert_eq!(close().await.status(), StatusCode::OK);

    db.drop().await.unwrap();
}
