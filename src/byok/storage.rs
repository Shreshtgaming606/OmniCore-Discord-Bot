use super::{
    crypto::MasterKey,
    provider::{self, AiProvider, ProviderError, ProviderResponse},
};
use crate::database;
use mongodb::{
    IndexModel,
    bson::{Bson, DateTime, Document, doc},
    options::IndexOptions,
};
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateMessage, GuildId};
use rand::RngCore;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;

pub const CREDENTIALS: &str = "guild_ai_credentials";
pub const USAGE: &str = "ai_usage";
pub const QUOTAS: &str = "ai_daily_quotas";
const RETENTION_DAYS: i64 = 90;

#[derive(Clone, Debug)]
pub struct Limits {
    pub user_daily: i32,
    pub guild_daily: i32,
    pub output_tokens: i32,
    pub concurrency: u32,
    pub channels: Vec<u64>,
    pub roles: Vec<u64>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            user_daily: 10,
            guild_daily: 100,
            output_tokens: 512,
            concurrency: 2,
            channels: vec![],
            roles: vec![],
        }
    }
}
impl Limits {
    pub fn from_document(doc: Option<&Document>) -> Self {
        let Some(doc) = doc else {
            return Self::default();
        };
        let mut result = Self::default();
        result.user_daily = doc
            .get_i32("user_daily")
            .unwrap_or(result.user_daily)
            .clamp(1, 1_000);
        result.guild_daily = doc
            .get_i32("guild_daily")
            .unwrap_or(result.guild_daily)
            .clamp(1, 100_000);
        result.output_tokens = doc
            .get_i32("output_tokens")
            .unwrap_or(result.output_tokens)
            .clamp(32, 4_096);
        result.concurrency = doc
            .get_i32("concurrency")
            .unwrap_or(result.concurrency as i32)
            .clamp(1, 16) as u32;
        result.channels = read_ids(doc, "channels");
        result.roles = read_ids(doc, "roles");
        result
    }
    pub fn to_document(&self) -> Document {
        doc! { "user_daily": self.user_daily.clamp(1, 1_000),
            "guild_daily": self.guild_daily.clamp(1, 100_000),
            "output_tokens": self.output_tokens.clamp(32, 4_096),
            "concurrency": (self.concurrency as i32).clamp(1, 16),
            "channels": self.channels.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "roles": self.roles.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        }
    }
}
fn read_ids(doc: &Document, name: &str) -> Vec<u64> {
    doc.get_array(name)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Bson::as_str)
        .filter_map(|id| id.parse::<u64>().ok())
        .take(50)
        .collect()
}

fn crossed_daily_notice(before: i32, limit: i32) -> bool {
    let threshold = ((limit as i64 * 8 + 9) / 10) as i32;
    before < threshold && before + 1 >= threshold
}

pub struct Connection {
    pub provider: AiProvider,
    pub model: String,
    pub validated_at: DateTime,
}
struct Credential {
    connection: Connection,
    secret: Zeroizing<String>,
}

#[derive(Debug)]
pub enum AiError {
    NotConnected,
    Forbidden,
    Quota,
    Busy,
    Provider(ProviderError),
    Storage,
    InvalidModel,
}

pub struct ByokService {
    master: MasterKey,
    http: reqwest::Client,
    discord: Arc<serenity::Http>,
    active: Arc<Mutex<HashMap<u64, u32>>>,
}

struct ActiveGuard {
    guild: u64,
    active: Arc<Mutex<HashMap<u64, u32>>>,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.active.lock() {
            let count = map.entry(self.guild).or_default();
            *count = count.saturating_sub(1);
        }
    }
}

