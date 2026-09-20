use mongodb::bson::{Document, doc};
use mongodb::options::IndexOptions;
use mongodb::{Client, Database, IndexModel};

pub async fn connect(mongo_url: &str, db_name: &str) -> mongodb::error::Result<Database> {
    let mut options = mongodb::options::ClientOptions::parse(mongo_url).await?;
    options.server_selection_timeout = Some(std::time::Duration::from_secs(5));
    let client = Client::with_options(options)?;
    let db = client.database(db_name);
    // The driver is lazy; force a roundtrip so startup fails fast.
    db.run_command(doc! {"ping": 1}).await?;
    Ok(db)
}

pub async fn ensure_indexes(db: &Database) -> mongodb::error::Result<()> {
    let urls = db.collection::<Document>("urls");
    urls.create_index(
        IndexModel::builder()
            .keys(doc! {"code": 1})
            .options(IndexOptions::builder().unique(true).build())
            .build(),
    )
    .await?;
    urls.create_index(
        IndexModel::builder()
            .keys(doc! {"expires_at": 1})
            .options(
                IndexOptions::builder()
                    .name("expires_at_ttl".to_string())
                    .expire_after(std::time::Duration::from_secs(0))
                    .partial_filter_expression(doc! {"expires_at": {"$exists": true}})
                    .build(),
            )
            .build(),
    )
    .await?;

    // Partial, not sparse: legacy accounts store explicit nulls, which sparse
    // does not skip and a unique index would treat as equal.
    let users = db.collection::<Document>("users");
    for field in ["id", "username", "github_id", "google_id", "facebook_id"] {
        users
            .create_index(
                IndexModel::builder()
                    .keys(doc! {field: 1})
                    .options(
                        IndexOptions::builder()
                            .unique(true)
                            .partial_filter_expression(doc! {field: {"$type": "string"}})
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }

    // Not optional: without these, the admin list's join and `$graphLookup`
    // both walk the whole collection.
    urls.create_index(IndexModel::builder().keys(doc! {"owner": 1}).build())
        .await?;
    users
        .create_index(IndexModel::builder().keys(doc! {"promoted_by": 1}).build())
        .await?;
    Ok(())
}

/// A BSON date as RFC3339, since the DTOs hold `Option<String>`. Anything else
/// passes through: a legacy string would abort `$dateToString` outright.
pub fn as_iso_string(field: &str) -> Document {
    let path = format!("${field}");
    doc! {"$cond": [
        {"$eq": [{"$type": &path}, "date"]},
        {"$dateToString": {"date": &path, "format": "%Y-%m-%dT%H:%M:%S.%LZ"}},
        &path,
    ]}
}

/// Mirrors the Node implementation, plus stripping `token_version` and `_id`.
pub fn url_aggregate_pipeline(
    filter: Document,
    limit: i64,
    skip: i64,
    sort: Document,
) -> Vec<Document> {
    vec![
        // Before the join, so `owner` is still an id and `code` can use its index.
        doc! {"$match": filter},
        doc! {"$lookup": {"from": "users", "localField": "owner", "foreignField": "id", "as": "owner"}},
        doc! {"$set": {"owner": {"$ifNull": [{"$first": "$owner"}, null]}}},
        // Before the sort: fixed-width UTC, so lexicographic is chronological.
        doc! {"$set": {
            "created_at": as_iso_string("created_at"),
            "updated_at": as_iso_string("updated_at"),
            "last_hit_at": as_iso_string("last_hit_at"),
            "expires_at": as_iso_string("expires_at"),
        }},
        // Callers' sorts end in `_id`, so tied rows still have one total order.
        doc! {"$sort": sort},
        doc! {"$skip": skip},
        doc! {"$limit": limit},
        // Last, because `$sort` above needs the `_id` this drops.
        doc! {"$unset": [
            "_id", "id", "owner._id", "owner.id", "owner.github_id",
            "owner.facebook_id", "owner.google_id", "owner.hash_passwd",
            "owner.token_version", "owner.email", "owner.created_at"
        ]},
    ]
}
