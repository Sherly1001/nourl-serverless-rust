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
    assert_eq!(body["total"], 1, "the admin is in the tree, not the bucket");

    let rendered = body.to_string();
    assert!(!rendered.contains("hash_passwd"), "never leak the hash");
    assert!(!rendered.contains("$argon2"), "never leak the hash");
    assert_eq!(body["admins"][0]["username"], "boss");
    assert_eq!(body["admins"][0]["has_password"], true);

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
    let boss = admin(&app, &db, "the-boss").await;
    // The sort applies to the bucket, which the admin is not in.
    account(&app, "aaa-member").await;
    account(&app, "zzz-member").await;

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
        "zzz-member"
    );

    // A URL field must not be accepted just because URLs can sort by it.
    let wrong_collection = app
        .clone()
        .oneshot(authed_get("/api/admin/users?sort=hits,-1", &boss))
        .await
        .unwrap();
    assert_eq!(wrong_collection.status(), StatusCode::BAD_REQUEST);

    // Derived from the chain, so there is no field to sort on.
    let derived = app
        .oneshot(authed_get("/api/admin/users?sort=admin_level,-1", &boss))
        .await
        .unwrap();
    assert_eq!(derived.status(), StatusCode::BAD_REQUEST);

    db.drop().await.unwrap();
}

/// Ordering by the link count is the one sort that cannot be applied before
/// the join, so it takes a different pipeline. It has to produce the same rows
/// as the ordinary shape, just in a different order.
#[tokio::test]
async fn users_can_be_ordered_by_how_many_links_they_own() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "count-boss").await;
    let busy = account(&app, "busy").await;
    let quiet = account(&app, "quiet").await;
    account(&app, "idle").await;

    for code in ["one", "two", "three"] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                &busy,
                json!({"code": code, "url": "https://example.com"}),
            ))
            .await
            .unwrap();
    }
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &quiet,
            json!({"code": "solo", "url": "https://example.com"}),
        ))
        .await
        .unwrap();

    let by_count = |dir: &'static str| {
        let (app, boss) = (app.clone(), boss.clone());
        async move {
            let response = app
                .oneshot(authed_get(
                    &format!("/api/admin/users?sort=url_count,{dir}"),
                    &boss,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = body_json(response).await;
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row["username"].as_str().unwrap().to_string(),
                        row["url_count"].as_u64().unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };

    assert_eq!(
        by_count("-1").await,
        [
            ("busy".to_string(), 3),
            ("quiet".to_string(), 1),
            ("idle".to_string(), 0)
        ]
    );
    // The join before the paging must not drop or duplicate anyone.
    let ascending = by_count("1").await;
    assert_eq!(ascending.first().unwrap().0, "idle");
    assert_eq!(ascending.len(), 3);

    let paged = app
        .oneshot(authed_get(
            "/api/admin/users?sort=url_count,-1&limit=1&skip=1",
            &boss,
        ))
        .await
        .unwrap();
    let body = body_json(paged).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["username"], "quiet");
    assert_eq!(body["total"], 3, "the total ignores the page");

    db.drop().await.unwrap();
}

/// The tree's shape is fixed, but the order siblings appear in is the client's
/// to choose — and a search must never remove an admin, or the accounts below
/// them lose their parent.
#[tokio::test]
async fn the_admin_tree_honours_the_sort_but_not_the_search() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "m-root").await;
    promote(&app, &root, "z-second").await;
    promote(&app, &root, "a-third").await;

    let names = |query: &'static str| {
        let (app, root) = (app.clone(), root.clone());
        async move {
            let response = app
                .oneshot(authed_get(&format!("/api/admin/users?{query}"), &root))
                .await
                .unwrap();
            body_json(response).await["admins"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["username"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };

    assert_eq!(
        names("sort=username,1").await,
        ["a-third", "m-root", "z-second"]
    );
    assert_eq!(
        names("sort=username,-1").await,
        ["z-second", "m-root", "a-third"]
    );

    // The client highlights a match; it does not prune the tree.
    assert_eq!(names("q=z-second&sort=username,1").await.len(), 3);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn searching_users_narrows_the_page_and_the_total() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "searchboss").await;
    account(&app, "findme").await;
    account(&app, "hidden").await;

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

    let literal = app
        .oneshot(authed_get("/api/admin/users?q=find.me", &boss))
        .await
        .unwrap();
    assert_eq!(body_json(literal).await["total"], 0);

    db.drop().await.unwrap();
}

