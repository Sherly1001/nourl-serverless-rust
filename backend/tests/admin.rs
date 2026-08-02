mod helpers;

use axum::http::StatusCode;
use helpers::{
    authed_get, authed_request, body_json, json_request, request, session_cookie, test_app,
};
use mongodb::bson::doc;
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

/// Registers an account and grants it the admin flag out of band, exactly the
/// way the first admin is bootstrapped in production.
async fn admin(app: &axum::Router, db: &mongodb::Database, username: &str) -> String {
    let cookie = account(app, username).await;
    db.collection::<mongodb::bson::Document>("users")
        .update_one(
            doc! {"username": username},
            doc! {"$set": {"is_admin": true}},
        )
        .await
        .unwrap();
    cookie
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

#[tokio::test]
async fn the_user_list_strips_secrets_and_counts_links() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "boss").await;
    let member = account(&app, "member").await;

    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &member,
            json!({"code": "theirs", "url": "https://example.com"}),
        ))
        .await
        .unwrap();

    let listed = app
        .oneshot(authed_get("/api/admin/users?limit=10", &boss))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let body = body_json(listed).await;
    assert_eq!(body["total"], 2);

    let rendered = body.to_string();
    assert!(!rendered.contains("hash_passwd"), "never leak the hash");
    assert!(!rendered.contains("$argon2"), "never leak the hash");

    let member_row = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["username"] == "member")
        .expect("member is listed");
    assert_eq!(member_row["is_admin"], false);
    assert_eq!(member_row["has_password"], true);
    assert_eq!(member_row["url_count"], 1, "counts the links they own");
    assert!(
        member_row["created_at"].as_str().is_some(),
        "created_at must be an RFC3339 string, not a BSON date"
    );
    assert_eq!(member_row["providers"].as_array().unwrap().len(), 0);

    db.drop().await.unwrap();
}

/// The users list has its own sort whitelist. Sharing the URL one would accept
/// `hits` (a field users do not have) while refusing `username`.
#[tokio::test]
async fn users_sort_by_their_own_fields_only() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "zzz-boss").await;
    account(&app, "aaa-member").await;

    let ascending = app
        .clone()
        .oneshot(authed_get("/api/admin/users?sort=username,1", &boss))
        .await
        .unwrap();
    assert_eq!(ascending.status(), StatusCode::OK);
    let body = body_json(ascending).await;
    assert_eq!(body["items"][0]["username"], "aaa-member");

    let descending = app
        .clone()
        .oneshot(authed_get("/api/admin/users?sort=username,-1", &boss))
        .await
        .unwrap();
    assert_eq!(
        body_json(descending).await["items"][0]["username"],
        "zzz-boss"
    );

    // A URL field must not be accepted just because URLs can sort by it.
    let wrong_collection = app
        .clone()
        .oneshot(authed_get("/api/admin/users?sort=hits,-1", &boss))
        .await
        .unwrap();
    assert_eq!(wrong_collection.status(), StatusCode::BAD_REQUEST);

    // `url_count` only exists after the join, so it is not sortable either.
    let after_join = app
        .oneshot(authed_get("/api/admin/users?sort=url_count,-1", &boss))
        .await
        .unwrap();
    assert_eq!(after_join.status(), StatusCode::BAD_REQUEST);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn searching_users_narrows_the_page_and_the_total() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "searchboss").await;
    account(&app, "findme").await;
    account(&app, "hidden").await;

    // Also findable by a profile field, not only the username.
    let cookie = account(&app, "byemail").await;
    app.clone()
        .oneshot(authed_request(
            "PUT",
            "/api/auth/me",
            &cookie,
            json!({"email": "needle@example.com"}),
        ))
        .await
        .unwrap();

    let by_username = app
        .clone()
        .oneshot(authed_get("/api/admin/users?q=findme", &boss))
        .await
        .unwrap();
    let body = body_json(by_username).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["username"], "findme");
    assert_eq!(
        body["total"], 1,
        "total must reflect the search, not the collection"
    );

    let by_email = app
        .clone()
        .oneshot(authed_get("/api/admin/users?q=needle", &boss))
        .await
        .unwrap();
    assert_eq!(body_json(by_email).await["items"][0]["username"], "byemail");

    // A regex metacharacter is matched literally rather than as a wildcard.
    let literal = app
        .oneshot(authed_get("/api/admin/users?q=find.me", &boss))
        .await
        .unwrap();
    assert_eq!(body_json(literal).await["total"], 0);

    db.drop().await.unwrap();
}
