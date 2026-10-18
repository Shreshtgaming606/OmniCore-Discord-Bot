use futures::TryStreamExt;
use mongodb::{
    Client, IndexModel,
    bson::{Bson, DateTime, Document, doc},
    options::{ClientOptions, IndexOptions},
};
use once_cell::sync::OnceCell;
use std::time::Duration;

use crate::retention;

pub static CLIENT: OnceCell<mongodb::Client> = OnceCell::new();
pub const AI_TURNS: &str = "ai_turns";
pub const LEGACY_MESSAGES: &str = "messages";
pub const GUILD_SETTINGS: &str = "per_guild_settings";
pub const LOCKED_CHANNELS: &str = "locked_channels";
pub const AUTOMOD_WARNINGS: &str = "automod_warnings";
pub const AUTOMOD_WARNING_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

pub async fn is_ai_approved(guild_id: u64) -> mongodb::error::Result<bool> {
    let settings = get_collection(GUILD_SETTINGS)?
        .find_one(doc! {"guild_id": guild_id.to_string()})
        .await?;
    Ok(settings
        .as_ref()
        .is_some_and(|document| document.get_bool("ai_approved").unwrap_or(false)))
}

fn guild_filter(guild_id: &str) -> Document {
    doc! { "guild_id": guild_id }
}
fn user_turn_filter(user_id: &str) -> Document {
    doc! { "$or": [{ "user_id": user_id }, { "request_user_id": user_id }] }
}