/// Looks up the account id the list reports for `username`, wherever it sits —
/// admins come back in `admins`, everyone else in the paged `items`.
async fn id_of(app: &axum::Router, cookie: &str, username: &str) -> String {
    row_of(app, cookie, username).await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Fetches one row of the list by username, from either bucket.
async fn row_of(app: &axum::Router, cookie: &str, username: &str) -> serde_json::Value {
    let listed = app
        .clone()
        .oneshot(authed_get("/api/admin/users?limit=100", cookie))
        .await
        .unwrap();
    let body = body_json(listed).await;
    body["admins"]
        .as_array()
        .unwrap()
        .iter()
        .chain(body["items"].as_array().unwrap())
        .find(|row| row["username"] == username)
        .unwrap_or_else(|| panic!("{username} is listed"))
        .clone()
}

/// Sends a change to someone's admin standing, as `actor`.
async fn set_admin(
    app: &axum::Router,
    actor: &str,
    target_id: &str,
    body: serde_json::Value,
) -> axum::http::Response<axum::body::Body> {
    app.clone()
        .oneshot(authed_request(
            "PUT",
            &format!("/api/admin/users/{target_id}"),
            actor,
            body,
        ))
        .await
        .unwrap()
}

/// Registers `username`, promotes them with `promoter`'s session, and returns
/// the new admin's own cookie.
async fn promote(app: &axum::Router, promoter: &str, username: &str) -> String {
    let cookie = account(app, username).await;
    let id = id_of(app, promoter, username).await;
    let response = set_admin(app, promoter, &id, json!({"is_admin": true})).await;
    assert_eq!(response.status(), StatusCode::OK, "promoting {username}");
    cookie
}

#[tokio::test]
async fn an_admin_may_promote_others_but_never_themselves() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "boss2").await;
    let plain = account(&app, "promotable").await;

    let target = id_of(&app, &boss, "promotable").await;
    let own = id_of(&app, &boss, "boss2").await;

    let promoted = set_admin(&app, &boss, &target, json!({"is_admin": true})).await;
    assert_eq!(promoted.status(), StatusCode::OK);
    assert_eq!(body_json(promoted).await["is_admin"], true);

    // The promotion is real, not just echoed back.
    let now_admin = app
        .clone()
        .oneshot(authed_get("/api/admin/users", &plain))
        .await
        .unwrap();
    assert_eq!(now_admin.status(), StatusCode::OK);

    // Demoting yourself is how a root locks everyone out of the admin pages.
    let self_demote = set_admin(&app, &boss, &own, json!({"is_admin": false})).await;
    assert_eq!(self_demote.status(), StatusCode::BAD_REQUEST);

    let still_admin = app
        .clone()
        .oneshot(authed_get("/api/auth/me", &boss))
        .await
        .unwrap();
    assert_eq!(body_json(still_admin).await["is_admin"], true);

    let missing = set_admin(&app, &boss, "no-such-id", json!({"is_admin": true})).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    db.drop().await.unwrap();
}

/// The chain: an admin seeded in the database is depth 0, and every grant hangs
/// one below whoever made it. Depth is derived from the parent pointer, not
/// stored, so there is nothing to keep in sync.
#[tokio::test]
async fn a_promotee_hangs_one_below_whoever_promoted_them() {
    let (app, db) = test_app().await;
    let one = admin(&app, &db, "chain-one").await;
    let two = promote(&app, &one, "chain-two").await;
    promote(&app, &two, "chain-three").await;

    let seeded = row_of(&app, &one, "chain-one").await;
    assert_eq!(seeded["admin_level"], 0, "a seeded admin is the root");
    assert!(
        seeded["promoted_by"].is_null(),
        "nobody promoted the first admin"
    );

    let second = row_of(&app, &one, "chain-two").await;
    assert_eq!(second["admin_level"], 1);
    assert_eq!(second["promoted_by"], id_of(&app, &one, "chain-one").await);

    let third = row_of(&app, &one, "chain-three").await;
    assert_eq!(third["admin_level"], 2);
    assert_eq!(third["promoted_by"], id_of(&app, &one, "chain-two").await);

    account(&app, "nobody").await;
    let plain = row_of(&app, &one, "nobody").await;
    assert!(plain["admin_level"].is_null());
    assert!(plain["promoted_by"].is_null());

    db.drop().await.unwrap();
}

/// Admins are returned whole; everyone else is paged and searched. The split
/// is what lets the client draw a tree that a search cannot cut branches off.
#[tokio::test]
async fn admins_come_back_whole_and_everyone_else_is_paged() {
    let (app, db) = test_app().await;
    let one = admin(&app, &db, "split-one").await;
    promote(&app, &one, "split-two").await;
    account(&app, "plain-a").await;
    account(&app, "plain-b").await;

    let listed = app
        .clone()
        .oneshot(authed_get(
            "/api/admin/users?q=plain-a&sort=username,1",
            &one,
        ))
        .await
        .unwrap();
    let body = body_json(listed).await;

    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["username"], "plain-a");
    assert_eq!(body["total"], 1, "the total counts ordinary accounts only");

    // ...intact, or "split-two" would have no parent to hang from.
    let admins: Vec<&str> = body["admins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["username"].as_str().unwrap())
        .collect();
    assert_eq!(admins, ["split-one", "split-two"]);

    assert!(
        !body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["is_admin"] == true)
    );

    db.drop().await.unwrap();
}