impl ByokService {
    pub fn new(master_hex: &str, bot_token: &str) -> Result<Arc<Self>, &'static str> {
        Ok(Arc::new(Self {
            master: MasterKey::from_hex(master_hex)?,
            http: provider::client(),
            discord: Arc::new(serenity::Http::new(bot_token)),
            active: Arc::new(Mutex::new(HashMap::new())),
        }))
    }
    fn acquire(&self, guild: u64, limit: u32) -> Result<ActiveGuard, AiError> {
        let mut map = self.active.lock().map_err(|_| AiError::Busy)?;
        let active = map.entry(guild).or_default();
        if *active >= limit {
            return Err(AiError::Busy);
        }
        *active += 1;
        Ok(ActiveGuard {
            guild,
            active: self.active.clone(),
        })
    }
    pub async fn list_models(
        &self,
        provider: AiProvider,
        key: &str,
    ) -> Result<Vec<String>, AiError> {
        provider::models(&self.http, provider, key)
            .await
            .map_err(AiError::Provider)
    }
    pub async fn connect(
        &self,
        guild: u64,
        provider: AiProvider,
        key: &str,
        model: &str,
    ) -> Result<(), AiError> {
        if key.len() < 10 || key.len() > 1_024 {
            return Err(AiError::Provider(ProviderError::InvalidKey));
        }
        let models = self.list_models(provider, key).await?;
        if !models.iter().any(|available| available == model) {
            return Err(AiError::InvalidModel);
        }
        let sealed = self
            .master
            .seal(guild, provider.name(), key)
            .map_err(|_| AiError::Storage)?;
        let mut generation = [0_u8; 16];
        rand::rng().fill_bytes(&mut generation);
        database::get_collection(CREDENTIALS).map_err(|_| AiError::Storage)?
            .replace_one(doc! {"_id": guild.to_string()}, doc! {
                "_id": guild.to_string(), "guild_id": guild.to_string(), "provider": provider.name(),
                "model": model, "sealed": sealed, "generation": hex::encode(generation),
                "validatedAt": DateTime::now(),
            }).upsert(true).await.map_err(|_| AiError::Storage)?;
        Ok(())
    }
    pub async fn connection(&self, guild: u64) -> Result<Option<Connection>, AiError> {
        let Some(doc) = database::get_collection(CREDENTIALS)
            .map_err(|_| AiError::Storage)?
            .find_one(doc! {"_id": guild.to_string()})
            .await
            .map_err(|_| AiError::Storage)?
        else {
            return Ok(None);
        };
        Ok(Some(parse_connection(&doc)?))
    }
    async fn credential(&self, guild: u64) -> Result<Option<Credential>, AiError> {
        let Some(doc) = database::get_collection(CREDENTIALS)
            .map_err(|_| AiError::Storage)?
            .find_one(doc! {"_id": guild.to_string()})
            .await
            .map_err(|_| AiError::Storage)?
        else {
            return Ok(None);
        };
        let connection = parse_connection(&doc)?;
        let sealed = doc.get_document("sealed").map_err(|_| AiError::Storage)?;
        let secret = self
            .master
            .open(guild, connection.provider.name(), sealed)
            .map_err(|_| AiError::Storage)?;
        Ok(Some(Credential { connection, secret }))
    }
    pub async fn change_model(&self, guild: u64, model: &str) -> Result<(), AiError> {
        let credential = self.credential(guild).await?.ok_or(AiError::NotConnected)?;
        let available = self
            .list_models(credential.connection.provider, &credential.secret)
            .await?;
        if !available.iter().any(|id| id == model) {
            return Err(AiError::InvalidModel);
        }
        database::get_collection(CREDENTIALS)
            .map_err(|_| AiError::Storage)?
            .update_one(
                doc! {"_id": guild.to_string()},
                doc! {"$set": {"model": model, "validatedAt": DateTime::now()}},
            )
            .await
            .map_err(|_| AiError::Storage)?;
        Ok(())
    }
    pub async fn disconnect(&self, guild: u64) -> Result<(), AiError> {
        database::get_collection(CREDENTIALS)
            .map_err(|_| AiError::Storage)?
            .delete_one(doc! {"_id": guild.to_string()})
            .await
            .map_err(|_| AiError::Storage)?;
        Ok(())
    }
    pub async fn limits(&self, guild: u64) -> Result<Limits, AiError> {
        let doc = database::get_collection(database::GUILD_SETTINGS)
            .map_err(|_| AiError::Storage)?
            .find_one(doc! {"guild_id": guild.to_string()})
            .await
            .map_err(|_| AiError::Storage)?;
        Ok(Limits::from_document(
            doc.as_ref().and_then(|d| d.get_document("ai_byok").ok()),
        ))
    }
    pub async fn save_limits(&self, guild: u64, limits: &Limits) -> Result<(), AiError> {
        database::get_collection(database::GUILD_SETTINGS)
            .map_err(|_| AiError::Storage)?
            .update_one(
                doc! {"guild_id": guild.to_string()},
                doc! {"$set": {"ai_byok": limits.to_document()}, "$setOnInsert": {"guild_id": guild.to_string(), "prefix": "!", "ai_approved": false}},
            )
            .upsert(true)
            .await
            .map_err(|_| AiError::Storage)?;
        Ok(())
    }
    pub async fn ask(
        &self,
        guild: u64,
        user: u64,
        channel: u64,
        roles: &[u64],
        prompt: &str,
    ) -> Result<ProviderResponse, AiError> {
        if prompt.is_empty() || prompt.chars().count() > 2_000 {
            return Err(AiError::Forbidden);
        }
        let limits = self.limits(guild).await?;
        if (!limits.channels.is_empty() && !limits.channels.contains(&channel))
            || (!limits.roles.is_empty() && !roles.iter().any(|id| limits.roles.contains(id)))
        {
            return Err(AiError::Forbidden);
        }
        let _active = self.acquire(guild, limits.concurrency)?;
        // No credential is cached; disconnect prevents subsequent new requests.
        let credential = self.credential(guild).await?.ok_or(AiError::NotConnected)?;
        let quota_near_limit = self.reserve_quota(guild, user, &limits).await?;
        if quota_near_limit {
            let discord = self.discord.clone();
            let limit = limits.guild_daily;
            tokio::spawn(async move {
                let guild_id = GuildId::new(guild);
                let Ok(info) = guild_id.to_partial_guild(&discord).await else {
                    return;
                };
                let notice = format!(
                    "OmniCore AI usage alert: this server has reached 80% of its daily {limit}-request limit. Requests reset at 00:00 UTC. Check /ai usage and /ai limits. Provider charges can differ from request counts."
                );
                let sent = tokio::time::timeout(
                    Duration::from_secs(12),
                    info.owner_id.dm(
                        &discord,
                        CreateMessage::new().content(notice).allowed_mentions(
                            CreateAllowedMentions::new()
                                .empty_users()
                                .empty_roles()
                                .everyone(false),
                        ),
                    ),
                )
                .await;
                if !sent.is_ok_and(|result| result.is_ok()) {
                    log::warn!("BYOK quota notice could not be delivered for guild {guild}");
                }
            });
        }
        let response = provider::complete(
            &self.http,
            credential.connection.provider,
            &credential.secret,
            &credential.connection.model,
            prompt,
            limits.output_tokens as u32,
        )
        .await;
        let status = match &response {
            Ok(_) => "succeeded",
            Err(ProviderError::RateLimited) => "rate_limited",
            Err(ProviderError::InvalidKey) => "invalid_key",
            Err(_) => "failed",
        };
        let (input, output) = response
            .as_ref()
            .map(|r| (r.input_tokens, r.output_tokens))
            .unwrap_or((None, None));
        if self
            .record_usage(
                guild,
                credential.connection.provider,
                &credential.connection.model,
                status,
                input,
                output,
            )
            .await
            .is_err()
        {
            log::warn!("BYOK usage metadata could not be saved for guild {guild}");
        }
        response.map_err(AiError::Provider)
    }
    async fn reserve_quota(&self, guild: u64, user: u64, limits: &Limits) -> Result<bool, AiError> {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let id = format!("{guild}:{today}");
        let quotas = database::get_collection(QUOTAS).map_err(|_| AiError::Storage)?;
        quotas
            .update_one(
                doc! {"_id": &id},
                doc! {"$setOnInsert": {
                    "_id": &id, "guild_id": guild.to_string(), "day": &today, "total": 0,
                    "users": doc! {}, "expiresAt": retention_date(DateTime::now())
                }},
            )
            .upsert(true)
            .await
            .map_err(|_| AiError::Storage)?;
        let path = format!("users.{user}");
        let mut below = doc! {};
        below.insert(&path, doc! {"$lt": limits.user_daily});
        let mut absent = doc! {};
        absent.insert(&path, doc! {"$exists": false});
        let mut inc = doc! {"total": 1};
        inc.insert(path, 1);
        let claimed = quotas
            .find_one_and_update(
                doc! {"_id": &id, "total": {"$lt": limits.guild_daily}, "$or": [below, absent]},
                doc! {"$inc": inc},
            )
            .await
            .map_err(|_| AiError::Storage)?;
        if let Some(previous) = claimed {
            let before = previous.get_i32("total").unwrap_or(0);
            Ok(crossed_daily_notice(before, limits.guild_daily))
        } else {
            Err(AiError::Quota)
        }
    }
    async fn record_usage(
        &self,
        guild: u64,
        provider: AiProvider,
        model: &str,
        status: &str,
        input: Option<i64>,
        output: Option<i64>,
    ) -> Result<(), AiError> {
        let now = DateTime::now();
        let mut doc = doc! { "guild_id": guild.to_string(), "provider": provider.name(), "model": model,
        "status": status, "occurredAt": now, "expiresAt": retention_date(now) };
        if let Some(input) = input {
            doc.insert("input_tokens", input.max(0));
        }
        if let Some(output) = output {
            doc.insert("output_tokens", output.max(0));
        }
        database::get_collection(USAGE)
            .map_err(|_| AiError::Storage)?
            .insert_one(doc)
            .await
            .map_err(|_| AiError::Storage)?;
        Ok(())
    }
    pub async fn usage(&self, guild: u64) -> Result<UsageSummary, AiError> {
        let since = DateTime::from_millis(DateTime::now().timestamp_millis() - 30 * 86_400_000);
        let pipeline = vec![
            doc! {"$match": {"guild_id": guild.to_string(), "occurredAt": {"$gte": since}}},
            doc! {"$group": {"_id": "$status", "requests": {"$sum": 1},
            "input": {"$sum": {"$ifNull": ["$input_tokens", 0]}},
            "output": {"$sum": {"$ifNull": ["$output_tokens", 0]}},
            "missing": {"$sum": {"$cond": [{"$and": [{"$eq": ["$status", "succeeded"]}, {"$eq": [{"$type": "$input_tokens"}, "missing"]}]}, 1, 0]}}}},
        ];
        let mut cursor = database::get_collection(USAGE)
            .map_err(|_| AiError::Storage)?
            .aggregate(pipeline)
            .await
            .map_err(|_| AiError::Storage)?;
        use futures::TryStreamExt;
        let mut summary = UsageSummary::default();
        while let Some(row) = cursor.try_next().await.map_err(|_| AiError::Storage)? {
            let count = row
                .get_i32("requests")
                .map(i64::from)
                .or_else(|_| row.get_i64("requests"))
                .unwrap_or(0);
            summary.requests += count;
            if row.get_str("_id").ok() == Some("succeeded") {
                summary.successful += count;
            } else {
                summary.failures += count;
            }
            summary.input_tokens += row.get_i64("input").unwrap_or(0);
            summary.output_tokens += row.get_i64("output").unwrap_or(0);
            summary.missing_tokens += row
                .get_i32("missing")
                .map(i64::from)
                .or_else(|_| row.get_i64("missing"))
                .unwrap_or(0);
        }
        Ok(summary)
    }
}