pub async fn ensure_indexes() -> mongodb::error::Result<()> {
    let client = CLIENT
        .get()
        .ok_or_else(|| mongodb::error::Error::custom("MongoDB client not initialized"))?;
    let db = client.database("omnicore_bot");
    let exists = db
        .list_collection_names()
        .await?
        .iter()
        .any(|name| name == AI_TURNS);
    let collection = get_collection(AI_TURNS)?;
    if exists {
        let total = collection.count_documents(doc! {}).await?;
        let valid_dates = collection
            .count_documents(doc! { "expiresAt": { "$type": "date" } })
            .await?;
        if total != valid_dates {
            return Err(mongodb::error::Error::custom(
                "ai_turns contains missing or non-Date expiresAt values; inspect before startup",
            ));
        }
        let indexes: Vec<IndexModel> = collection.list_indexes().await?.try_collect().await?;
        let ttl_indexes: Vec<&IndexModel> = indexes
            .iter()
            .filter(|index| index.keys.contains_key("expiresAt"))
            .collect();
        if ttl_indexes.len() > 1
            || ttl_indexes.iter().any(|index| {
                index.keys != doc! { "expiresAt": 1 }
                    || index
                        .options
                        .as_ref()
                        .and_then(|options| options.expire_after)
                        != Some(Duration::ZERO)
            })
        {
            return Err(mongodb::error::Error::custom(
                "ai_turns has a conflicting expiresAt index; inspect it before startup",
            ));
        }
        if ttl_indexes.is_empty() && total != 0 {
            return Err(mongodb::error::Error::custom(
                "ai_turns contains documents without a verified TTL index; inspect them before enabling TTL",
            ));
        }
        if !ttl_indexes.is_empty() {
            ensure_automod_warning_index().await?;
            log::info!("[Database] TTL indexes verified.");
            return Ok(());
        }
    }
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! { "expiresAt": 1 })
                .options(
                    IndexOptions::builder()
                        .name("ai_turns_expires_at_ttl".to_string())
                        .expire_after(Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await?;
    ensure_automod_warning_index().await?;
    log::info!("[Database] TTL indexes verified.");
    Ok(())
}

async fn ensure_automod_warning_index() -> mongodb::error::Result<()> {
    let collection = get_collection(AUTOMOD_WARNINGS)?;
    let client = CLIENT
        .get()
        .ok_or_else(|| mongodb::error::Error::custom("MongoDB client not initialized"))?;
    let exists = client
        .database("omnicore_bot")
        .list_collection_names()
        .await?
        .iter()
        .any(|name| name == AUTOMOD_WARNINGS);
    if !exists {
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "expiresAt": 1 })
                    .options(
                        IndexOptions::builder()
                            .name("automod_warnings_expires_at_ttl".to_string())
                            .expire_after(Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await?;
        return Ok(());
    }
    let total = collection.count_documents(doc! {}).await?;
    let valid_dates = collection
        .count_documents(doc! { "expiresAt": { "$type": "date" } })
        .await?;
    if total != valid_dates {
        return Err(mongodb::error::Error::custom(
            "automod_warnings contains missing or non-Date expiresAt values",
        ));
    }
    let indexes: Vec<IndexModel> = collection.list_indexes().await?.try_collect().await?;
    let ttl_indexes: Vec<&IndexModel> = indexes
        .iter()
        .filter(|index| index.keys.contains_key("expiresAt"))
        .collect();
    if ttl_indexes.len() > 1
        || ttl_indexes.iter().any(|index| {
            index.keys != doc! { "expiresAt": 1 }
                || index
                    .options
                    .as_ref()
                    .and_then(|options| options.expire_after)
                    != Some(Duration::ZERO)
        })
    {
        return Err(mongodb::error::Error::custom(
            "automod_warnings has a conflicting expiresAt index; inspect before startup",
        ));
    }
    if ttl_indexes.is_empty() {
        if total != 0 {
            return Err(mongodb::error::Error::custom(
                "automod_warnings contains records without a verified TTL index",
            ));
        }
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "expiresAt": 1 })
                    .options(
                        IndexOptions::builder()
                            .name("automod_warnings_expires_at_ttl".to_string())
                            .expire_after(Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    Ok(())
}

fn warning_id(guild_id: &str, user_id: &str) -> String {
    format!("{guild_id}:{user_id}")
}

fn new_automod_warning(guild_id: &str, user_id: &str, now: DateTime) -> Document {
    doc! {
        "_id": warning_id(guild_id, user_id),
        "guild_id": guild_id,
        "user_id": user_id,
        "warnedAt": now,
        "expiresAt": DateTime::from_millis(now.timestamp_millis() + AUTOMOD_WARNING_WINDOW.as_millis() as i64),
    }
}

pub async fn has_active_automod_warning(
    guild_id: &str,
    user_id: &str,
) -> mongodb::error::Result<bool> {
    Ok(get_collection(AUTOMOD_WARNINGS)?
        .find_one(doc! {
            "_id": warning_id(guild_id, user_id),
            "expiresAt": { "$gt": DateTime::now() }
        })
        .await?
        .is_some())
}

pub async fn record_automod_warning(guild_id: &str, user_id: &str) -> mongodb::error::Result<()> {
    let warning = new_automod_warning(guild_id, user_id, DateTime::now());
    get_collection(AUTOMOD_WARNINGS)?
        .replace_one(doc! { "_id": warning_id(guild_id, user_id) }, warning)
        .upsert(true)
        .await?;
    Ok(())
}

fn new_ai_turn(
    guild_id: &str,
    user_id: Option<&str>,
    request_user_id: &str,
    payload: Document,
    created_at: DateTime,
) -> Document {
    let mut turn = doc! {
        "guild_id": guild_id,
        "request_user_id": request_user_id,
        "payload": payload,
        "createdAt": created_at,
        "expiresAt": retention::expires_at(created_at),
    };
    if let Some(id) = user_id {
        turn.insert("user_id", id);
    }
    turn
}

pub async fn insert_ai_turn(
    guild_id: &str,
    user_id: Option<&str>,
    request_user_id: &str,
    payload: Document,
) -> mongodb::error::Result<()> {
    get_collection(AI_TURNS)?
        .insert_one(new_ai_turn(
            guild_id,
            user_id,
            request_user_id,
            payload,
            DateTime::now(),
        ))
        .await?;
    Ok(())
}

pub async fn recent_ai_turns(guild_id: &str, limit: i64) -> mongodb::error::Result<Vec<Document>> {
    let docs: Vec<Document> = get_collection(AI_TURNS)?
        .find(doc! { "guild_id": guild_id, "expiresAt": { "$gt": DateTime::now() } })
        .sort(doc! { "createdAt": -1, "_id": -1 })
        .limit(limit)
        .await?
        .try_collect()
        .await?;
    Ok(docs
        .into_iter()
        .rev()
        .filter_map(|doc| doc.get_document("payload").ok().cloned())
        .collect())
}

/// Read the old embedded history only while a reviewed migration is pending.
/// Turns without a reliable timestamp are excluded from AI context, not silently assigned a new lifetime.
pub async fn recent_legacy_turns(
    guild_id: &str,
    limit: i64,
) -> mongodb::error::Result<Vec<Document>> {
    let Some(doc) = get_collection(LEGACY_MESSAGES)?
        .find_one(guild_filter(guild_id))
        .projection(doc! { "messages": { "$slice": -limit } })
        .await?
    else {
        return Ok(Vec::new());
    };
    let cutoff = retention::cutoff(DateTime::now()).timestamp_millis();
    let turns = doc
        .get_array("messages")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Bson::as_document)
        .filter(|turn| legacy_timestamp_millis(turn).is_some_and(|time| time >= cutoff))
        .cloned()
        .collect();
    Ok(turns)
}

pub fn legacy_timestamp_millis(turn: &Document) -> Option<i64> {
    if let Ok(value) = turn.get_i64("timestamp") {
        return value.checked_mul(1_000);
    }
    if let Ok(value) = turn.get_i32("timestamp") {
        return Some(i64::from(value) * 1_000);
    }
    if let Ok(value) = turn.get_datetime("timestamp") {
        return Some(value.timestamp_millis());
    }
    if let Ok(value) = turn.get_str("timestamp") {
        return chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|time| time.timestamp_millis());
    }
    None
}

