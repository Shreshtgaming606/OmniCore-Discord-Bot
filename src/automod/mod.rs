pub mod classifier;
pub mod config;
pub mod policy;
mod rules;

use crate::database;
use crate::moderation_events::{Action as EventAction, ModerationEvent, NotificationService};
use classifier::Assessment;
use config::AutoModConfig;
use mongodb::bson::doc;
use poise::serenity_prelude as serenity;
use policy::Action;
use serenity::{
    Context, CreateAllowedMentions, CreateMessage, EditMember, GuildId, Member, Message,
    Permissions, UserId,
};
use std::{
    cmp::Reverse,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

const INCIDENT_TTL: Duration = Duration::from_secs(60);
const DEDUP_TTL: Duration = Duration::from_secs(300);

struct Incident {
    content: String,
    expires: Instant,
}
#[derive(Default)]
struct Runtime {
    incidents: HashMap<u64, Incident>,
    seen: HashMap<u64, Instant>,
    repeats: HashMap<(u64, u64), (u64, Instant, u8)>,
    guild_requests: HashMap<u64, (Instant, u8)>,
    user_requests: HashMap<(u64, u64), (Instant, u8)>,
}

impl Runtime {
    fn cleanup(&mut self, now: Instant) {
        self.incidents.retain(|_, item| item.expires > now);
        self.seen
            .retain(|_, time| now.duration_since(*time) < DEDUP_TTL);
        self.repeats
            .retain(|_, (_, time, _)| now.duration_since(*time) < Duration::from_secs(60));
        self.guild_requests
            .retain(|_, (time, _)| now.duration_since(*time) < Duration::from_secs(60));
        self.user_requests
            .retain(|_, (time, _)| now.duration_since(*time) < Duration::from_secs(60));
    }
    fn repeat_count(&mut self, guild: u64, user: u64, fingerprint: u64, now: Instant) -> u8 {
        if self.repeats.len() >= 50_000 {
            self.repeats.clear();
        }
        let entry = self
            .repeats
            .entry((guild, user))
            .or_insert((fingerprint, now, 0));
        if entry.0 == fingerprint && now.duration_since(entry.1) < Duration::from_secs(45) {
            entry.2 = entry.2.saturating_add(1);
        } else {
            *entry = (fingerprint, now, 1);
        }
        entry.2
    }
    fn permit_request(&mut self, guild: u64, user: u64, now: Instant) -> bool {
        if self.guild_requests.len() >= 10_000 {
            self.guild_requests.clear();
        }
        if self.user_requests.len() >= 50_000 {
            self.user_requests.clear();
        }
        fn window<K: std::hash::Hash + Eq + Copy>(
            map: &mut HashMap<K, (Instant, u8)>,
            key: K,
            limit: u8,
            now: Instant,
        ) -> bool {
            let entry = map.entry(key).or_insert((now, 0));
            if now.duration_since(entry.0) >= Duration::from_secs(60) {
                *entry = (now, 0);
            }
            if entry.1 >= limit {
                return false;
            }
            true
        }
        if !window(&mut self.user_requests, (guild, user), 4, now)
            || !window(&mut self.guild_requests, guild, 30, now)
        {
            return false;
        }
        if let Some(entry) = self.user_requests.get_mut(&(guild, user)) {
            entry.1 += 1;
        }
        if let Some(entry) = self.guild_requests.get_mut(&guild) {
            entry.1 += 1;
        }
        true
    }
    fn claim_message(&mut self, id: u64, now: Instant) -> bool {
        if self.seen.contains_key(&id) {
            return false;
        }
        if self.seen.len() >= 100_000 {
            self.seen.clear();
        }
        self.seen.insert(id, now);
        true
    }
}

pub struct AutoModService {
    notifications: Arc<NotificationService>,
    client: reqwest::Client,
    semaphore: Arc<Semaphore>,
    runtime: Mutex<Runtime>,
    config_cache: Mutex<HashMap<u64, (AutoModConfig, Instant)>>,
    user_locks: Mutex<HashMap<(u64, u64), Arc<AsyncMutex<()>>>>,
}

impl AutoModService {
    pub fn new(notifications: Arc<NotificationService>) -> Arc<Self> {
        let service = Arc::new(Self {
            notifications,
            client: reqwest::Client::new(),
            semaphore: Arc::new(Semaphore::new(12)),
            runtime: Mutex::new(Runtime::default()),
            config_cache: Mutex::new(HashMap::new()),
            user_locks: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&service);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            loop {
                interval.tick().await;
                let Some(service) = weak.upgrade() else {
                    break;
                };
                if let Ok(mut runtime) = service.runtime.lock() {
                    runtime.cleanup(Instant::now());
                }
                if let Ok(mut locks) = service.user_locks.lock() {
                    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
                }
            }
        });
        service
    }

    pub async fn load_config(&self, guild_id: GuildId) -> mongodb::error::Result<AutoModConfig> {
        let started = Instant::now();
        if let Ok(cache) = self.config_cache.lock()
            && let Some((config, when)) = cache.get(&guild_id.get())
            && started.saturating_duration_since(*when) < Duration::from_secs(10)
        {
            return Ok(config.clone());
        }
        let doc = database::get_collection(database::GUILD_SETTINGS)?
            .find_one(doc! {"guild_id": guild_id.to_string()})
            .await?;
        let config = AutoModConfig::from_guild_document(doc.as_ref());
        if let Ok(mut cache) = self.config_cache.lock() {
            if let Some((newer, when)) = cache.get(&guild_id.get())
                && *when > started
            {
                return Ok(newer.clone());
            }
            if cache.len() >= 10_000 {
                cache.clear();
            }
            cache.insert(guild_id.get(), (config.clone(), Instant::now()));
        }
        Ok(config)
    }

    pub async fn save_config(
        &self,
        guild_id: GuildId,
        config: &AutoModConfig,
    ) -> mongodb::error::Result<()> {
        database::get_collection(database::GUILD_SETTINGS)?
            .update_one(doc! {"guild_id": guild_id.to_string()},
                doc! {"$set": {"automod": config.to_document()}, "$setOnInsert": {"prefix": "!", "ai_approved": false}})
            .upsert(true).await?;
        if let Ok(mut cache) = self.config_cache.lock() {
            if cache.len() >= 10_000 {
                cache.clear();
            }
            cache.insert(guild_id.get(), (config.clone(), Instant::now()));
        }
        Ok(())
    }

    fn user_lock(&self, guild_id: GuildId, user_id: UserId) -> Option<Arc<AsyncMutex<()>>> {
        let mut locks = self.user_locks.lock().ok()?;
        Some(
            locks
                .entry((guild_id.get(), user_id.get()))
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone(),
        )
    }

    /// Returns true when the message entered AutoMod and must not enter AI chat memory.
    pub async fn submit(self: &Arc<Self>, ctx: &Context, message: &Message) -> bool {
        let Some(guild_id) = message.guild_id else {
            return false;
        };
        if message.author.bot || message.content.is_empty() {
            return false;
        }
        let now = Instant::now();
        let (repeat, is_candidate) = {
            let Ok(mut runtime) = self.runtime.lock() else {
                return true;
            };
            if runtime.seen.contains_key(&message.id.get()) {
                return true;
            }
            let repeat = runtime.repeat_count(
                guild_id.get(),
                message.author.id.get(),
                rules::fingerprint(&message.content),
                now,
            );
            let is_candidate = rules::candidate(&message.content, message.mentions.len(), repeat);
            (repeat, is_candidate)
        };
        if !is_candidate {
            return false;
        }
        let config =
            match tokio::time::timeout(Duration::from_secs(2), self.load_config(guild_id)).await {
                Ok(Ok(config)) => config,
                Err(_) => {
                    log::warn!("AutoMod config lookup timed out for guild {guild_id}");
                    return true;
                }
                Ok(Err(error)) => {
                    log::warn!("AutoMod config unavailable for guild {guild_id}: {error}");
                    return true;
                }
            };
        if !config.enabled {
            return false;
        }
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return true,
        };
        {
            let Ok(mut runtime) = self.runtime.lock() else {
                return true;
            };
            if !runtime.claim_message(message.id.get(), now) {
                return true;
            }
            if !runtime.permit_request(guild_id.get(), message.author.id.get(), now) {
                return true;
            }
            if runtime.incidents.len() >= 12 {
                return true;
            }
            runtime.incidents.insert(
                message.id.get(),
                Incident {
                    content: message.content.clone(),
                    expires: now + INCIDENT_TTL,
                },
            );
        }
        let service = self.clone();
        let context = ctx.clone();
        let message = message.clone();
        tokio::spawn(async move {
            let id = message.id.get();
            let _permit = permit;
            if tokio::time::timeout(
                Duration::from_secs(50),
                service.process(&context, &message, repeat),
            )
            .await
            .is_err()
            {
                log::warn!("AutoMod incident {id} timed out safely");
            }
            if let Ok(mut runtime) = service.runtime.lock() {
                runtime.incidents.remove(&id);
            }
        });
        true
    }

    async fn process(&self, ctx: &Context, message: &Message, repeat: u8) {
        let Some(guild_id) = message.guild_id else {
            return;
        };
        let content = self.runtime.lock().ok().and_then(|runtime| {
            runtime
                .incidents
                .get(&message.id.get())
                .map(|incident| incident.content.clone())
        });
        let Some(content) = content else {
            return;
        };
        let assessment = match classifier::classify(
            &self.client,
            &content,
            repeat,
            message.mentions.len(),
        )
        .await
        {
            Ok(value) => value,
            Err(reason) => {
                log::warn!(
                    "AutoMod classifier unavailable for guild {guild_id}: {reason}; no action"
                );
                return;
            }
        };
        // Honor owner changes made while the classifier was running.
        let config =
            match tokio::time::timeout(Duration::from_secs(2), self.load_config(guild_id)).await {
                Ok(Ok(config)) => config,
                _ => return,
            };
        let mut classified = policy::decide(&config, &assessment);
        if matches!(classified, Action::Timeout(_)) && !rules::timeout_eligible(&content) {
            classified = if config.warnings_enabled {
                Action::Warn
            } else {
                Action::None
            };
        }
        drop(content);
        if classified == Action::None {
            return;
        }
        let guild = match guild_id.to_partial_guild(&ctx.http).await {
            Ok(guild) => guild,
            Err(error) => {
                log::warn!("AutoMod guild lookup failed: {error}");
                return;
            }
        };
        if message.author.bot {
            return;
        }
        let target = match guild_id.member(&ctx.http, message.author.id).await {
            Ok(member) => member,
            Err(error) => {
                log::warn!("AutoMod member lookup failed: {error}");
                return;
            }
        };
        let staff = guild.member_permissions(&target).intersects(
            Permissions::ADMINISTRATOR
                | Permissions::MODERATE_MEMBERS
                | Permissions::MANAGE_MESSAGES
                | Permissions::KICK_MEMBERS
                | Permissions::BAN_MEMBERS
                | Permissions::MANAGE_GUILD
                | Permissions::MANAGE_ROLES
                | Permissions::MANAGE_CHANNELS,
        );
        if message.author.id == guild.owner_id || staff {
            if !config.warnings_enabled {
                log::info!(
                    "AutoMod protected member flagged but warnings disabled guild={guild_id} user={}",
                    message.author.id
                );
                return;
            }
            let public = format!(
                "⚠️ <@{}> was warned for {}.",
                message.author.id,
                assessment.reason()
            );
            let publicly_delivered = send_public_notice(ctx, message, &public).await;
            let privately_delivered = if message.author.id != guild.owner_id || !config.notify_owner
            {
                let private = format!(
                    "⚠️ OmniCore Auto Moderation\nYour message was flagged for {}. Please avoid sending messages like this in the server.",
                    assessment.reason()
                );
                message
                    .author
                    .id
                    .dm(&ctx.http, private_message(&private))
                    .await
                    .is_ok()
            } else {
                false
            };
            let label = if publicly_delivered {
                "Protected member warned; public reply sent; no timeout"
            } else if privately_delivered {
                "Protected member warned by DM; public reply failed; no timeout"
            } else {
                "Protected member flagged; public reply failed; no timeout"
            };
            log::info!(
                "AutoMod protected member guild={guild_id} user={} category={} action={label}",
                message.author.id,
                assessment.reason()
            );
            if publicly_delivered || privately_delivered {
                self.record_event(
                    message,
                    &guild,
                    &assessment,
                    EventAction::Warning,
                    None,
                    &config,
                )
                .await;
            }
            return;
        }
        let Some(user_lock) = self.user_lock(guild_id, message.author.id) else {
            log::warn!("AutoMod user lock unavailable for guild {guild_id}; no action");
            return;
        };
        let _user_guard = user_lock.lock().await;
        let warned = match tokio::time::timeout(
            Duration::from_secs(3),
            database::has_active_automod_warning(
                &guild_id.to_string(),
                &message.author.id.to_string(),
            ),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                log::warn!(
                    "AutoMod warning lookup failed for guild {guild_id}: {error}; using warning-only fallback"
                );
                false
            }
            Err(_) => {
                log::warn!(
                    "AutoMod warning lookup timed out for guild {guild_id}; using warning-only fallback"
                );
                false
            }
        };
        let action = policy::with_warning_history(&config, classified, warned);
        let (label, duration, event_action) = match action {
            Action::None => return,
            Action::Warn => {
                let text = format!(
                    "⚠️ OmniCore Auto Moderation\nYour message was flagged for {}. Another confirmed violation within 24 hours may result in a temporary timeout.",
                    assessment.reason()
                );
                let public = format!(
                    "⚠️ <@{}> was warned for {}. Another confirmed violation within 24 hours may result in a timeout.",
                    message.author.id,
                    assessment.reason()
                );
                let publicly_delivered = send_public_notice(ctx, message, &public).await;
                let privately_delivered = message
                    .author
                    .id
                    .dm(&ctx.http, private_message(&text))
                    .await
                    .is_ok();
                let label = if publicly_delivered || privately_delivered {
                    match database::record_automod_warning(
                        &guild_id.to_string(),
                        &message.author.id.to_string(),
                    )
                    .await
                    {
                        Ok(()) if publicly_delivered => "Warning; public reply sent",
                        Ok(()) => "Warning; public reply failed, DM sent",
                        Err(error) => {
                            log::warn!(
                                "AutoMod warning could not be saved for guild {guild_id}: {error}"
                            );
                            "Warning delivered; escalation state unavailable"
                        }
                    }
                } else {
                    "Warning could not be delivered"
                };
                (
                    label,
                    None,
                    (publicly_delivered || privately_delivered).then_some(EventAction::Warning),
                )
            }
            Action::Timeout(requested) => {
                let duration = config::capped(requested);
                let target = match guild_id.member(&ctx.http, message.author.id).await {
                    Ok(member) => member,
                    Err(error) => {
                        log::warn!("AutoMod member recheck failed before timeout: {error}");
                        return;
                    }
                };
                if target
                    .communication_disabled_until
                    .is_some_and(|until| until.unix_timestamp() > chrono::Utc::now().timestamp())
                {
                    log::info!(
                        "AutoMod skipped a duplicate timeout for guild {guild_id}, user {}",
                        message.author.id
                    );
                    return;
                }
                if guild.member_permissions(&target).intersects(
                    Permissions::ADMINISTRATOR
                        | Permissions::MODERATE_MEMBERS
                        | Permissions::MANAGE_MESSAGES
                        | Permissions::KICK_MEMBERS
                        | Permissions::BAN_MEMBERS
                        | Permissions::MANAGE_GUILD
                        | Permissions::MANAGE_ROLES
                        | Permissions::MANAGE_CHANNELS,
                ) {
                    log::info!(
                        "AutoMod skipped timeout after staff role change in guild {guild_id}"
                    );
                    return;
                }
                let bot_id = ctx.cache.current_user().id;
                let bot_member = guild_id.member(&ctx.http, bot_id).await;
                let can_moderate = bot_member.as_ref().is_ok_and(|bot| {
                    guild
                        .member_permissions(bot)
                        .contains(Permissions::MODERATE_MEMBERS)
                });
                let above_target = bot_member.as_ref().is_ok_and(|bot| {
                    highest_role(bot, &guild.roles) > highest_role(&target, &guild.roles)
                });
                if !policy::may_timeout(false, false, false, can_moderate, above_target) {
                    let public = format!(
                        "⚠️ <@{}> was flagged for {} after a prior warning. A timeout could not be applied; server moderators have been notified.",
                        message.author.id,
                        assessment.reason()
                    );
                    let _ = send_public_notice(ctx, message, &public).await;
                    (
                        "Timeout unavailable: missing permission or role hierarchy",
                        Some(duration),
                        Some(EventAction::FailedTimeout),
                    )
                } else {
                    let until =
                        chrono::Utc::now() + chrono::Duration::seconds(duration.as_secs() as i64);
                    let reason = format!("OmniCore AutoMod: {}", assessment.reason());
                    match ctx
                        .http
                        .edit_member(
                            guild_id,
                            message.author.id,
                            &EditMember::new().disable_communication_until(until.to_rfc3339()),
                            Some(&reason),
                        )
                        .await
                    {
                        Ok(_) => {
                            let public = format!(
                                "⏱️ <@{}> was timed out for {} minutes for {} after a prior warning.",
                                message.author.id,
                                duration.as_secs() / 60,
                                assessment.reason()
                            );
                            let publicly_delivered =
                                send_public_notice(ctx, message, &public).await;
                            let text = format!(
                                "⏱️ OmniCore Auto Moderation\nA temporary timeout was applied for {}. Duration: {} minutes. If incorrect, contact the server's moderation team.",
                                assessment.reason(),
                                duration.as_secs() / 60
                            );
                            let _ = message
                                .author
                                .id
                                .dm(&ctx.http, private_message(&text))
                                .await;
                            (
                                if publicly_delivered {
                                    "Timeout; public reply sent"
                                } else {
                                    "Timeout; public reply failed"
                                },
                                Some(duration),
                                Some(EventAction::Timeout),
                            )
                        }
                        Err(error) => {
                            log::warn!(
                                "AutoMod timeout failed for guild {guild_id}, user {}: {error}",
                                message.author.id
                            );
                            let public = format!(
                                "⚠️ <@{}> was flagged for {} after a prior warning. A timeout could not be applied; server moderators have been notified.",
                                message.author.id,
                                assessment.reason()
                            );
                            let _ = send_public_notice(ctx, message, &public).await;
                            (
                                "Timeout could not be applied",
                                Some(duration),
                                Some(EventAction::FailedTimeout),
                            )
                        }
                    }
                }
            }
        };
        log::info!(
            "AutoMod action guild={guild_id} user={} category={} action={label} duration={}",
            message.author.id,
            assessment.reason(),
            duration.map_or(0, |d| d.as_secs())
        );
        if let Some(event_action) = event_action {
            self.record_event(
                message,
                &guild,
                &assessment,
                event_action,
                duration,
                &config,
            )
            .await;
        }
    }

    async fn record_event(
        &self,
        message: &Message,
        guild: &serenity::PartialGuild,
        assessment: &Assessment,
        action: EventAction,
        duration: Option<Duration>,
        config: &AutoModConfig,
    ) {
        self.notifications
            .publish(
                ModerationEvent {
                    incident_id: message.id.get(),
                    guild_id: guild.id,
                    guild_name: guild.name.clone(),
                    guild_owner: guild.owner_id,
                    member_id: message.author.id,
                    member_name: message.author.name.clone(),
                    channel_id: message.channel_id,
                    action,
                    category: assessment.reason(),
                    severity: assessment.severity.name(),
                    duration,
                    occurred_at: mongodb::bson::DateTime::from_millis(
                        message.timestamp.unix_timestamp() * 1_000,
                    ),
                },
                config,
            )
            .await;
    }
}

