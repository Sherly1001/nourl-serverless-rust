mod helpers;

/// Stand-in for a PHC string; these tests never exercise argon2 itself.
const FAKE_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$fake$fake";

use backend::users;

#[tokio::test]
async fn create_then_find_by_username_and_id() {
    let db = helpers::test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();

    let created = users::create(
        &db,
        users::NewUser::with_password_hash("alice", FAKE_HASH.into()),
    )
    .await
    .unwrap();
    assert_eq!(created.username, "alice");
    assert!(!created.is_admin, "registration must never mint an admin");
    assert_eq!(created.token_version, 0);
    assert!(!created.id.is_empty());

    let by_name = users::find_by_username(&db, "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_name.id, created.id);
    let by_id = users::find_by_id(&db, &created.id).await.unwrap().unwrap();
    assert_eq!(by_id.username, "alice");

    assert!(
        users::find_by_username(&db, "nobody")
            .await
            .unwrap()
            .is_none()
    );

    db.drop().await.unwrap();
}

#[tokio::test]
async fn usernames_are_unique() {
    let db = helpers::test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();

    users::create(
        &db,
        users::NewUser::with_password_hash("dup", FAKE_HASH.into()),
    )
    .await
    .unwrap();
    assert!(
        users::create(
            &db,
            users::NewUser::with_password_hash("dup", FAKE_HASH.into())
        )
        .await
        .is_err()
    );

    db.drop().await.unwrap();
}

/// Sparse skips only missing fields, so a sparse unique index would reject the
/// second account carrying an explicit `null` provider id.
#[tokio::test]
async fn explicit_null_provider_ids_do_not_collide() {
    let db = helpers::test_db().await;
    let users = db.collection::<mongodb::bson::Document>("users");
    for (id, username) in [("id-a", "user-a"), ("id-b", "user-b")] {
        users
            .insert_one(mongodb::bson::doc! {
                "id": id,
                "username": username,
                "github_id": null,
                "google_id": null,
                "facebook_id": null,
            })
            .await
            .unwrap();
    }

    backend::db::ensure_indexes(&db).await.unwrap();

    let loaded = users::find_by_id(&db, "id-a").await.unwrap().unwrap();
    assert_eq!(loaded.username, "user-a");
    assert!(!loaded.is_admin, "absent is_admin must not read as admin");
    assert_eq!(loaded.token_version, 0);
    assert!(loaded.hash_passwd.is_none());

    db.drop().await.unwrap();
}

#[tokio::test]
async fn profile_fields_are_settable_at_creation_and_editable_after() {
    let db = helpers::test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();

    // A password signup carries no profile data; display_name falls back.
    let plain = users::create(
        &db,
        users::NewUser::with_password_hash("plain", FAKE_HASH.into()),
    )
    .await
    .unwrap();
    assert_eq!(plain.display_name.as_deref(), Some("plain"));
    assert!(plain.email.is_none());
    assert!(plain.avatar_url.is_none());

    // An OAuth signup (phase 2b) supplies all three up front.
    let rich = users::create(
        &db,
        users::NewUser {
            username: "rich".into(),
            hash_passwd: None,
            display_name: Some("Rich Person".into()),
            email: Some("rich@example.com".into()),
            avatar_url: Some("https://example.com/a.png".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(rich.display_name.as_deref(), Some("Rich Person"));
    assert_eq!(rich.email.as_deref(), Some("rich@example.com"));
    assert!(rich.hash_passwd.is_none());

    // A partial update leaves untouched fields alone.
    users::update_profile(
        &db,
        &rich.id,
        shared::UpdateProfileRequest {
            display_name: Some("Renamed".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let after = users::find_by_id(&db, &rich.id).await.unwrap().unwrap();
    assert_eq!(after.display_name.as_deref(), Some("Renamed"));
    assert_eq!(
        after.avatar_url.as_deref(),
        Some("https://example.com/a.png"),
        "an omitted field must not be cleared"
    );

    db.drop().await.unwrap();
}

/// `$inc` past `i32::MAX` does not error or wrap — Mongo quietly rewrites the
/// field as an int64. An i32 field would stop deserializing at that point and
/// every request for that account would 500, with no way back into range.
#[tokio::test]
async fn token_version_survives_promotion_past_i32() {
    let db = helpers::test_db().await;
    let raw = db.collection::<mongodb::bson::Document>("users");
    raw.insert_one(mongodb::bson::doc! {
        "id": "overflow",
        "username": "overflow",
        "token_version": i32::MAX,
    })
    .await
    .unwrap();

    users::bump_token_version(&db, "overflow").await.unwrap();

    let loaded = users::find_by_id(&db, "overflow").await.unwrap().unwrap();
    assert_eq!(loaded.token_version, i32::MAX as i64 + 1);

    db.drop().await.unwrap();
}

/// Storing the password and revoking the old sessions is one update. Two would
/// leave a window where the new password works and every session signed under
/// the old one still does — the thing a password change is meant to end.
#[tokio::test]
async fn setting_a_password_revokes_outstanding_tokens_in_the_same_update() {
    let db = helpers::test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();

    let user = users::create(
        &db,
        users::NewUser::with_password_hash("rotator", FAKE_HASH.into()),
    )
    .await
    .unwrap();

    users::set_password(&db, &user.id, "second-hash")
        .await
        .unwrap();

    let reloaded = users::find_by_id(&db, &user.id).await.unwrap().unwrap();
    assert_eq!(reloaded.hash_passwd.as_deref(), Some("second-hash"));
    assert_eq!(reloaded.token_version, user.token_version + 1);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn bump_token_version_invalidates_old_snapshots() {
    let db = helpers::test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();

    let user = users::create(
        &db,
        users::NewUser::with_password_hash("bumper", FAKE_HASH.into()),
    )
    .await
    .unwrap();
    users::bump_token_version(&db, &user.id).await.unwrap();

    let reloaded = users::find_by_id(&db, &user.id).await.unwrap().unwrap();
    assert_eq!(reloaded.token_version, user.token_version + 1);

    db.drop().await.unwrap();
}