/// The subtree rule: an admin owns what grew below them, and nothing else.
#[tokio::test]
async fn an_admin_may_act_only_inside_their_own_subtree() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "tree-root").await;
    let left = promote(&app, &root, "tree-left").await;
    let right = promote(&app, &root, "tree-right").await;
    promote(&app, &left, "tree-left-child").await;
    let right_child = promote(&app, &right, "tree-right-child").await;

    let id = |name: &'static str| {
        let (app, root) = (app.clone(), root.clone());
        async move { id_of(&app, &root, name).await }
    };
    let (id_root, id_left, id_right) = (
        id("tree-root").await,
        id("tree-left").await,
        id("tree-right").await,
    );
    let id_right_child = id("tree-right-child").await;

    let demote = json!({"is_admin": false});

    // Sideways is refused even at equal depth: not your branch.
    assert_eq!(
        set_admin(&app, &left, &id_right, demote.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        set_admin(&app, &left, &id_right_child, demote.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "nor anyone the other branch promoted"
    );
    assert_eq!(
        set_admin(&app, &right_child, &id_root, demote.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(row_of(&app, &root, "tree-right").await["admin_level"], 1);
    assert_eq!(row_of(&app, &root, "tree-root").await["admin_level"], 0);

    // Downward works, at any distance: the root reaches the whole tree.
    assert_eq!(
        set_admin(&app, &root, &id_left, demote).await.status(),
        StatusCode::OK
    );

    db.drop().await.unwrap();
}

/// Demoting cascades: the flag was only ever held on the strength of the
/// vouching above it.
#[tokio::test]
async fn demoting_an_admin_demotes_everyone_below_them() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "casc-root").await;
    let mid = promote(&app, &root, "casc-mid").await;
    let leaf = promote(&app, &mid, "casc-leaf").await;
    promote(&app, &leaf, "casc-deep").await;
    // A sibling branch, to prove the cascade is bounded by the subtree.
    promote(&app, &root, "casc-other").await;

    let id_mid = id_of(&app, &root, "casc-mid").await;
    let response = set_admin(&app, &root, &id_mid, json!({"is_admin": false})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["demoted"], 3, "the target and the two below them");

    for name in ["casc-mid", "casc-leaf", "casc-deep"] {
        let row = row_of(&app, &root, name).await;
        assert_eq!(row["is_admin"], false, "{name} kept the flag");
        assert!(row["promoted_by"].is_null(), "{name} kept its parent");
        assert!(row["admin_level"].is_null());
    }
    assert_eq!(row_of(&app, &root, "casc-other").await["is_admin"], true);

    for cookie in [&mid, &leaf] {
        let locked_out = app
            .clone()
            .oneshot(authed_get("/api/admin/users", cookie))
            .await
            .unwrap();
        assert_eq!(locked_out.status(), StatusCode::FORBIDDEN);
    }

    db.drop().await.unwrap();
}

/// Moving a branch is the same write as promoting: set who vouches for them.
#[tokio::test]
async fn an_admin_can_be_moved_within_the_part_of_the_tree_you_control() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "move-root").await;
    let left = promote(&app, &root, "move-left").await;
    promote(&app, &root, "move-right").await;
    promote(&app, &left, "move-child").await;

    let id = |name: &'static str| {
        let (app, root) = (app.clone(), root.clone());
        async move { id_of(&app, &root, name).await }
    };
    let (id_left, id_right) = (id("move-left").await, id("move-right").await);
    let id_child = id("move-child").await;

    let moved = set_admin(
        &app,
        &root,
        &id_left,
        json!({"is_admin": true, "promoted_by": id_right}),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    assert_eq!(body_json(moved).await["promoted_by"], id_right);

    // Depth followed from the pointer rather than needing a rewrite.
    assert_eq!(row_of(&app, &root, "move-left").await["admin_level"], 2);
    assert_eq!(row_of(&app, &root, "move-child").await["admin_level"], 3);

    // A cycle is refused: the parent cannot be inside the branch being moved.
    let cycle = set_admin(
        &app,
        &root,
        &id_right,
        json!({"is_admin": true, "promoted_by": id_child}),
    )
    .await;
    assert_eq!(cycle.status(), StatusCode::BAD_REQUEST);
    let itself = set_admin(
        &app,
        &root,
        &id_right,
        json!({"is_admin": true, "promoted_by": id_right}),
    )
    .await;
    assert_eq!(itself.status(), StatusCode::BAD_REQUEST);
    assert_eq!(row_of(&app, &root, "move-right").await["admin_level"], 1);

    db.drop().await.unwrap();
}

/// A move must not be a way to reach outside your own subtree, in either
/// direction: not by grafting someone onto a branch you do not control, and not
/// by placing them under an ordinary account.
#[tokio::test]
async fn a_move_cannot_reach_outside_the_callers_subtree() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "graft-root").await;
    let left = promote(&app, &root, "graft-left").await;
    promote(&app, &root, "graft-right").await;
    promote(&app, &left, "graft-child").await;
    account(&app, "graft-plain").await;

    let id = |name: &'static str| {
        let (app, root) = (app.clone(), root.clone());
        async move { id_of(&app, &root, name).await }
    };
    let id_child = id("graft-child").await;
    let id_right = id("graft-right").await;
    let id_plain = id("graft-plain").await;

    // `left` controls its own child, but `right` is not theirs to hang it on.
    let sideways = set_admin(
        &app,
        &left,
        &id_child,
        json!({"is_admin": true, "promoted_by": id_right}),
    )
    .await;
    assert_eq!(sideways.status(), StatusCode::FORBIDDEN);

    let under_plain = set_admin(
        &app,
        &root,
        &id_child,
        json!({"is_admin": true, "promoted_by": id_plain}),
    )
    .await;
    assert_eq!(under_plain.status(), StatusCode::BAD_REQUEST);

    let missing = set_admin(
        &app,
        &root,
        &id_child,
        json!({"is_admin": true, "promoted_by": "no-such-id"}),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    assert_eq!(row_of(&app, &root, "graft-child").await["admin_level"], 2);

    db.drop().await.unwrap();
}

