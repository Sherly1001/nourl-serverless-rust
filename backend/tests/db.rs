use futures::TryStreamExt;
use mongodb::bson::doc;

async fn test_db() -> mongodb::Database {
    let url = std::env::var("MONGO_URL").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into());
    let name = format!("nourl_test_{}", uuid::Uuid::new_v4().simple());
    backend::db::connect(&url, &name).await.unwrap()
}

#[tokio::test]
async fn creates_indexes() {
    let db = test_db().await;
    backend::db::ensure_indexes(&db).await.unwrap();
    let names = db
        .collection::<mongodb::bson::Document>("urls")
        .list_index_names()
        .await
        .unwrap();
    assert!(names.iter().any(|n| n == "code_1"), "{names:?}");
    assert!(names.iter().any(|n| n == "expires_at_ttl"), "{names:?}");
    db.drop().await.unwrap();
}

#[tokio::test]
async fn pipeline_joins_owner_and_strips_secrets() {
    let db = test_db().await;
    db.collection("users")
        .insert_one(doc! {
            "id": "u1", "username": "sher", "hash_passwd": "x",
            "github_id": "g", "google_id": "go", "facebook_id": "f", "token_version": 3
        })
        .await
        .unwrap();
    db.collection("urls")
        .insert_one(doc! {"code": "abc", "url": "https://a.com", "owner": "u1"})
        .await
        .unwrap();

    let pipeline = backend::db::url_aggregate_pipeline(doc! {"code": "abc"}, 20, 0, doc! {});
    let rows: Vec<mongodb::bson::Document> = db
        .collection::<mongodb::bson::Document>("urls")
        .aggregate(pipeline)
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    let owner = rows[0].get_document("owner").unwrap();
    assert_eq!(owner.get_str("username").unwrap(), "sher");
    for secret in [
        "hash_passwd",
        "github_id",
        "google_id",
        "facebook_id",
        "token_version",
        "id",
    ] {
        assert!(!owner.contains_key(secret), "leaked {secret}");
    }
    db.drop().await.unwrap();
}
