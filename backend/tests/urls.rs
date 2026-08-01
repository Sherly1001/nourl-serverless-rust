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
async fn malformed_body_returns_json_validation_error() {
    let (app, db) = test_app().await;
    // missing `url` field
    let resp = app
        .clone()
        .oneshot(req(
            Method::POST,
            "/api/urls",
            Some(json!({"code": "lmao"})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "validation");
    // invalid json body
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/urls")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "validation");
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

/// Registers a user and returns their session cookie.
async fn account(app: &axum::Router, username: &str) -> String {
    let response = app
        .clone()
        .oneshot(helpers::json_request(
            "POST",
            "/api/auth/register",
            json!({"username": username, "password": "hunter2hunter2"}),
        ))
        .await
        .unwrap();
    helpers::session_cookie(&response).unwrap()
}

#[tokio::test]
async fn creating_while_logged_in_takes_ownership() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "owner1").await;

    let created = app
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "mine", "url": "https://example.com"}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(body_json(created).await["owner"]["username"], "owner1");

    db.drop().await.unwrap();
}

#[tokio::test]
async fn recreating_your_own_code_is_a_409_that_reveals_the_url() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "owner2").await;
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "dup", "url": "https://first.example"}),
        ))
        .await
        .unwrap();

    let again = app
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "dup", "url": "https://second.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::CONFLICT);
    let message = body_json(again).await["error"]["message"].to_string();
    assert!(
        message.contains("https://first.example"),
        "the owner is allowed to see their own target: {message}"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn other_peoples_codes_are_403_without_leaking_the_url() {
    let (app, db) = test_app().await;
    let owner = account(&app, "owner3").await;
    let other = account(&app, "other3").await;
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "theirs", "url": "https://secret.example"}),
        ))
        .await
        .unwrap();

    let attempt = json!({"code": "theirs", "url": "https://x.example"});
    for request in [
        helpers::authed_request("POST", "/api/urls", &other, attempt.clone()),
        helpers::authed_request("PUT", "/api/urls/theirs", &other, attempt.clone()),
        helpers::authed_request("DELETE", "/api/urls/theirs", &other, json!({})),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let message = body_json(response).await["error"]["message"].to_string();
        assert!(
            !message.contains("secret"),
            "leaked the target url: {message}"
        );
    }

    db.drop().await.unwrap();
}