/// The flag is not something an account can set on itself by calling the
/// endpoint it is not allowed to reach.
#[tokio::test]
async fn an_ordinary_account_cannot_promote_itself() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "boss3").await;
    let plain = account(&app, "climber").await;
    let own = id_of(&app, &boss, "climber").await;

    let attempt = set_admin(&app, &plain, &own, json!({"is_admin": true})).await;
    assert_eq!(attempt.status(), StatusCode::FORBIDDEN);

    let unchanged = app
        .oneshot(authed_get("/api/auth/me", &plain))
        .await
        .unwrap();
    assert_eq!(body_json(unchanged).await["is_admin"], false);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn deleting_a_user_orphans_their_links_with_a_deadline() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "del-boss").await;
    let doomed = account(&app, "doomed").await;

    // The sooner of an existing expiry and the grace period must win.
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &doomed,
            json!({"code": "no-expiry", "url": "https://example.com"}),
        ))
        .await
        .unwrap();
    let soon = (chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339();
    app.clone()
        .oneshot(authed_request(
            "POST",
            "/api/urls",
            &doomed,
            json!({"code": "expires-soon", "url": "https://example.com", "expires_at": soon}),
        ))
        .await
        .unwrap();

    let target = id_of(&app, &boss, "doomed").await;
    let deleted = app
        .clone()
        .oneshot(authed_request(
            "DELETE",
            &format!("/api/admin/users/{target}"),
            &boss,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let outcome = body_json(deleted).await;
    assert_eq!(outcome["orphaned"], 2);
    assert_eq!(
        outcome["links_deleted"], 0,
        "an admin never destroys someone else's links"
    );
    assert_eq!(outcome["grace_days"], 7);

    assert!(
        db.collection::<mongodb::bson::Document>("users")
            .find_one(doc! {"username": "doomed"})
            .await
            .unwrap()
            .is_none()
    );

    // Not deleted: somebody may still be following them.
    let urls = db.collection::<mongodb::bson::Document>("urls");
    for code in ["no-expiry", "expires-soon"] {
        let row = urls
            .find_one(doc! {"code": code})
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{code} must survive its owner"));
        assert!(row.get("owner").is_none(), "{code} must be unowned");
        assert!(
            row.get_datetime("expires_at").is_ok(),
            "{code} needs a deadline"
        );
    }

    let cutoff = chrono::Utc::now() + chrono::Duration::days(7);
    let sooner = urls
        .find_one(doc! {"code": "expires-soon"})
        .await
        .unwrap()
        .unwrap();
    let kept = sooner
        .get_datetime("expires_at")
        .unwrap()
        .timestamp_millis();
    assert!(
        kept < cutoff.timestamp_millis(),
        "an earlier expiry must not be pushed out to the grace period"
    );

    db.drop().await.unwrap();
}

/// Deleting is heavier than demoting, so it must not reach anywhere demoting
/// cannot.
#[tokio::test]
async fn deleting_obeys_the_same_subtree_rule_as_demoting() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "dsub-root").await;
    let left = promote(&app, &root, "dsub-left").await;
    promote(&app, &root, "dsub-right").await;

    let delete = |actor: &str, id: String| {
        let (app, actor) = (app.clone(), actor.to_string());
        async move {
            app.oneshot(authed_request(
                "DELETE",
                &format!("/api/admin/users/{id}"),
                &actor,
                json!({}),
            ))
            .await
            .unwrap()
            .status()
        }
    };

    let id_root = id_of(&app, &root, "dsub-root").await;
    let id_left = id_of(&app, &root, "dsub-left").await;
    let id_right = id_of(&app, &root, "dsub-right").await;

    assert_eq!(delete(&left, id_right.clone()).await, StatusCode::FORBIDDEN);
    assert_eq!(delete(&left, id_root).await, StatusCode::FORBIDDEN);
    assert_eq!(delete(&left, id_left).await, StatusCode::BAD_REQUEST);

    assert_eq!(row_of(&app, &root, "dsub-right").await["is_admin"], true);
    assert_eq!(row_of(&app, &root, "dsub-left").await["is_admin"], true);

    db.drop().await.unwrap();
}

