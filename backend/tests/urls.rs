mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use helpers::{
    authed_get, authed_request, body_json, json_request, request, session_cookie, test_app,
};
use mongodb::bson::doc;
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn create_roundtrip() {
    let (app, db) = test_app().await;
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/urls",
            json!({"code": "hi", "url": "https://a.com"}),
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
            .oneshot(json_request("POST", "/api/urls", bad))
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
        .oneshot(json_request("POST", "/api/urls", json!({"code": "lmao"})))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "validation");
    // invalid json body
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
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
        .oneshot(json_request(
            "PUT",
            "/api/urls/a",
            json!({"code": "b", "url": "https://b.com"}),
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
        json_request(
            "POST",
            "/api/urls",
            json!({"code": "own", "url": "https://x.com"}),
        ),
        json_request(
            "PUT",
            "/api/urls/own",
            json!({"code": "own", "url": "https://x.com"}),
        ),
        request("DELETE", "/api/urls/own"),
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
        .oneshot(request("DELETE", "/api/urls/gone"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v, json!({"code": "gone", "deleted": true}));
    let resp = app
        .oneshot(request("DELETE", "/api/urls/gone"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    db.drop().await.unwrap();
}

/// Registers a user and returns their session cookie.
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
async fn creating_while_logged_in_takes_ownership() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "owner1").await;

    let created = app
        .oneshot(authed_request(
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
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "dup", "url": "https://first.example"}),
        ))
        .await
        .unwrap();

    let again = app
        .oneshot(authed_request(
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
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "theirs", "url": "https://secret.example"}),
        ))
        .await
        .unwrap();

    let attempt = json!({"code": "theirs", "url": "https://x.example"});
    for attempt_request in [
        authed_request("POST", "/api/urls", &other, attempt.clone()),
        authed_request("PUT", "/api/urls/theirs", &other, attempt.clone()),
        authed_request("DELETE", "/api/urls/theirs", &other, json!({})),
    ] {
        let response = app.clone().oneshot(attempt_request).await.unwrap();
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
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "editable", "url": "https://before.example"}),
        ))
        .await
        .unwrap();

    let edited = app
        .clone()
        .oneshot(authed_request(
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
        .oneshot(authed_request(
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
        .oneshot(request("DELETE", "/api/urls/editable"))
        .await
        .unwrap();
    assert_eq!(old_code.status(), StatusCode::NOT_FOUND);

    let removed = app
        .oneshot(authed_request(
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
            .oneshot(authed_request(
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
        .oneshot(authed_request(
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
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &mover,
            json!({"code": "alsomine", "url": "https://other.example"}),
        ))
        .await
        .unwrap();
    let onto_own = app
        .clone()
        .oneshot(authed_request(
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
        .oneshot(authed_request(
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
        .oneshot(authed_request(
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
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "adminedit", "url": "https://before.example"}),
        ))
        .await
        .unwrap();

    let edited = app
        .clone()
        .oneshot(authed_request(
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

    // Editing someone's link in place is allowed. Moving another link on top of
    // it deletes theirs, so it is asked about rather than done — but an admin
    // may answer, where anyone else gets a 403.
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &admin,
            json!({"code": "adminown", "url": "https://admin.example"}),
        ))
        .await
        .unwrap();
    let onto_theirs = app
        .oneshot(authed_request(
            "PUT",
            "/api/urls/adminown",
            &admin,
            json!({"code": "adminedit", "url": "https://admin.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(onto_theirs.status(), StatusCode::CONFLICT);

    db.drop().await.unwrap();
}

/// A revoked session must not silently fall back to writing as anonymous —
/// that would hand a logged-out user the anonymous permission set.
#[tokio::test]
async fn a_revoked_session_is_rejected_rather_than_downgraded() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "revoked6").await;
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/auth/logout",
            &cookie,
            json!({}),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(authed_request(
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
async fn list_requires_auth_and_shows_only_your_own() {
    let (app, db) = test_app().await;
    let mine = account(&app, "lister1").await;
    let theirs = account(&app, "lister2").await;

    let anonymous = app
        .clone()
        .oneshot(request("GET", "/api/urls"))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    for (cookie, code) in [(&mine, "mine-a"), (&mine, "mine-b"), (&theirs, "theirs-a")] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                cookie,
                json!({"code": code, "url": "https://example.com"}),
            ))
            .await
            .unwrap();
    }
    // An unowned link belongs to nobody's list.
    db.collection("urls")
        .insert_one(doc! {"code": "orphan", "url": "https://example.com"})
        .await
        .unwrap();

    let listed = app.oneshot(authed_get("/api/urls", &mine)).await.unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let body = body_json(listed).await;
    assert_eq!(body["total"], 2);
    let codes: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"mine-a"));
    assert!(codes.contains(&"mine-b"));
    assert!(!codes.contains(&"theirs-a"));
    assert!(!codes.contains(&"orphan"));
    assert_eq!(body["items"][0]["owner"]["username"], "lister1");

    db.drop().await.unwrap();
}

#[tokio::test]
async fn admins_see_everything_and_search_narrows_it() {
    let (app, db) = test_app().await;
    let user = account(&app, "listed").await;
    let admin = account(&app, "listadmin").await;
    db.collection::<mongodb::bson::Document>("users")
        .update_one(
            doc! {"username": "listadmin"},
            doc! {"$set": {"is_admin": true}},
        )
        .await
        .unwrap();

    for code in ["alpha", "beta"] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                &user,
                json!({"code": code, "url": "https://example.com"}),
            ))
            .await
            .unwrap();
    }
    // Anonymous links have no owner to scope by, so only the unscoped admin
    // view can reach them at all.
    db.collection("urls")
        .insert_one(doc! {"code": "orphan", "url": "https://example.com"})
        .await
        .unwrap();

    let all = app
        .clone()
        .oneshot(authed_get("/api/urls?sort=code,1", &admin))
        .await
        .unwrap();
    let all = body_json(all).await;
    assert_eq!(all["total"], 3);
    let codes: Vec<&str> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["alpha", "beta", "orphan"]);
    assert!(
        all["items"][2]["owner"].is_null(),
        "an unowned link lists with a null owner rather than being skipped"
    );

    // The admin's own list is not special-cased: they own nothing here.
    let owned_by_admin = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["owner"]["username"] == "listadmin")
        .count();
    assert_eq!(owned_by_admin, 0);

    let searched = app
        .clone()
        .oneshot(authed_get("/api/urls?q=alph", &admin))
        .await
        .unwrap();
    let body = body_json(searched).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["code"], "alpha");

    // `mine` puts an admin back in the ordinary view. The account page counts
    // through this endpoint to say how many links closing the account would
    // take with it, and closing an account only ever touches its owner's.
    let mine = app
        .clone()
        .oneshot(authed_get("/api/urls?mine=true", &admin))
        .await
        .unwrap();
    let mine = body_json(mine).await;
    assert_eq!(mine["total"], 0, "the admin owns none of these links");

    let theirs = app
        .clone()
        .oneshot(authed_get("/api/urls?mine=true", &user))
        .await
        .unwrap();
    assert_eq!(
        body_json(theirs).await["total"],
        2,
        "an ordinary account is already scoped, so mine changes nothing"
    );

    // A rejected sort field must not reach Mongo.
    let bad_sort = app
        .oneshot(authed_get("/api/urls?sort=hash_passwd,1", &admin))
        .await
        .unwrap();
    assert_eq!(bad_sort.status(), StatusCode::BAD_REQUEST);

    db.drop().await.unwrap();
}

/// `total` counts everything matching the filter, not just the page, or the UI
/// cannot render pagination.
#[tokio::test]
async fn paging_and_sorting_walk_the_whole_set() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "pager").await;
    for code in ["a", "b", "c"] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                &cookie,
                json!({"code": code, "url": "https://example.com"}),
            ))
            .await
            .unwrap();
    }

    let first = app
        .clone()
        .oneshot(authed_get("/api/urls?sort=code,1&limit=2&skip=0", &cookie))
        .await
        .unwrap();
    let first = body_json(first).await;
    assert_eq!(
        first["total"], 3,
        "total spans the whole match, not the page"
    );
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["items"][0]["code"], "a");
    assert_eq!(first["items"][1]["code"], "b");

    let second = app
        .clone()
        .oneshot(authed_get("/api/urls?sort=code,1&limit=2&skip=2", &cookie))
        .await
        .unwrap();
    let second = body_json(second).await;
    assert_eq!(second["total"], 3);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["code"], "c");

    let descending = app
        .oneshot(authed_get("/api/urls?sort=code,-1", &cookie))
        .await
        .unwrap();
    assert_eq!(body_json(descending).await["items"][0]["code"], "c");

    db.drop().await.unwrap();
}

