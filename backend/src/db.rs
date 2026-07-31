use mongodb::bson::{Document, doc};
use mongodb::options::IndexOptions;
use mongodb::{Client, Database, IndexModel};

pub async fn connect(mongo_url: &str, db_name: &str) -> mongodb::error::Result<Database> {
    let client = Client::with_uri_str(mongo_url).await?;
    Ok(client.database(db_name))
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
    Ok(())
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
        doc! {"$lookup": {"from": "users", "localField": "owner", "foreignField": "id", "as": "owner"}},
        doc! {"$set": {"owner": {"$ifNull": [{"$first": "$owner"}, null]}}},
        doc! {"$unset": [
            "_id", "id", "owner._id", "owner.id", "owner.github_id",
            "owner.facebook_id", "owner.google_id", "owner.hash_passwd",
            "owner.token_version"
        ]},
        doc! {"$match": filter},
        doc! {"$sort": sort_doc},
        doc! {"$skip": skip},
        doc! {"$limit": limit},
    ]
}