/// The branch has to come down too: pointing at an id that no longer exists
/// would strand those accounts where nothing walking upward could reach them.
#[tokio::test]
async fn deleting_an_admin_demotes_the_branch_below_them() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "dcasc-root").await;
    let mid = promote(&app, &root, "dcasc-mid").await;
    let leaf = promote(&app, &mid, "dcasc-leaf").await;
    promote(&app, &leaf, "dcasc-deep").await;

    let id_mid = id_of(&app, &root, "dcasc-mid").await;
    let deleted = app
        .clone()
        .oneshot(authed_request(
            "DELETE",
            &format!("/api/admin/users/{id_mid}"),
            &root,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(
        body_json(deleted).await["demoted"],
        2,
        "the two admins below the deleted one"
    );

    for name in ["dcasc-leaf", "dcasc-deep"] {
        let row = row_of(&app, &root, name).await;
        assert_eq!(row["is_admin"], false, "{name} kept the flag");
        assert!(
            row["promoted_by"].is_null(),
            "{name} still points at an account that no longer exists"
        );
    }

    // And they really are out, not just relabelled.
    let locked_out = app
        .oneshot(authed_get("/api/admin/users", &leaf))
        .await
        .unwrap();
    assert_eq!(locked_out.status(), StatusCode::FORBIDDEN);

    db.drop().await.unwrap();
}

/// Demoting has the same two answers as deleting: take the branch down, or
/// hand it to the demoted admin's own parent.
#[tokio::test]
async fn demoting_an_admin_can_hand_their_branch_to_their_own_parent() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "rrep-root").await;
    let mid = promote(&app, &root, "rrep-mid").await;
    promote(&app, &mid, "rrep-leaf").await;

    let id_mid = id_of(&app, &root, "rrep-mid").await;
    let id_root = id_of(&app, &root, "rrep-root").await;
    let response = set_admin(
        &app,
        &root,
        &id_mid,
        json!({"is_admin": false, "orphans": "reparent"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["demoted"], 1, "only the admin who was asked about");
    assert_eq!(body["reparented"], 1, "the admin directly below them");

    let leaf = row_of(&app, &root, "rrep-leaf").await;
    assert_eq!(leaf["is_admin"], true, "the flag was meant to survive");
    assert_eq!(leaf["promoted_by"], id_root);
    assert_eq!(leaf["admin_level"], 1);
    assert_eq!(row_of(&app, &root, "rrep-mid").await["is_admin"], false);

    db.drop().await.unwrap();
}

/// The other way to dispose of a deleted admin's branch: keep their standing
/// and move them up one level, onto whoever promoted the account being removed.
#[tokio::test]
async fn deleting_an_admin_can_hand_their_branch_to_their_own_parent() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "drep-root").await;
    let mid = promote(&app, &root, "drep-mid").await;
    promote(&app, &mid, "drep-leaf").await;

    let id_mid = id_of(&app, &root, "drep-mid").await;
    let id_root = id_of(&app, &root, "drep-root").await;
    let deleted = app
        .clone()
        .oneshot(authed_request(
            "DELETE",
            &format!("/api/admin/users/{id_mid}?orphans=reparent"),
            &root,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let outcome = body_json(deleted).await;
    assert_eq!(outcome["reparented"], 1, "the admin directly below");
    assert_eq!(outcome["demoted"], 0, "nobody lost the flag");

    let leaf = row_of(&app, &root, "drep-leaf").await;
    assert_eq!(leaf["is_admin"], true, "the flag was meant to survive");
    assert_eq!(
        leaf["promoted_by"], id_root,
        "they should hang from the deleted account's own parent"
    );
    // And one level shallower than they were, since a level disappeared.
    assert_eq!(leaf["admin_level"], 1);

    db.drop().await.unwrap();
}

/// An admin who no longer wants the responsibility should not have to ask
/// permission — but a root has nobody above them to put it back.
#[tokio::test]
async fn an_admin_can_resign_unless_they_are_the_root() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "res-root").await;
    let mid = promote(&app, &root, "res-mid").await;
    let leaf = promote(&app, &mid, "res-leaf").await;

    let id_root = id_of(&app, &root, "res-root").await;
    let id_mid = id_of(&app, &root, "res-mid").await;

    // A root resigning would cascade through every admin.
    let root_quits = set_admin(&app, &root, &id_root, json!({"is_admin": false})).await;
    assert_eq!(root_quits.status(), StatusCode::BAD_REQUEST);
    assert_eq!(row_of(&app, &root, "res-root").await["is_admin"], true);

    // A promoted admin takes their branch, as any demotion does.
    let resigned = set_admin(&app, &mid, &id_mid, json!({"is_admin": false})).await;
    assert_eq!(resigned.status(), StatusCode::OK);
    let body = body_json(resigned).await;
    assert_eq!(body["is_admin"], false);
    assert_eq!(body["demoted"], 2, "themselves and the admin below them");

    for cookie in [&mid, &leaf] {
        let locked_out = app
            .clone()
            .oneshot(authed_get("/api/admin/users", cookie))
            .await
            .unwrap();
        assert_eq!(locked_out.status(), StatusCode::FORBIDDEN);
    }
    assert_eq!(row_of(&app, &root, "res-mid").await["is_admin"], false);

    // And whoever promoted them can put it back.
    let restored = set_admin(&app, &root, &id_mid, json!({"is_admin": true})).await;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(row_of(&app, &root, "res-mid").await["admin_level"], 1);

    db.drop().await.unwrap();
}

