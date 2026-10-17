//! Legacy embedded AI history migration. Dry run by default; never run --apply without a reviewed backup and counts.
use futures::TryStreamExt;
use mongodb::{
    Client,
    bson::{Bson, DateTime, Document, doc},
    options::UpdateOptions,
};
use std::collections::HashMap;

const DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
type Counts = (u64, u64, u64, u64, u64); // documents, recent, expired, unknown, skipped

fn timestamp_millis(turn: &Document) -> Option<i64> {
    match turn.get("timestamp") {
        Some(Bson::Int64(value)) => value.checked_mul(1_000),
        Some(Bson::Int32(value)) => Some(i64::from(*value) * 1_000),
        Some(Bson::DateTime(value)) => Some(value.timestamp_millis()),
        Some(Bson::String(value)) => chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|time| time.timestamp_millis()),
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("Migration could not complete; inspect the database before retrying.");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let expected = if args.is_empty() {
        None
    } else if args.len() == 5 && args[0] == "--apply" && args[1] == "--reviewed-backup-and-counts" {
        Some((
            args[2].parse::<u64>()?,
            args[3].parse::<u64>()?,
            args[4].parse::<u64>()?,
        ))
    } else {
        eprintln!(
            "Usage: retention_migrate [--apply --reviewed-backup-and-counts EXPECTED_DOCUMENTS EXPECTED_RECENT_TURNS EXPECTED_EXPIRED_TURNS]"
        );
        std::process::exit(2);
    };
    let apply = expected.is_some();
    dotenvy::from_filename(".env.runtime").ok();
    dotenvy::dotenv().ok();
    let uri = std::env::var("MONGO_URI")?;
    let client = Client::with_uri_str(&uri).await?;
    let db = client.database("omnicore_bot");
    let legacy = db.collection::<Document>("messages");
    let turns = db.collection::<Document>("ai_turns");
    let cutoff = DateTime::now().timestamp_millis() - DAYS * DAY_MS;
    if apply {
        let (documents, recent, expired, unknown, skipped) = scan_legacy(&legacy, cutoff).await?;
        println!(
            "preflight: legacy_guild_documents={documents}, recent_turns={recent}, expired_turns={expired}, unknown_timestamp_turns={unknown}, skipped_guild_documents={skipped}"
        );
        if Some((documents, recent, expired)) != expected || unknown != 0 || skipped != 0 {
            eprintln!(
                "Preflight counts differ from reviewed values or contain unparseable records; no migration writes performed."
            );
            std::process::exit(2);
        }
        let indexes: Vec<_> = turns.list_indexes().await?.try_collect().await?;
        let ttl_ready = indexes.iter().any(|index| {
            index.keys == doc! { "expiresAt": 1 }
                && index
                    .options
                    .as_ref()
                    .and_then(|options| options.expire_after)
                    == Some(std::time::Duration::ZERO)
        });
        if !ttl_ready {
            eprintln!("ai_turns TTL index is not verified; migration stopped.");
            std::process::exit(2);
        }
    }
    let mut cursor = legacy.find(doc! {}).await?;
    let (mut guild_docs, mut recent, mut expired, mut unknown, mut skipped) =
        (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    while let Some(guild_doc) = cursor.try_next().await? {
        guild_docs += 1;
        let Some(Bson::Array(messages)) = guild_doc.get("messages") else {
            skipped += 1;
            continue;
        };
        let mut parsed = Vec::with_capacity(messages.len());
        let mut has_unknown = false;
        for (position, item) in messages.iter().enumerate() {
            let Some(turn) = item.as_document() else {
                unknown += 1;
                has_unknown = true;
                continue;
            };
            let Some(time) = timestamp_millis(turn) else {
                unknown += 1;
                has_unknown = true;
                continue;
            };
            if time < cutoff {
                expired += 1;
            } else {
                recent += 1;
            }
            parsed.push((position, time, turn));
        }
        if has_unknown {
            skipped += 1;
            continue;
        }
        let Some(guild_id) = guild_doc.get_str("guild_id").ok() else {
            skipped += 1;
            continue;
        };
        let Some(source_id) = guild_doc.get("_id") else {
            skipped += 1;
            continue;
        };
        if !apply {
            continue;
        }
        let request_owners: HashMap<String, String> = parsed
            .iter()
            .filter_map(|(_, _, turn)| {
                if turn.get_str("role").ok() != Some("USER") {
                    return None;
                }
                Some((
                    turn.get_document("message")
                        .ok()?
                        .get_str("id")
                        .ok()?
                        .to_string(),
                    turn.get_document("author")
                        .ok()?
                        .get_str("id")
                        .ok()?
                        .to_string(),
                ))
            })
            .collect();
        for (position, time, turn) in parsed {
            if time < cutoff {
                continue;
            }
            let mut migrated = doc! {
                "_id": format!("legacy:{}:{}", source_id, position),
                "guild_id": guild_id,
                "payload": turn.clone(),
                "createdAt": DateTime::from_millis(time),
                "expiresAt": DateTime::from_millis(time + DAYS * DAY_MS),
            };
            if turn.get_str("role").ok() == Some("USER") {
                if let Ok(user_id) = turn
                    .get_document("author")
                    .and_then(|author| author.get_str("id"))
                {
                    migrated.insert("user_id", user_id);
                    migrated.insert("request_user_id", user_id);
                }
            } else if turn.get_str("role").ok() == Some("ASSISTANT") {
                if let Some(user_id) = turn
                    .get_document("message")
                    .ok()
                    .and_then(|message| message.get_document("replying_to").ok())
                    .and_then(|reply| reply.get_str("id").ok())
                    .and_then(|id| request_owners.get(id))
                {
                    migrated.insert("request_user_id", user_id.clone());
                }
            }
            turns
                .update_one(
                    doc! { "_id": migrated.get("_id").cloned().unwrap() },
                    doc! { "$setOnInsert": migrated },
                )
                .with_options(UpdateOptions::builder().upsert(true).build())
                .await?;
        }
        // Exact array match prevents removing a document changed by an older running bot.
        let result = legacy
            .delete_one(doc! { "_id": source_id.clone(), "messages": messages.clone() })
            .await?;
        if result.deleted_count == 0 {
            skipped += 1;
        }
    }
    println!(
        "mode={}, legacy_guild_documents={}, recent_turns={}, expired_turns={}, unknown_timestamp_turns={}, skipped_guild_documents={}",
        if apply { "apply" } else { "dry-run" },
        guild_docs,
        recent,
        expired,
        unknown,
        skipped
    );
    if !apply {
        println!(
            "No database writes were performed. Apply requires a reviewed backup and explicit flags."
        );
    }
    Ok(())
}

async fn scan_legacy(
    legacy: &mongodb::Collection<Document>,
    cutoff: i64,
) -> mongodb::error::Result<Counts> {
    let mut cursor = legacy
        .find(doc! {})
        .projection(doc! { "guild_id": 1, "messages.timestamp": 1 })
        .await?;
    let (mut documents, mut recent, mut expired, mut unknown, mut skipped) = (0, 0, 0, 0, 0);
    while let Some(guild_doc) = cursor.try_next().await? {
        documents += 1;
        let Some(Bson::Array(messages)) = guild_doc.get("messages") else {
            skipped += 1;
            continue;
        };
        let mut invalid = guild_doc.get_str("guild_id").is_err() || !guild_doc.contains_key("_id");
        for item in messages {
            let time = item.as_document().and_then(timestamp_millis);
            match time {
                Some(time) if time >= cutoff => recent += 1,
                Some(_) => expired += 1,
                None => {
                    unknown += 1;
                    invalid = true;
                }
            }
        }
        if invalid {
            skipped += 1;
        }
    }
    Ok((documents, recent, expired, unknown, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_keeps_original_turn_age() {
        let user = doc! { "timestamp": "2023-11-14T22:13:20Z" };
        let assistant = doc! { "timestamp": 1_700_000_000_i64 };
        assert_eq!(timestamp_millis(&user), Some(1_700_000_000_000));
        assert_eq!(timestamp_millis(&assistant), Some(1_700_000_000_000));
    }
}