#[derive(Default)]
pub struct UsageSummary {
    pub requests: i64,
    pub successful: i64,
    pub failures: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub missing_tokens: i64,
}

fn parse_connection(doc: &Document) -> Result<Connection, AiError> {
    let provider = AiProvider::parse(doc.get_str("provider").map_err(|_| AiError::Storage)?)
        .ok_or(AiError::Storage)?;
    Ok(Connection {
        provider,
        model: doc
            .get_str("model")
            .map_err(|_| AiError::Storage)?
            .to_string(),
        validated_at: *doc
            .get_datetime("validatedAt")
            .map_err(|_| AiError::Storage)?,
    })
}

fn retention_date(now: DateTime) -> DateTime {
    DateTime::from_millis(now.timestamp_millis() + RETENTION_DAYS * 86_400_000)
}

pub async fn ensure_indexes() -> mongodb::error::Result<()> {
    for name in [USAGE, QUOTAS] {
        database::get_collection(name)?
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"expiresAt": 1})
                    .options(
                        IndexOptions::builder()
                            .name(format!("{name}_ttl"))
                            .expire_after(Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    database::get_collection(USAGE)?
        .create_index(
            IndexModel::builder()
                .keys(doc! {"guild_id": 1, "occurredAt": -1})
                .options(
                    IndexOptions::builder()
                        .name("ai_usage_guild_time".to_string())
                        .build(),
                )
                .build(),
        )
        .await?;
    Ok(())
}

pub async fn delete_guild(guild: &str) -> mongodb::error::Result<()> {
    database::get_collection(CREDENTIALS)?
        .delete_many(doc! {"guild_id": guild})
        .await?;
    database::get_collection(USAGE)?
        .delete_many(doc! {"guild_id": guild})
        .await?;
    database::get_collection(QUOTAS)?
        .delete_many(doc! {"guild_id": guild})
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_fail_safe_and_bound_extreme_values() {
        let limits = Limits::from_document(Some(
            &doc! {"user_daily": -5, "guild_daily": 9999999, "output_tokens": 999999, "concurrency": 0},
        ));
        assert_eq!(limits.user_daily, 1);
        assert_eq!(limits.guild_daily, 100_000);
        assert_eq!(limits.output_tokens, 4_096);
        assert_eq!(limits.concurrency, 1);
    }

    #[test]
    fn owner_notice_triggers_once_at_eighty_percent() {
        assert!(!crossed_daily_notice(6, 10));
        assert!(crossed_daily_notice(8 - 1, 10));
        assert!(!crossed_daily_notice(8, 10));
        assert!(crossed_daily_notice(0, 1));
    }
}