/// Resigning is the only thing an admin may do to their own standing.
#[tokio::test]
async fn an_admin_cannot_promote_or_move_themselves() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "self-root").await;
    let left = promote(&app, &root, "self-left").await;
    promote(&app, &root, "self-right").await;

    let id_left = id_of(&app, &root, "self-left").await;
    let id_right = id_of(&app, &root, "self-right").await;

    // Re-granting your own flag would reset your own place in the chain.
    let self_promote = set_admin(&app, &left, &id_left, json!({"is_admin": true})).await;
    assert_eq!(self_promote.status(), StatusCode::BAD_REQUEST);

    // And moving yourself is how you would leave the branch you were put in.
    let self_move = set_admin(
        &app,
        &left,
        &id_left,
        json!({"is_admin": true, "promoted_by": id_right}),
    )
    .await;
    assert_eq!(self_move.status(), StatusCode::BAD_REQUEST);

    let unchanged = row_of(&app, &root, "self-left").await;
    assert_eq!(unchanged["admin_level"], 1);
    assert_eq!(
        unchanged["promoted_by"],
        id_of(&app, &root, "self-root").await
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn settings_round_trip_without_ever_returning_a_secret() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "set-boss").await;

    let initial = app
        .clone()
        .oneshot(authed_get("/api/admin/settings", &boss))
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let body = body_json(initial).await;
    assert_eq!(
        body["password"]["enabled"], true,
        "password is on by default"
    );
    assert_eq!(body["github"]["enabled"], false);
    assert_eq!(body["github"]["has_secret"], false);

    let saved = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/admin/settings",
            &boss,
            json!({
                "password": {"enabled": true},
                "github": {"enabled": true, "client_id": "gh-id", "client_secret": "gh-secret"},
                "google": {"enabled": false},
                "facebook": {"enabled": false},
            }),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let body = body_json(saved).await;
    assert_eq!(body["github"]["client_id"], "gh-id");
    assert_eq!(body["github"]["has_secret"], true);
    assert!(
        !body.to_string().contains("gh-secret"),
        "the secret must never come back out"
    );

    // The page cannot send back a secret it was never given.
    let resaved = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/admin/settings",
            &boss,
            json!({
                "password": {"enabled": true},
                "github": {"enabled": true, "client_id": "gh-id-2"},
                "google": {"enabled": false},
                "facebook": {"enabled": false},
            }),
        ))
        .await
        .unwrap();
    let body = body_json(resaved).await;
    assert_eq!(body["github"]["client_id"], "gh-id-2");
    assert_eq!(body["github"]["has_secret"], true, "secret survived");

    // An explicit empty string is how a credential is retired.
    let cleared = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/admin/settings",
            &boss,
            json!({
                "password": {"enabled": true},
                "github": {"enabled": true, "client_id": "gh-id-2", "client_secret": ""},
                "google": {"enabled": false},
                "facebook": {"enabled": false},
            }),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(cleared).await["github"]["has_secret"], false);

    // The settings survive a reload rather than living in the handler.
    let reloaded = app
        .clone()
        .oneshot(authed_get("/api/admin/settings", &boss))
        .await
        .unwrap();
    assert_eq!(body_json(reloaded).await["github"]["client_id"], "gh-id-2");

    db.drop().await.unwrap();
}

/// The public login page reads the same document, but only ever learns which
/// methods to offer.
#[tokio::test]
async fn saved_settings_drive_the_public_methods_endpoint() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "meth-boss").await;

    let configure = |secret: &'static str| {
        let (app, boss) = (app.clone(), boss.clone());
        async move {
            app.oneshot(authed_request(
                "PUT",
                "/api/admin/settings",
                &boss,
                json!({
                    "password": {"enabled": false},
                    "github": {"enabled": true, "client_id": "gh-id", "client_secret": secret},
                    "google": {"enabled": false},
                    "facebook": {"enabled": false},
                }),
            ))
            .await
            .unwrap()
        }
    };

    assert_eq!(configure("gh-secret").await.status(), StatusCode::OK);
    let methods = app
        .clone()
        .oneshot(request("GET", "/api/auth/methods"))
        .await
        .unwrap();
    let body = body_json(methods).await;
    assert_eq!(body["github"], true);
    assert_eq!(body["password"], false);
    assert!(
        !body.to_string().contains("gh-id"),
        "the public endpoint says which methods, not how they are configured"
    );

    // Enabled without credentials is not offered: the redirect would break.
    assert_eq!(configure("").await.status(), StatusCode::OK);
    let methods = app
        .oneshot(request("GET", "/api/auth/methods"))
        .await
        .unwrap();
    assert_eq!(body_json(methods).await["github"], false);

    db.drop().await.unwrap();
}

/// The sign-in settings are the root's alone. They decide how *everyone*
/// authenticates — including whether password login exists at all — so they are
/// deployment configuration rather than day-to-day administration.
#[tokio::test]
async fn only_the_root_admin_can_see_or_change_the_settings() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "cfg-root").await;
    let promoted = promote(&app, &root, "cfg-deputy").await;

    // The deputy runs the user list perfectly well...
    let users = app
        .clone()
        .oneshot(authed_get("/api/admin/users", &promoted))
        .await
        .unwrap();
    assert_eq!(users.status(), StatusCode::OK);

    // ...but the settings are not theirs.
    let reading = app
        .clone()
        .oneshot(authed_get("/api/admin/settings", &promoted))
        .await
        .unwrap();
    assert_eq!(reading.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(reading).await["error"]["code"], "forbidden");

    let writing = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/admin/settings",
            &promoted,
            json!({
                "password": {"enabled": false},
                "github": {"enabled": false},
                "google": {"enabled": false},
                "facebook": {"enabled": false},
            }),
        ))
        .await
        .unwrap();
    assert_eq!(writing.status(), StatusCode::FORBIDDEN);

    // And the refusal was not a silent no-op: password login is still on.
    let unchanged = app
        .oneshot(authed_get("/api/admin/settings", &root))
        .await
        .unwrap();
    assert_eq!(body_json(unchanged).await["password"]["enabled"], true);

    db.drop().await.unwrap();
}

