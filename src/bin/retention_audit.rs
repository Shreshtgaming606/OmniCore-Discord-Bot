//! Read-only inventory. Prints counts and schema metadata, never document contents or IDs.
use futures::TryStreamExt;
use mongodb::{
    Client,
    bson::{Bson, Document, doc},
};

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "Read-only audit could not complete; check MongoDB connectivity and permissions."
        );
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::from_filename(".env.runtime").ok();
    dotenvy::dotenv().ok();
    let uri = std::env::var("MONGO_URI")?;
    let client = Client::with_uri_str(&uri).await?;
    let db = client.database("omnicore_bot");
    let mut names = db.list_collection_names().await?;
    names.sort();
    println!("Read-only collection inventory:");
    for name in &names {
        let collection = db.collection::<Document>(name);
        let count = collection.count_documents(doc! {}).await?;
        let dated = collection
            .count_documents(doc! { "expiresAt": { "$type": "date" } })
            .await?;
        let string_dates = collection
            .count_documents(doc! { "expiresAt": { "$type": "string" } })
            .await?;
        let missing_dates = collection
            .count_documents(doc! { "expiresAt": { "$exists": false } })
            .await?;
        let created = collection
            .count_documents(doc! { "createdAt": { "$type": "date" } })
            .await?;
        println!(
            "{name}: documents={count}, expiresAt_date={dated}, expiresAt_string={string_dates}, expiresAt_missing={missing_dates}, createdAt_date={created}"
        );
        let indexes: Vec<_> = collection.list_indexes().await?.try_collect().await?;
        for index in indexes {
            let ttl = index
                .options
                .as_ref()
                .and_then(|options| options.expire_after)
                .map(|duration| duration.as_secs());
            println!(
                "  index keys={:?}, expireAfterSeconds={ttl:?}",
                index.keys.keys().collect::<Vec<_>>()
            );
        }
    }
    if names.iter().any(|name| name == "messages") {
        let collection = db.collection::<Document>("messages");
        let mut cursor = collection
            .find(doc! {})
            .projection(doc! { "messages.timestamp": 1, "messages.role": 1 })
            .await?;
        let cutoff = chrono::Utc::now().timestamp_millis() - 30 * 24 * 60 * 60 * 1_000;
        let (mut recent, mut old, mut unknown) = (0_u64, 0_u64, 0_u64);
        while let Some(guild_doc) = cursor.try_next().await? {
            if let Ok(turns) = guild_doc.get_array("messages") {
                for turn in turns {
                    let Some(turn) = turn.as_document() else {
                        unknown += 1;
                        continue;
                    };
                    let timestamp = match turn.get("timestamp") {
                        Some(Bson::Int64(value)) => value.checked_mul(1_000),
                        Some(Bson::Int32(value)) => Some(i64::from(*value) * 1_000),
                        Some(Bson::DateTime(value)) => Some(value.timestamp_millis()),
                        Some(Bson::String(value)) => chrono::DateTime::parse_from_rfc3339(value)
                            .ok()
                            .map(|date| date.timestamp_millis()),
                        _ => None,
                    };
                    match timestamp {
                        Some(time) if time >= cutoff => recent += 1,
                        Some(_) => old += 1,
                        None => unknown += 1,
                    }
                }
            }
        }
        println!(
            "legacy messages: turns_within_30_days={recent}, older_turns={old}, unparseable_or_missing_timestamp={unknown}"
        );
    }
    println!("No database writes were performed.");
    Ok(())
}