/// Creating a code that already exists but belongs to nobody must hand it to
/// the author. `$setOnInsert` does not fire when the document is already
/// there, so ownership was silently skipped.
#[tokio::test]
async fn creating_over_an_unowned_code_claims_it() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "claimer").await;
    db.collection("urls")
        .insert_one(doc! {"code": "orphan", "url": "https://old.example"})
        .await
        .unwrap();

    let created = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "orphan", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(body_json(created).await["owner"]["username"], "claimer");

    let listed = app
        .clone()
        .oneshot(authed_get("/api/urls", &cookie))
        .await
        .unwrap();
    assert_eq!(
        body_json(listed).await["total"],
        1,
        "a claimed link belongs in the author's list"
    );

    // Editing is not claiming: a PUT at an unowned link leaves it unowned, so
    // fixing a stray link does not quietly absorb it.
    db.collection("urls")
        .insert_one(doc! {"code": "stray", "url": "https://old.example"})
        .await
        .unwrap();
    let edited = app
        .oneshot(authed_request(
            "PUT",
            "/api/urls/stray",
            &cookie,
            json!({"code": "stray", "url": "https://edited.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    let edited = body_json(edited).await;
    assert_eq!(edited["url"], "https://edited.example");
    assert!(edited["owner"].is_null(), "a PUT must not claim ownership");

    db.drop().await.unwrap();
}

/// Mongo stores these as BSON datetimes but `UrlEntry` holds strings, so
/// without a conversion in the pipeline the whole response fails to
/// deserialize — creating a link with an expiry used to be a 500.
#[tokio::test]
async fn dates_come_back_as_rfc3339_strings() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "dater").await;
    let expiry = (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339();

    let created = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "dated", "url": "https://example.com", "expires_at": expiry}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = body_json(created).await;
    for field in ["created_at", "expires_at"] {
        let value = body[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field}: {body}"));
        assert!(value.ends_with('Z'), "{field} is not RFC3339: {value}");
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap_or_else(|e| panic!("{field} unparseable: {value} ({e})"));
    }
    assert!(body["last_hit_at"].is_null(), "never hit yet");
    let first_write = body["updated_at"]
        .as_str()
        .expect("stamped on create")
        .to_string();

    // An edit moves updated_at without disturbing created_at.
    let edited = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/urls/dated",
            &cookie,
            json!({"code": "dated", "url": "https://elsewhere.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    let edited = body_json(edited).await;
    assert_eq!(
        edited["created_at"], body["created_at"],
        "creation is fixed"
    );
    assert_ne!(
        edited["updated_at"].as_str().unwrap(),
        first_write,
        "an edit must move updated_at"
    );

    // A legacy document whose created_at is already a string must survive the
    // same pipeline rather than aborting the aggregation.
    db.collection("urls")
        .insert_one(doc! {"code": "legacy", "url": "https://old.example", "created_at": "2020-01-01T00:00:00Z"})
        .await
        .unwrap();
    let listed = app
        .oneshot(authed_get("/api/urls?q=legacy", &cookie))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);

    db.drop().await.unwrap();
}

/// Every link here shares one `updated_at`, so the order is decided entirely by
/// the tiebreak the sort carries. The `_id`s are handed out in reverse of the
/// codes, so insertion order and `_id` order disagree: a pipeline that drops
/// `_id` before it sorts has no tiebreak left, pages the rows in whatever order
/// the collection scan hands back, and the same link surfaces on two pages.
#[tokio::test]
async fn tied_sort_keys_page_in_id_order() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "tied").await;
    let owner = db
        .collection::<mongodb::bson::Document>("users")
        .find_one(doc! {"username": "tied"})
        .await
        .unwrap()
        .expect("registration stored the account")
        .get_str("id")
        .unwrap()
        .to_string();

    let stamp = mongodb::bson::DateTime::now();
    let codes = ["c0", "c1", "c2", "c3", "c4", "c5"];
    let docs: Vec<_> = codes
        .iter()
        .enumerate()
        .map(|(i, code)| {
            let mut bytes = [0u8; 12];
            bytes[11] = (codes.len() - 1 - i) as u8;
            doc! {
                "_id": mongodb::bson::oid::ObjectId::from_bytes(bytes),
                "code": *code,
                "url": "https://example.com",
                "owner": &owner,
                "created_at": stamp,
                "updated_at": stamp,
            }
        })
        .collect();
    db.collection::<mongodb::bson::Document>("urls")
        .insert_many(docs)
        .await
        .unwrap();

    let mut seen: Vec<String> = Vec::new();
    for skip in [0, 2, 4] {
        let page = app
            .clone()
            .oneshot(authed_get(
                &format!("/api/urls?sort=updated_at,-1&limit=2&skip={skip}"),
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let body = body_json(page).await;
        assert_eq!(body["total"], 6);
        for item in body["items"].as_array().unwrap() {
            seen.push(item["code"].as_str().unwrap().to_string());
        }
    }

    // `_id` ascending, the reverse of the order they were inserted in.
    assert_eq!(seen, ["c5", "c4", "c3", "c2", "c1", "c0"]);
    db.drop().await.unwrap();
}

/// A day out, as the wire spells it.
fn tomorrow() -> String {
    mongodb::bson::DateTime::from_millis(
        mongodb::bson::DateTime::now().timestamp_millis() + 86_400_000,
    )
    .try_to_rfc3339_string()
    .unwrap()
}

/// An absent `expires_at` is not a request to remove one — a rename or a URL
/// fix would otherwise silently un-expire the link.
#[tokio::test]
async fn an_omitted_expiry_leaves_the_stored_one_alone() {
    let (app, db) = test_app().await;

    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/urls",
            json!({"code": "keep", "url": "https://a.example", "expires_at": tomorrow()}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let was = body_json(created).await["expires_at"].clone();

    let edited = app
        .oneshot(json_request(
            "PUT",
            "/api/urls/keep",
            json!({"code": "keep", "url": "https://b.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    let body = body_json(edited).await;
    assert_eq!(body["url"], "https://b.example");
    assert_eq!(body["expires_at"], was, "editing the url must not touch it");

    db.drop().await.unwrap();
}

/// An explicit empty string is the one way to say "this link stops expiring".
#[tokio::test]
async fn an_empty_expiry_removes_it() {
    let (app, db) = test_app().await;

    app.clone()
        .oneshot(json_request(
            "POST",
            "/api/urls",
            json!({"code": "clear", "url": "https://a.example", "expires_at": tomorrow()}),
        ))
        .await
        .unwrap();

    let cleared = app
        .oneshot(json_request(
            "PUT",
            "/api/urls/clear",
            json!({"code": "clear", "url": "https://a.example", "expires_at": ""}),
        ))
        .await
        .unwrap();
    assert_eq!(cleared.status(), StatusCode::OK);
    assert!(
        body_json(cleared).await["expires_at"].is_null(),
        "the field is gone, not blank"
    );

    let stored = db
        .collection::<mongodb::bson::Document>("urls")
        .find_one(doc! {"code": "clear"})
        .await
        .unwrap()
        .unwrap();
    assert!(
        stored.get("expires_at").is_none(),
        "unset, so the partial TTL index has nothing to reap"
    );

    db.drop().await.unwrap();
}

/// The expiry survives the code changing, which is the write that touches two
/// documents rather than one.
#[tokio::test]
async fn a_rename_carries_the_expiry_across() {
    let (app, db) = test_app().await;

    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/urls",
            json!({"code": "before", "url": "https://a.example", "expires_at": tomorrow()}),
        ))
        .await
        .unwrap();
    let was = body_json(created).await["expires_at"].clone();

    let renamed = app
        .oneshot(json_request(
            "PUT",
            "/api/urls/before",
            json!({"code": "after", "url": "https://a.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    let body = body_json(renamed).await;
    assert_eq!(body["code"], "after");
    assert_eq!(body["expires_at"], was);

    db.drop().await.unwrap();
}

/// The 409 carries the link it collided with, so the UI can offer to replace
/// it rather than only naming what is in the way.
#[tokio::test]
async fn an_own_code_conflict_carries_the_whole_link() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "collider").await;
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "dup2", "url": "https://first.example", "expires_at": tomorrow()}),
        ))
        .await
        .unwrap();

    let again = app
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "dup2", "url": "https://second.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::CONFLICT);
    let conflict = body_json(again).await["error"]["conflict"].clone();
    assert_eq!(conflict["code"], "dup2");
    assert_eq!(conflict["url"], "https://first.example");
    assert!(conflict["expires_at"].is_string());

    db.drop().await.unwrap();
}

/// Sending the same write back with `overwrite` is how the owner says yes.
#[tokio::test]
async fn overwrite_replaces_a_code_you_own() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "replacer").await;
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "over", "url": "https://first.example"}),
        ))
        .await
        .unwrap();
    db.collection::<mongodb::bson::Document>("urls")
        .update_one(doc! {"code": "over"}, doc! {"$set": {"hits": 41}})
        .await
        .unwrap();

    let replaced = app
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "over", "url": "https://second.example", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    let body = body_json(replaced).await;
    assert_eq!(body["url"], "https://second.example");
    assert_eq!(body["hits"], 41, "replacing the target keeps the history");

    db.drop().await.unwrap();
}