/// Settings are at least as closed as the user list.
#[tokio::test]
async fn settings_are_closed_to_anonymous_and_ordinary_users() {
    let (app, db) = test_app().await;
    let plain = account(&app, "settings-nobody").await;

    for (method, cookie) in [("GET", None), ("PUT", None)] {
        let response = app
            .clone()
            .oneshot(match cookie {
                Some(c) => authed_request(method, "/api/admin/settings", c, json!({})),
                None => request(method, "/api/admin/settings"),
            })
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method}");
    }

    let ordinary = app
        .clone()
        .oneshot(authed_get("/api/admin/settings", &plain))
        .await
        .unwrap();
    assert_eq!(ordinary.status(), StatusCode::FORBIDDEN);

    let writing = app
        .oneshot(authed_request(
            "PUT",
            "/api/admin/settings",
            &plain,
            json!({
                "password": {"enabled": false},
                "github": {"enabled": false},
                "google": {"enabled": false},
                "facebook": {"enabled": false},
            }),
        ))
        .await
        .unwrap();
    assert_eq!(writing.status(), StatusCode::FORBIDDEN);

    db.drop().await.unwrap();
}

/// The nav has to know who the root is before any list has loaded, so the
/// session itself carries it.
#[tokio::test]
async fn the_session_says_whether_you_are_the_root() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "flag-root").await;
    let deputy = promote(&app, &root, "flag-deputy").await;
    let plain = account(&app, "flag-plain").await;

    let me = |cookie: &str| {
        let (app, cookie) = (app.clone(), cookie.to_string());
        async move {
            body_json(
                app.oneshot(authed_get("/api/auth/me", &cookie))
                    .await
                    .unwrap(),
            )
            .await
        }
    };

    let root_me = me(&root).await;
    assert_eq!(root_me["is_admin"], true);
    assert_eq!(root_me["is_root"], true);

    let deputy_me = me(&deputy).await;
    assert_eq!(deputy_me["is_admin"], true);
    assert_eq!(deputy_me["is_root"], false, "promoted, so not the root");

    let plain_me = me(&plain).await;
    assert_eq!(plain_me["is_admin"], false);
    assert_eq!(plain_me["is_root"], false);

    db.drop().await.unwrap();
}

/// A grant naming no parent leaves an existing admin where they are: the
/// alternative lets a stray toggle re-parent a whole branch.
#[tokio::test]
async fn re_granting_an_existing_admin_does_not_move_them() {
    let (app, db) = test_app().await;
    let root = admin(&app, &db, "idem-root").await;
    let mid = promote(&app, &root, "idem-mid").await;
    promote(&app, &mid, "idem-leaf").await;

    let id_mid = id_of(&app, &root, "idem-mid").await;
    let id_leaf = id_of(&app, &root, "idem-leaf").await;

    // Under the old default this pulled them up to depth 1.
    let again = set_admin(&app, &root, &id_leaf, json!({"is_admin": true})).await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(body_json(again).await["promoted_by"], id_mid);

    let unchanged = row_of(&app, &root, "idem-leaf").await;
    assert_eq!(unchanged["admin_level"], 2, "still below the middle admin");
    assert_eq!(unchanged["promoted_by"], id_mid);

    // Naming the parent is how a move is asked for, and that still works.
    let id_root = id_of(&app, &root, "idem-root").await;
    let moved = set_admin(
        &app,
        &root,
        &id_leaf,
        json!({"is_admin": true, "promoted_by": id_root}),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    assert_eq!(row_of(&app, &root, "idem-leaf").await["admin_level"], 1);

    account(&app, "idem-new").await;
    let id_new = id_of(&app, &root, "idem-new").await;
    assert_eq!(
        set_admin(&app, &mid, &id_new, json!({"is_admin": true}))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(row_of(&app, &root, "idem-new").await["promoted_by"], id_mid);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_bulk_promote_grants_every_flag_in_one_request() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "bulkboss").await;
    account(&app, "one").await;
    account(&app, "two").await;
    let ids = vec![
        id_of(&app, &boss, "one").await,
        id_of(&app, &boss, "two").await,
    ];

    let response = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": ids, "action": "promote"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["affected"], 2);
    assert_eq!(row_of(&app, &boss, "one").await["is_admin"], true);
    assert_eq!(row_of(&app, &boss, "two").await["is_admin"], true);

    db.drop().await.unwrap();
}

/// A parent and its child in the same selection. Whichever order they arrive
/// in, the parent's cascade must not turn the child's turn into an error.
#[tokio::test]
async fn a_bulk_demote_survives_a_parent_and_its_child_together() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "bulkboss").await;
    let parent_cookie = promote(&app, &boss, "parent").await;
    promote(&app, &parent_cookie, "child").await;
    let parent = id_of(&app, &boss, "parent").await;
    let child = id_of(&app, &boss, "child").await;

    // Parent first, which is the order that would strand the child's call.
    let response = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": [parent, child], "action": "demote"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["affected"], 2);
    assert_eq!(row_of(&app, &boss, "parent").await["is_admin"], false);
    assert_eq!(row_of(&app, &boss, "child").await["is_admin"], false);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn a_bulk_delete_orphans_the_links_and_reports_the_deadline() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "bulkboss").await;
    let owner = account(&app, "linkowner").await;
    for code in ["a", "b"] {
        app.clone()
            .oneshot(authed_request(
                "POST",
                "/api/urls",
                &owner,
                json!({"code": code, "url": "https://target.example"}),
            ))
            .await
            .unwrap();
    }
    let id = id_of(&app, &boss, "linkowner").await;

    let response = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": [id], "action": "delete"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["affected"], 1);
    assert_eq!(body["orphaned"], 2);
    assert_eq!(body["grace_days"], 7);

    let link = db
        .collection::<mongodb::bson::Document>("urls")
        .find_one(doc! {"code": "a"})
        .await
        .unwrap()
        .unwrap();
    assert!(link.get("owner").is_none());
    assert!(link.get_datetime("expires_at").is_ok());

    db.drop().await.unwrap();
}

