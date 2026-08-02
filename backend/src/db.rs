use mongodb::bson::{Document, doc};
use mongodb::options::IndexOptions;
use mongodb::{Client, Database, IndexModel};

pub async fn connect(mongo_url: &str, db_name: &str) -> mongodb::error::Result<Database> {
    let mut options = mongodb::options::ClientOptions::parse(mongo_url).await?;
    options.server_selection_timeout = Some(std::time::Duration::from_secs(5));
    let client = Client::with_options(options)?;
    let db = client.database(db_name);
    // The driver is lazy — force a real roundtrip so startup fails fast
    // when the database is unreachable.
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

    // Every unique index here is *partial*, restricted to documents where the
    // field is actually a string. Accounts predating this phase have no
    // `username` at all and store `null` in every provider id, and a plain
    // unique index — sparse or not — treats those as equal values and rejects
    // the second one. Sparse is not enough: it skips missing fields, not
    // explicit nulls.
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
    Ok(())
}

/// Renders a BSON date as an RFC3339 string, because `UrlEntry`'s date fields
/// are `Option<String>` and a raw BSON datetime fails to deserialize into one.
/// Anything that is not a date passes through untouched — legacy documents may
/// already hold a string, and `$dateToString` would abort the whole pipeline on
/// one.
pub fn as_iso_string(field: &str) -> Document {
    let path = format!("${field}");
    doc! {"$cond": [
        {"$eq": [{"$type": &path}, "date"]},
        {"$dateToString": {"date": &path, "format": "%Y-%m-%dT%H:%M:%S.%LZ"}},
        &path,
    ]}
}

/// Mirrors urlAggregatePipeline from the Node.js implementation, plus
/// stripping `token_version` (new field) and `_id`s for clean serde.
pub fn url_aggregate_pipeline(
    filter: Document,
    limit: i64,
    skip: i64,
    sort: Document,
) -> Vec<Document> {
    let mut sort_doc = sort;
    sort_doc.insert("_id", -1);
    vec![
        // Filter before the join, so `owner` is still the raw id string rather
        // than the joined object, and so the index on `code` can be used.
        doc! {"$match": filter},
        doc! {"$lookup": {"from": "users", "localField": "owner", "foreignField": "id", "as": "owner"}},
        doc! {"$set": {"owner": {"$ifNull": [{"$first": "$owner"}, null]}}},
        doc! {"$unset": [
            "_id", "id", "owner._id", "owner.id", "owner.github_id",
            "owner.facebook_id", "owner.google_id", "owner.hash_passwd",
            "owner.token_version", "owner.email", "owner.created_at"
        ]},
        // Before the sort, so ordering by a date orders these strings — the
        // format is fixed-width UTC, so lexicographic is chronological.
        doc! {"$set": {
            "created_at": as_iso_string("created_at"),
            "updated_at": as_iso_string("updated_at"),
            "last_hit_at": as_iso_string("last_hit_at"),
            "expires_at": as_iso_string("expires_at"),
        }},
        doc! {"$sort": sort_doc},
        doc! {"$skip": skip},
        doc! {"$limit": limit},
    ]
}