pub async fn delete_guild_ai_history(guild_id: &str) -> mongodb::error::Result<()> {
    get_collection(AI_TURNS)?
        .delete_many(guild_filter(guild_id))
        .await?;
    get_collection(LEGACY_MESSAGES)?
        .delete_many(guild_filter(guild_id))
        .await?;
    Ok(())
}

/// Called after a verified guild removal or an internally authorized server deletion request.
pub async fn delete_guild_data(guild_id: &str) -> mongodb::error::Result<()> {
    crate::byok::storage::delete_guild(guild_id).await?;
    get_collection(crate::voice::VOICE_USAGE)?
        .delete_many(guild_filter(guild_id))
        .await?;
    get_collection(crate::moderation_events::EVENTS)?
        .delete_many(guild_filter(guild_id))
        .await?;
    get_collection(crate::moderation_events::DELIVERIES)?
        .delete_many(guild_filter(guild_id))
        .await?;
    delete_guild_ai_history(guild_id).await?;
    get_collection(GUILD_SETTINGS)?
        .delete_many(guild_filter(guild_id))
        .await?;
    get_collection(LOCKED_CHANNELS)?
        .delete_many(guild_filter(guild_id))
        .await?;
    get_collection(AUTOMOD_WARNINGS)?
        .delete_many(guild_filter(guild_id))
        .await?;
    Ok(())
}