/// Every id is checked before anything is written, and the whole selection is
/// judged as one — so one refusal leaves the rest exactly as it was, and the
/// answer says which ids were the problem rather than only the first.
#[tokio::test]
async fn a_refused_id_rolls_the_whole_selection_back() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "bulkboss").await;
    account(&app, "bystander").await;
    let me = id_of(&app, &boss, "bulkboss").await;
    let other = id_of(&app, &boss, "bystander").await;

    let response = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": [other, me, "no-such-id"], "action": "delete"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let rejected = body_json(response).await["error"]["rejected"].clone();
    let rejected = rejected.as_array().expect("every bad id is named");
    assert_eq!(rejected.len(), 2, "both bad ids, not just the first");
    assert_eq!(rejected[0]["id"], me);
    assert_eq!(rejected[0]["code"], "validation");
    assert_eq!(rejected[1]["id"], "no-such-id");
    assert_eq!(rejected[1]["code"], "not_found");

    assert!(
        db.collection::<mongodb::bson::Document>("users")
            .find_one(doc! {"username": "bystander"})
            .await
            .unwrap()
            .is_some(),
        "the rest of the selection is untouched"
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn bulk_refuses_an_empty_selection_and_an_absurd_one() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "boss").await;

    let empty = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": [], "action": "promote"}),
        ))
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let ids: Vec<String> = (0..101).map(|n| format!("id-{n}")).collect();
    let huge = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": ids, "action": "promote"}),
        ))
        .await
        .unwrap();
    assert_eq!(huge.status(), StatusCode::BAD_REQUEST);

    let plain = account(&app, "ordinary").await;
    let refused = app
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &plain,
            json!({"ids": ["whoever"], "action": "promote"}),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    db.drop().await.unwrap();
}

/// Switches `failCommand` on for one collection, or off. Scoped by namespace:
/// the suite shares one mongod, so an unscoped failpoint hits other tests.
async fn fail_updates_after_the_first(db: &mongodb::Database, on: bool) {
    let admin_db = db.client().database("admin");
    let command = if on {
        doc! {
            "configureFailPoint": "failCommand",
            // Not `times: 1`: the first write must land to have something to undo.
            "mode": {"skip": 1},
            "data": {
                "failCommands": ["update"],
                "namespace": format!("{}.users", db.name()),
                "errorCode": 8,
            },
        }
    } else {
        doc! {"configureFailPoint": "failCommand", "mode": "off"}
    };
    admin_db.run_command(command).await.unwrap();
}

/// The rollback itself, which no other test reaches — the rest are caught
/// before any write, so they pass with the transaction taken out.
#[tokio::test]
async fn a_write_failing_partway_undoes_the_writes_before_it() {
    let (app, db) = test_app().await;
    let boss = admin(&app, &db, "bulkboss").await;
    account(&app, "first").await;
    account(&app, "second").await;
    let ids = vec![
        id_of(&app, &boss, "first").await,
        id_of(&app, &boss, "second").await,
    ];

    fail_updates_after_the_first(&db, true).await;
    let response = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &boss,
            json!({"ids": ids, "action": "promote"}),
        ))
        .await
        .unwrap();
    fail_updates_after_the_first(&db, false).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        row_of(&app, &boss, "first").await["is_admin"],
        false,
        "the promotion that succeeded must not survive the one that did not"
    );
    assert_eq!(row_of(&app, &boss, "second").await["is_admin"], false);

    db.drop().await.unwrap();
}

/// An admin dragged up past two of the selected accounts moves twice and keeps
/// one flag, so it must be counted once.
#[tokio::test]
async fn an_admin_moved_up_twice_is_reported_once() {
    let (app, db) = test_app().await;
    let a = admin(&app, &db, "tree-a").await;
    let b = promote(&app, &a, "tree-b").await;
    let c = promote(&app, &b, "tree-c").await;
    promote(&app, &c, "tree-d").await;
    promote(&app, &c, "tree-e").await;
    promote(&app, &b, "tree-f").await;

    let ids = vec![
        id_of(&app, &a, "tree-b").await,
        id_of(&app, &a, "tree-c").await,
        id_of(&app, &a, "tree-d").await,
    ];
    let response = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/admin/users/bulk",
            &a,
            json!({"ids": ids, "action": "demote", "orphans": "reparent"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await;
    assert_eq!(body["affected"], 3);
    assert_eq!(body["demoted"], 0, "nothing below the selection lost it");
    assert_eq!(
        body["reparented"], 2,
        "E and F kept the flag; E climbing two levels is still one account"
    );

    assert_eq!(row_of(&app, &a, "tree-e").await["is_admin"], true);
    assert_eq!(row_of(&app, &a, "tree-f").await["is_admin"], true);
    for gone in ["tree-b", "tree-c", "tree-d"] {
        assert_eq!(row_of(&app, &a, gone).await["is_admin"], false);
    }

    db.drop().await.unwrap();
}