#[tokio::test]
async fn owners_may_edit_and_delete_their_own_links() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "owner4").await;
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "editable", "url": "https://before.example"}),
        ))
        .await
        .unwrap();

    let edited = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/editable",
            &cookie,
            json!({"code": "editable", "url": "https://after.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    let edited = body_json(edited).await;
    assert_eq!(edited["url"], "https://after.example");
    assert_eq!(
        edited["owner"]["username"], "owner4",
        "an edit must not drop ownership"
    );

    // Renaming the code carries the link — and its ownership — across.
    let renamed = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/editable",
            &cookie,
            json!({"code": "renamed", "url": "https://after.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = body_json(renamed).await;
    assert_eq!(renamed["code"], "renamed");
    assert_eq!(renamed["owner"]["username"], "owner4");

    // The old code is gone rather than duplicated.
    let old_code = app
        .clone()
        .oneshot(req(Method::DELETE, "/api/urls/editable", None))
        .await
        .unwrap();
    assert_eq!(old_code.status(), StatusCode::NOT_FOUND);

    let removed = app
        .oneshot(helpers::authed_request(
            "DELETE",
            "/api/urls/renamed",
            &cookie,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);

    db.drop().await.unwrap();
}

/// A rename is checked at *both* ends. Without the destination check the write
/// hits the unique index on `code` and surfaces as a 500.
#[tokio::test]
async fn a_rename_may_not_land_on_a_code_that_is_taken() {
    let (app, db) = test_app().await;
    let mover = account(&app, "mover7").await;
    let other = account(&app, "other7").await;
    for (cookie, code, url) in [
        (&mover, "movable", "https://mine.example"),
        (&other, "occupied", "https://theirs.example"),
    ] {
        app.clone()
            .oneshot(helpers::authed_request(
                "POST",
                "/api/urls",
                cookie,
                json!({"code": code, "url": url}),
            ))
            .await
            .unwrap();
    }
    db.collection("urls")
        .insert_one(doc! {"code": "anonymous", "url": "https://free.example"})
        .await
        .unwrap();

    // Someone else's code: 403, and still no url leak.
    let onto_theirs = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/movable",
            &mover,
            json!({"code": "occupied", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(onto_theirs.status(), StatusCode::FORBIDDEN);
    let message = body_json(onto_theirs).await["error"]["message"].to_string();
    assert!(!message.contains("theirs.example"), "leaked: {message}");

    // Your own code: the same answer as re-creating it, rather than silently
    // destroying the other link.
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &mover,
            json!({"code": "alsomine", "url": "https://other.example"}),
        ))
        .await
        .unwrap();
    let onto_own = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/movable",
            &mover,
            json!({"code": "alsomine", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(onto_own.status(), StatusCode::CONFLICT);

    // The rejections left every document where it was.
    for code in ["movable", "occupied", "anonymous", "alsomine"] {
        let count = db
            .collection::<mongodb::bson::Document>("urls")
            .count_documents(doc! {"code": code})
            .await
            .unwrap();
        assert_eq!(count, 1, "{code}");
    }

    db.drop().await.unwrap();
}

/// An unowned link is already overwritable and deletable by anyone, so a
/// rename may take its code — the caller could reach the same state with a
/// DELETE and then this rename.
#[tokio::test]
async fn a_rename_takes_over_an_unowned_code() {
    let (app, db) = test_app().await;
    let mover = account(&app, "mover8").await;
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &mover,
            json!({"code": "movable", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    db.collection("urls")
        .insert_one(doc! {"code": "anonymous", "url": "https://free.example"})
        .await
        .unwrap();

    let moved = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/movable",
            &mover,
            json!({"code": "anonymous", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    let moved = body_json(moved).await;
    assert_eq!(moved["code"], "anonymous");
    assert_eq!(moved["url"], "https://mine.example");
    assert_eq!(
        moved["owner"]["username"], "mover8",
        "the mover keeps ownership of the code it moved into"
    );

    // Exactly one document survives under that code, and the old one is gone.
    let urls = db.collection::<mongodb::bson::Document>("urls");
    assert_eq!(
        urls.count_documents(doc! {"code": "anonymous"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        urls.count_documents(doc! {"code": "movable"})
            .await
            .unwrap(),
        0
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn admins_may_edit_anyones_link() {
    let (app, db) = test_app().await;
    let owner = account(&app, "owner5").await;
    let admin = account(&app, "admin5").await;
    // Admin is granted out of band, exactly like the first admin in production.
    db.collection::<mongodb::bson::Document>("users")
        .update_one(
            doc! {"username": "admin5"},
            doc! {"$set": {"is_admin": true}},
        )
        .await
        .unwrap();

    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "adminedit", "url": "https://before.example"}),
        ))
        .await
        .unwrap();

    let edited = app
        .clone()
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/adminedit",
            &admin,
            json!({"code": "adminedit", "url": "https://after.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    assert_eq!(
        body_json(edited).await["owner"]["username"],
        "owner5",
        "an admin edit must not steal the link"
    );

    // Editing someone's link in place is allowed; moving another link on top of
    // it — which would delete it — is not. Admins get the same 403 as anyone.
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &admin,
            json!({"code": "adminown", "url": "https://admin.example"}),
        ))
        .await
        .unwrap();
    let onto_theirs = app
        .oneshot(helpers::authed_request(
            "PUT",
            "/api/urls/adminown",
            &admin,
            json!({"code": "adminedit", "url": "https://admin.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(onto_theirs.status(), StatusCode::FORBIDDEN);

    db.drop().await.unwrap();
}

/// A revoked session must not silently fall back to writing as anonymous —
/// that would hand a logged-out user the anonymous permission set.
#[tokio::test]
async fn a_revoked_session_is_rejected_rather_than_downgraded() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "revoked6").await;
    app.clone()
        .oneshot(helpers::authed_request(
            "POST",
            "/api/auth/logout",
            &cookie,
            json!({}),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(helpers::authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "afterlogout", "url": "https://example.com"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

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