/// Removes this user's AI prompts and the AI/tool turns generated for those prompts.
/// Old embedded turns are filtered by author ID, and assistant replies tied to their message ID are removed.
/// Tool turns and text that mentions a user without a structured link require manual review.
pub async fn delete_user_data(user_id: &str) -> mongodb::error::Result<()> {
    get_collection(AI_TURNS)?
        .delete_many(user_turn_filter(user_id))
        .await?;
    let legacy = get_collection(LEGACY_MESSAGES)?;
    let mut cursor = legacy
        .find(doc! { "messages.author.id": user_id })
        .projection(doc! { "messages.author.id": 1, "messages.message.id": 1 })
        .await?;
    while let Some(guild_doc) = cursor.try_next().await? {
        let ids: Vec<Bson> = guild_doc
            .get_array("messages")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Bson::as_document)
            .filter(|turn| {
                turn.get_document("author")
                    .ok()
                    .and_then(|author| author.get_str("id").ok())
                    == Some(user_id)
            })
            .filter_map(|turn| {
                turn.get_document("message")
                    .ok()
                    .and_then(|message| message.get_str("id").ok())
            })
            .map(|id| Bson::String(id.to_string()))
            .collect();
        let Some(source_id) = guild_doc.get("_id") else {
            continue;
        };
        legacy
            .update_one(
                doc! { "_id": source_id.clone() },
                doc! { "$pull": { "messages": { "$or": [
                    { "author.id": user_id }, { "message.replying_to.id": { "$in": ids } }
                ] } } },
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_timestamp_formats() {
        assert_eq!(
            legacy_timestamp_millis(&doc! { "timestamp": 1_700_000_000_i64 }),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            legacy_timestamp_millis(&doc! { "timestamp": "2023-11-14T22:13:20Z" }),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            legacy_timestamp_millis(&doc! { "timestamp": "unknown" }),
            None
        );
    }

    #[test]
    fn new_turn_uses_bson_dates_and_scoped_ids() {
        let created = DateTime::from_millis(1_700_000_000_000);
        let turn = new_ai_turn(
            "guild-a",
            Some("user-a"),
            "user-a",
            doc! { "role": "USER" },
            created,
        );
        assert_eq!(turn.get_str("guild_id").unwrap(), "guild-a");
        assert_eq!(turn.get_str("user_id").unwrap(), "user-a");
        assert_eq!(turn.get_datetime("createdAt").unwrap(), &created);
        assert_eq!(
            turn.get_datetime("expiresAt").unwrap(),
            &retention::expires_at(created)
        );
    }

    #[test]
    fn deletion_filters_are_scoped_to_the_requested_identity() {
        assert_eq!(guild_filter("guild-a"), doc! { "guild_id": "guild-a" });
        assert_eq!(
            user_turn_filter("user-a"),
            doc! { "$or": [
                { "user_id": "user-a" }, { "request_user_id": "user-a" }
            ] }
        );
    }

    #[test]
    fn automod_warning_contains_only_scoped_ids_and_one_day_expiry() {
        let now = DateTime::from_millis(1_700_000_000_000);
        let warning = new_automod_warning("guild-a", "user-a", now);
        assert_eq!(warning.get_str("_id").unwrap(), "guild-a:user-a");
        assert_eq!(warning.get_str("guild_id").unwrap(), "guild-a");
        assert_eq!(warning.get_str("user_id").unwrap(), "user-a");
        assert_eq!(warning.get_datetime("warnedAt").unwrap(), &now);
        assert_eq!(
            warning
                .get_datetime("expiresAt")
                .unwrap()
                .timestamp_millis(),
            now.timestamp_millis() + 86_400_000
        );
        assert_eq!(warning.len(), 5);
    }
}

pub async fn mongo_connect() -> mongodb::error::Result<()> {
    let mongodb_uri = crate::config::MONGO_URI.get().unwrap();

    let client_options = ClientOptions::parse(mongodb_uri)
        .await
        .map_err(|e| mongodb::error::Error::custom(format!("Invalid MongoDB URI: {}", e)))?;
    let client = Client::with_options(client_options)?;
    client
        .database("admin")
        .run_command(doc! { "ping": 1 })
        .await?;

    CLIENT
        .set(client)
        .map_err(|_| mongodb::error::Error::custom("MongoDB client already initialized"))?;

    log::info!("Connected to MongoDB!");
    Ok(())
}

pub fn get_collection(
    collection_name: &str,
) -> Result<mongodb::Collection<mongodb::bson::Document>, mongodb::error::Error> {
    let client = CLIENT
        .get()
        .ok_or_else(|| mongodb::error::Error::custom("MongoDB client not initialized"))?;
    Ok(client
        .database("omnicore_bot")
        .collection::<mongodb::bson::Document>(collection_name))
}

pub async fn mongo_shutdown() {
    log::info!("Attempting MongoDB shutdown...");

    let client = match CLIENT.get() {
        Some(c) => c.clone(),
        None => {
            log::warn!("mongo_shutdown() called but MongoDB was never initialized.");
            return;
        }
    };

    client.shutdown().await;

    log::info!("MongoDB shutdown complete.");
}
