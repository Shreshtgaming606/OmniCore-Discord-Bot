//! Metadata-only AutoMod events and durable, bounded Discord notification delivery.
//! No Message or raw incident text crosses this boundary.
use crate::{automod::config::AutoModConfig, config, database};
use futures::TryStreamExt;
use mongodb::{
    IndexModel,
    bson::{DateTime, Document, doc},
    options::IndexOptions,
};
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, GuildId,
    PermissionOverwriteType, Permissions, UserId,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::mpsc;

pub const EVENTS: &str = "moderation_events";
pub const DELIVERIES: &str = "moderation_deliveries";
const RETENTION_DAYS: i64 = 90;
const MAX_ATTEMPTS: i32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Warning,
    Timeout,
    FailedTimeout,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Timeout => "timeout",
            Self::FailedTimeout => "failed_timeout",
        }
    }
}

pub struct ModerationEvent {
    pub incident_id: u64,
    pub guild_id: GuildId,
    pub guild_name: String,
    pub guild_owner: UserId,
    pub member_id: UserId,
    pub member_name: String,
    pub channel_id: ChannelId,
    pub action: Action,
    pub category: &'static str,
    pub severity: &'static str,
    pub duration: Option<Duration>,
    pub occurred_at: DateTime,
}

fn safe_label(input: &str) -> String {
    input
        .chars()
        .take(64)
        .map(|c| match c {
            '\n' | '\r' | '\t' | '`' | '<' | '>' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .replace('@', "@\u{200b}")
}

fn expires_at(now: DateTime) -> DateTime {
    DateTime::from_millis(now.timestamp_millis() + RETENTION_DAYS * 86_400_000)
}

fn event_document(event: &ModerationEvent) -> Document {
    let mut doc = doc! {
        "_id": event.incident_id.to_string(),
        "guild_id": event.guild_id.to_string(),
        "guild_name": safe_label(&event.guild_name),
        "member_id": event.member_id.to_string(),
        "member_name": safe_label(&event.member_name),
        "channel_id": event.channel_id.to_string(),
        "action": event.action.name(),
        "category": event.category,
        "severity": event.severity,
        "status": if event.action == Action::FailedTimeout { "failed" } else { "succeeded" },
        "occurredAt": event.occurred_at,
        "expiresAt": expires_at(event.occurred_at),
    };
    if let Some(duration) = event.duration {
        doc.insert(
            "duration_seconds",
            duration.as_secs().min(48 * 3_600) as i64,
        );
    }
    doc
}

/// The developer list is independent of legacy BOT_OWNERS. One delivery document
/// per (incident, recipient) prevents fan-out duplication when a developer owns a guild.
fn recipients(event: &ModerationEvent, settings: &AutoModConfig) -> BTreeMap<u64, (bool, bool)> {
    recipients_with_developers(event, settings, config::DEVELOPER_IDS.get())
}

fn recipients_with_developers(
    event: &ModerationEvent,
    settings: &AutoModConfig,
    developers: Option<&[u64; 2]>,
) -> BTreeMap<u64, (bool, bool)> {
    let mut recipients = BTreeMap::new();
    if settings.notify_owner {
        recipients.insert(event.guild_owner.get(), (true, false));
    }
    if settings.notify_developers
        && let Some(ids) = developers
    {
        for id in ids {
            let entry = recipients.entry(*id).or_insert((false, false));
            entry.1 = true;
        }
    }
    recipients
}

pub async fn ensure_indexes() -> mongodb::error::Result<()> {
    for name in [EVENTS, DELIVERIES] {
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
    database::get_collection(EVENTS)?
        .create_index(
            IndexModel::builder()
                .keys(doc! {"guild_id": 1, "occurredAt": -1})
                .options(
                    IndexOptions::builder()
                        .name("moderation_guild_time".to_string())
                        .build(),
                )
                .build(),
        )
        .await?;
    database::get_collection(DELIVERIES)?
        .create_index(
            IndexModel::builder()
                .keys(doc! {"status": 1, "nextAttempt": 1})
                .options(
                    IndexOptions::builder()
                        .name("moderation_due_deliveries".to_string())
                        .build(),
                )
                .build(),
        )
        .await?;
    Ok(())
}

pub struct NotificationService {
    wake: mpsc::Sender<()>,
}

impl NotificationService {
    pub fn start(token: &str) -> Arc<Self> {
        let (wake, rx) = mpsc::channel(256);
        let service = Arc::new(Self { wake });
        let http = Arc::new(serenity::Http::new(token));
        tokio::spawn(async move { worker(http, rx).await });
        service
    }

    pub async fn publish(&self, event: ModerationEvent, settings: &AutoModConfig) {
        if let Err(error) = self.persist(event, settings).await {
            log::error!("AutoMod metadata event could not be saved: {error}");
            return;
        }
        // The database remains the outbox if this bounded wake queue is full.
        let _ = self.wake.try_send(());
    }

    async fn persist(
        &self,
        event: ModerationEvent,
        settings: &AutoModConfig,
    ) -> mongodb::error::Result<()> {
        let events = database::get_collection(EVENTS)?;
        let document = event_document(&event);
        events
            .update_one(
                doc! {"_id": event.incident_id.to_string()},
                doc! {"$setOnInsert": document},
            )
            .upsert(true)
            .await?;
        let deliveries = database::get_collection(DELIVERIES)?;
        for (recipient, (is_owner, is_developer)) in recipients(&event, settings) {
            let delivery = doc! {
                "_id": format!("{}:{recipient}", event.incident_id),
                "incident_id": event.incident_id.to_string(),
                "guild_id": event.guild_id.to_string(),
                "recipient_id": recipient.to_string(),
                "is_owner": is_owner,
                "is_developer": is_developer,
                "fallback_channel_id": if is_owner { settings.log_channel_id.map(|id| id.to_string()) } else { None },
                "status": "pending",
                "attempts": 0,
                "nextAttempt": DateTime::now(),
                "expiresAt": expires_at(event.occurred_at),
            };
            deliveries
                .update_one(
                    doc! {"_id": format!("{}:{recipient}", event.incident_id)},
                    doc! {"$setOnInsert": delivery},
                )
                .upsert(true)
                .await?;
        }
        Ok(())
    }
}

fn owner_message(event: &Document) -> String {
    let duration = event
        .get_i64("duration_seconds")
        .ok()
        .map(|s| format!("\nDuration: {} minutes", s / 60))
        .unwrap_or_default();
    format!(
        "OMNICORE MODERATION ALERT\nServer: {} ({})\nMember: {} (ID: {})\nAction: {}{}\nReason: {}\nSeverity: {}\nChannel: <#{}>\nTime: <t:{}:F>\nStatus: {}\n\nThe server owner can configure AutoMod with /automod settings.",
        event.get_str("guild_name").unwrap_or("Unknown"),
        event.get_str("guild_id").unwrap_or("?"),
        event.get_str("member_name").unwrap_or("Unknown"),
        event.get_str("member_id").unwrap_or("?"),
        event.get_str("action").unwrap_or("unknown"),
        duration,
        event.get_str("category").unwrap_or("Unknown"),
        event.get_str("severity").unwrap_or("unknown"),
        event.get_str("channel_id").unwrap_or("?"),
        event
            .get_datetime("occurredAt")
            .map_or(0, |t| t.timestamp_millis() / 1_000),
        event.get_str("status").unwrap_or("unknown")
    )
}

fn developer_message(event: &Document) -> String {
    let duration = event
        .get_i64("duration_seconds")
        .ok()
        .map(|s| format!("\nDuration: {s} seconds"))
        .unwrap_or_default();
    format!(
        "OMNICORE OPERATOR MODERATION ALERT\nGuild: {} ({})\nAction: {}{}\nCategory: {}\nSeverity: {}\nTime: <t:{}:F>\nStatus: {}\nIncident: {}\nNo message content is included.",
        event.get_str("guild_name").unwrap_or("Unknown"),
        event.get_str("guild_id").unwrap_or("?"),
        event.get_str("action").unwrap_or("unknown"),
        duration,
        event.get_str("category").unwrap_or("Unknown"),
        event.get_str("severity").unwrap_or("unknown"),
        event
            .get_datetime("occurredAt")
            .map_or(0, |t| t.timestamp_millis() / 1_000),
        event.get_str("status").unwrap_or("unknown"),
        event.get_str("_id").unwrap_or("?")
    )
}

async fn worker(http: Arc<serenity::Http>, mut wake: mpsc::Receiver<()>) {
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! { _ = interval.tick() => {}, item = wake.recv() => { if item.is_none() { break; } } }
        if let Err(error) = deliver_due(&http).await {
            log::warn!("AutoMod notification delivery pass failed: {error}");
        }
    }
}

async fn deliver_due(http: &serenity::Http) -> mongodb::error::Result<()> {
    let deliveries = database::get_collection(DELIVERIES)?;
    let now = DateTime::now();
    let mut due = deliveries
        .find(doc! {
            "status": {"$in": ["pending", "sending"]},
            "nextAttempt": {"$lte": now},
            "expiresAt": {"$gt": now},
        })
        .limit(32)
        .await?;
    while let Some(candidate) = due.try_next().await? {
        let Some(id) = candidate.get_str("_id").ok() else {
            continue;
        };
        let claimed = deliveries.find_one_and_update(doc! {"_id": id, "status": {"$in": ["pending", "sending"]}, "nextAttempt": {"$lte": DateTime::now()}},
            doc! {"$set": {"status": "sending", "nextAttempt": DateTime::from_millis(DateTime::now().timestamp_millis()+30_000)}, "$inc": {"attempts": 1}}).await?;
        let Some(delivery) = claimed else {
            continue;
        };
        let Some(incident) = delivery.get_str("incident_id").ok() else {
            continue;
        };
        let Some(event) = database::get_collection(EVENTS)?
            .find_one(doc! {"_id": incident})
            .await?
        else {
            continue;
        };
        let recipient = delivery
            .get_str("recipient_id")
            .ok()
            .and_then(|id| id.parse::<u64>().ok());
        let Some(recipient) = recipient else {
            continue;
        };
        let is_owner = delivery.get_bool("is_owner").unwrap_or(false);
        let content = if is_owner {
            owner_message(&event)
        } else {
            developer_message(&event)
        };
        let message = CreateMessage::new()
            .content(content.clone())
            .allowed_mentions(
                CreateAllowedMentions::new()
                    .empty_users()
                    .empty_roles()
                    .everyone(false),
            );
        let sent = tokio::time::timeout(
            Duration::from_secs(12),
            UserId::new(recipient).dm(http, message),
        )
        .await
        .is_ok_and(|result| result.is_ok());
        let fallback = if !sent && is_owner {
            owner_fallback(http, &delivery, &content).await
        } else {
            false
        };
        if sent || fallback {
            deliveries
                .update_one(
                    doc! {"_id": id},
                    doc! {"$set": {"status": "sent", "sentAt": DateTime::now()}},
                )
                .await?;
        } else {
            let attempts = delivery.get_i32("attempts").unwrap_or(0) + 1;
            let final_failure = attempts >= MAX_ATTEMPTS;
            let wait_ms = (1_i64 << attempts.min(5)) * 5_000;
            deliveries.update_one(doc! {"_id": id}, doc! {"$set": {
                "status": if final_failure { "failed" } else { "pending" },
                "nextAttempt": DateTime::from_millis(DateTime::now().timestamp_millis()+wait_ms),
            }}).await?;
            log::warn!(
                "AutoMod notification delivery failed incident={} recipient={} final={final_failure}",
                incident,
                recipient
            );
        }
    }
    Ok(())
}

async fn owner_fallback(http: &serenity::Http, delivery: &Document, content: &str) -> bool {
    let Some(channel_id) = delivery
        .get_str("fallback_channel_id")
        .ok()
        .and_then(|id| id.parse::<u64>().ok())
    else {
        return false;
    };
    let Some(guild_id) = delivery
        .get_str("guild_id")
        .ok()
        .and_then(|id| id.parse::<u64>().ok())
    else {
        return false;
    };
    let id = ChannelId::new(channel_id);
    let Ok(channel) = id.to_channel(http).await else {
        return false;
    };
    let Some(channel) = channel.guild() else {
        return false;
    };
    if channel.guild_id != GuildId::new(guild_id) {
        return false;
    }
    let private = channel.permission_overwrites.iter().any(|overwrite| {
        overwrite.kind == PermissionOverwriteType::Role(GuildId::new(guild_id).everyone_role())
            && overwrite.deny.contains(Permissions::VIEW_CHANNEL)
    });
    if !private {
        return false;
    }
    tokio::time::timeout(
        Duration::from_secs(12),
        id.send_message(
            http,
            CreateMessage::new().content(content).allowed_mentions(
                CreateAllowedMentions::new()
                    .empty_users()
                    .empty_roles()
                    .everyone(false),
            ),
        ),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_carries_no_message_contents_and_caps_duration() {
        let event = ModerationEvent {
            incident_id: 1,
            guild_id: GuildId::new(2),
            guild_name: "Test".into(),
            guild_owner: UserId::new(3),
            member_id: UserId::new(4),
            member_name: "Member".into(),
            channel_id: ChannelId::new(5),
            action: Action::Timeout,
            category: "Harassment",
            severity: "severe",
            duration: Some(Duration::from_secs(10 * 86_400)),
            occurred_at: DateTime::now(),
        };
        let document = event_document(&event);
        assert_eq!(document.get_i64("duration_seconds").unwrap(), 172_800);
        assert!(!document.contains_key("content"));
        assert!(!developer_message(&document).contains("Member"));
    }

    #[test]
    fn warning_and_timeout_fan_out_to_three_distinct_recipients() {
        let mut event = ModerationEvent {
            incident_id: 10,
            guild_id: GuildId::new(20),
            guild_name: "Server".into(),
            guild_owner: UserId::new(30),
            member_id: UserId::new(40),
            member_name: "Member".into(),
            channel_id: ChannelId::new(50),
            action: Action::Warning,
            category: "Harassment",
            severity: "high",
            duration: None,
            occurred_at: DateTime::now(),
        };
        let settings = AutoModConfig::default();
        let ids = [1, 2];
        assert_eq!(
            recipients_with_developers(&event, &settings, Some(&ids)).len(),
            3
        );
        event.action = Action::Timeout;
        event.duration = Some(Duration::from_secs(60));
        assert_eq!(
            recipients_with_developers(&event, &settings, Some(&ids)).len(),
            3
        );
        event.guild_owner = UserId::new(1);
        let deduped = recipients_with_developers(&event, &settings, Some(&ids));
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped.get(&1), Some(&(true, true)));
    }
}
