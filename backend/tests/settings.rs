mod helpers;

use backend::settings;
use mongodb::bson::doc;

#[tokio::test]
async fn missing_document_means_password_only() {
    let db = helpers::test_db().await;

    let loaded = settings::load(&db).await.unwrap();
    let methods = loaded.methods();
    assert!(
        methods.password,
        "password login is the default when unconfigured"
    );
    assert!(!methods.github);
    assert!(!methods.google);
    assert!(!methods.facebook);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn stored_document_overrides_defaults() {
    let db = helpers::test_db().await;
    db.collection::<mongodb::bson::Document>("settings")
        .insert_one(doc! {
            "_id": "auth",
            "password": {"enabled": false},
            "github": {"enabled": true, "client_id": "gh-id", "client_secret": "gh-secret"},
        })
        .await
        .unwrap();

    let loaded = settings::load(&db).await.unwrap();
    assert!(!loaded.password.enabled);
    assert!(loaded.github.enabled);
    assert_eq!(loaded.github.client_id.as_deref(), Some("gh-id"));
    // Absent sections keep their defaults rather than exploding.
    assert!(!loaded.google.enabled);

    db.drop().await.unwrap();
}

#[tokio::test]
async fn an_oauth_method_needs_credentials_before_it_counts_as_enabled() {
    let db = helpers::test_db().await;
    db.collection::<mongodb::bson::Document>("settings")
        .insert_one(doc! {
            "_id": "auth",
            // Toggled on in the admin UI but never given credentials: offering
            // it on the login page would send users to a broken redirect.
            "github": {"enabled": true},
            "google": {"enabled": true, "client_id": "g-id"},
            "facebook": {"enabled": true, "client_id": "f-id", "client_secret": "f-secret"},
        })
        .await
        .unwrap();

    let methods = settings::load(&db).await.unwrap().methods();
    assert!(!methods.github, "no client_id or secret");
    assert!(!methods.google, "client_id but no secret");
    assert!(methods.facebook, "fully configured");

    db.drop().await.unwrap();
}