async fn send_public_notice(ctx: &Context, message: &Message, text: &str) -> bool {
    let allowed_mentions = CreateAllowedMentions::new()
        .users([message.author.id])
        .empty_roles()
        .everyone(false)
        .replied_user(false);
    let reply = CreateMessage::new()
        .content(text)
        .reference_message(message)
        .allowed_mentions(allowed_mentions.clone());
    match message.channel_id.send_message(&ctx.http, reply).await {
        Ok(_) => true,
        Err(error) => {
            log::warn!(
                "AutoMod public reply failed for message {}: {error}",
                message.id
            );
            message
                .channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(text)
                        .allowed_mentions(allowed_mentions),
                )
                .await
                .is_ok()
        }
    }
}

fn private_message(text: &str) -> CreateMessage {
    CreateMessage::new().content(text).allowed_mentions(
        CreateAllowedMentions::new()
            .empty_users()
            .empty_roles()
            .everyone(false),
    )
}

pub(crate) fn highest_role(
    member: &Member,
    roles: &HashMap<serenity::RoleId, serenity::Role>,
) -> Option<(u16, Reverse<u64>)> {
    member
        .roles
        .iter()
        .filter_map(|id| roles.get(id))
        .map(|role| (role.position, Reverse(role.id.get())))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dedup_and_incident_ttl() {
        let now = Instant::now();
        let mut state = Runtime::default();
        assert!(state.claim_message(1, now));
        assert!(!state.claim_message(1, now));
        state.incidents.insert(
            1,
            Incident {
                content: "private".into(),
                expires: now + INCIDENT_TTL,
            },
        );
        state.cleanup(now + INCIDENT_TTL + Duration::from_millis(1));
        assert!(!state.incidents.contains_key(&1));
        assert!(state.seen.contains_key(&1));
        for _ in 0..4 {
            assert!(state.permit_request(1, 2, now));
        }
        assert!(!state.permit_request(1, 2, now));
        assert!(state.permit_request(1, 3, now));
    }
    #[test]
    fn equal_position_role_tie_uses_lower_id() {
        assert!((5, Reverse(10_u64)) > (5, Reverse(20_u64)));
    }
}