/// The counter restarts only when asked, and the last visit goes with it —
/// a date with no visits behind it is a date about nothing.
#[tokio::test]
async fn overwrite_can_reset_the_hit_count() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "resetter").await;
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({"code": "fresh", "url": "https://first.example"}),
        ))
        .await
        .unwrap();
    let urls = db.collection::<mongodb::bson::Document>("urls");
    urls.update_one(
        doc! {"code": "fresh"},
        doc! {"$set": {"hits": 9, "last_hit_at": mongodb::bson::DateTime::now()}},
    )
    .await
    .unwrap();

    let replaced = app
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &cookie,
            json!({
                "code": "fresh",
                "url": "https://second.example",
                "overwrite": true,
                "reset_hits": true,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(body_json(replaced).await["hits"], 0);

    let stored = urls
        .find_one(doc! {"code": "fresh"})
        .await
        .unwrap()
        .unwrap();
    assert!(stored.get("last_hit_at").is_none());

    db.drop().await.unwrap();
}

/// `overwrite` is not a way past someone else's link. The answer stays the
/// same 403, with nothing in it worth harvesting.
#[tokio::test]
async fn overwrite_does_not_open_someone_elses_code() {
    let (app, db) = test_app().await;
    let owner = account(&app, "owner5").await;
    let other = account(&app, "other5").await;
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "walled", "url": "https://secret.example"}),
        ))
        .await
        .unwrap();

    let attempt = app
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &other,
            json!({"code": "walled", "url": "https://x.example", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert_eq!(attempt.status(), StatusCode::FORBIDDEN);
    let body = body_json(attempt).await;
    assert!(body["error"]["conflict"].is_null());
    assert!(!body["error"]["message"].to_string().contains("secret"));

    db.drop().await.unwrap();
}

/// A rename onto a code you already use destroys the link that was there, so
/// it asks first — and answers with what would be destroyed.
#[tokio::test]
async fn a_rename_onto_your_own_code_asks_before_it_destroys() {
    let (app, db) = test_app().await;
    let cookie = account(&app, "mover").await;
    for (code, url) in [
        ("from", "https://from.example"),
        ("onto", "https://onto.example"),
    ] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                &cookie,
                json!({"code": code, "url": url}),
            ))
            .await
            .unwrap();
    }

    let refused = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/urls/from",
            &cookie,
            json!({"code": "onto", "url": "https://from.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let conflict = body_json(refused).await["error"]["conflict"].clone();
    assert_eq!(conflict["code"], "onto");
    assert_eq!(conflict["url"], "https://onto.example");

    let moved = app
        .oneshot(authed_request(
            "PUT",
            "/api/urls/from",
            &cookie,
            json!({"code": "onto", "url": "https://from.example", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    assert_eq!(body_json(moved).await["url"], "https://from.example");

    let urls = db.collection::<mongodb::bson::Document>("urls");
    assert_eq!(
        urls.count_documents(doc! {"code": "onto"}).await.unwrap(),
        1
    );
    assert_eq!(
        urls.count_documents(doc! {"code": "from"}).await.unwrap(),
        0
    );

    db.drop().await.unwrap();
}

/// Makes `username` an admin, the way the first admin is made in production.
async fn promote(db: &mongodb::Database, username: &str) {
    db.collection::<mongodb::bson::Document>("users")
        .update_one(
            doc! {"username": username},
            doc! {"$set": {"is_admin": true}},
        )
        .await
        .unwrap();
}

/// An admin may write over anyone's link, which until now they did in silence.
/// The link comes back with its owner attached, because that is the fact that
/// makes the decision serious.
#[tokio::test]
async fn an_admin_creating_over_someone_elses_code_is_asked_first() {
    let (app, db) = test_app().await;
    let owner = account(&app, "victim").await;
    let admin = account(&app, "overlord").await;
    promote(&db, "overlord").await;

    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "theirs2", "url": "https://theirs.example"}),
        ))
        .await
        .unwrap();

    let asked = app
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &admin,
            json!({"code": "theirs2", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::CONFLICT);
    let conflict = body_json(asked).await["error"]["conflict"].clone();
    assert_eq!(conflict["url"], "https://theirs.example");
    assert_eq!(conflict["owner"]["username"], "victim");

    db.drop().await.unwrap();
}

/// Replacing where a link points does not take it, and taking it is a separate
/// thing to ask for.
#[tokio::test]
async fn an_admin_overwrite_leaves_the_owner_alone_unless_it_claims() {
    let (app, db) = test_app().await;
    let owner = account(&app, "victim2").await;
    let admin = account(&app, "overlord2").await;
    promote(&db, "overlord2").await;

    for (code, url) in [
        ("kept", "https://a.example"),
        ("taken", "https://b.example"),
    ] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                &owner,
                json!({"code": code, "url": url}),
            ))
            .await
            .unwrap();
    }

    let replaced = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &admin,
            json!({"code": "kept", "url": "https://fixed.example", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    let body = body_json(replaced).await;
    assert_eq!(body["url"], "https://fixed.example");
    assert_eq!(body["owner"]["username"], "victim2");

    let claimed = app
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &admin,
            json!({
                "code": "taken",
                "url": "https://fixed.example",
                "overwrite": true,
                "claim": true,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(claimed.status(), StatusCode::OK);
    assert_eq!(
        body_json(claimed).await["owner"]["username"],
        "overlord2",
        "claiming is the one way the owner changes"
    );

    db.drop().await.unwrap();
}

/// The destructive one: the other user's link is deleted so this one can take
/// its code. Refused until the admin says yes.
#[tokio::test]
async fn an_admin_renames_onto_someone_elses_code_only_after_confirming() {
    let (app, db) = test_app().await;
    let owner = account(&app, "victim3").await;
    let admin = account(&app, "overlord3").await;
    promote(&db, "overlord3").await;

    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &owner,
            json!({"code": "wanted", "url": "https://theirs.example"}),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &admin,
            json!({"code": "mine3", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();

    let asked = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/urls/mine3",
            &admin,
            json!({"code": "wanted", "url": "https://mine.example"}),
        ))
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(asked).await["error"]["conflict"]["owner"]["username"],
        "victim3"
    );

    let moved = app
        .oneshot(authed_request(
            "PUT",
            "/api/urls/mine3",
            &admin,
            json!({"code": "wanted", "url": "https://mine.example", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    let body = body_json(moved).await;
    assert_eq!(body["url"], "https://mine.example");
    assert_eq!(
        body["owner"]["username"], "overlord3",
        "the link that moved keeps its own owner; the one it landed on is gone"
    );

    let urls = db.collection::<mongodb::bson::Document>("urls");
    assert_eq!(
        urls.count_documents(doc! {"code": "mine3"}).await.unwrap(),
        0
    );

    db.drop().await.unwrap();
}
