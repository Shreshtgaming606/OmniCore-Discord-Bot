//! Short-lived, opt-in Discord voice sessions. No audio or transcripts are persisted here.

use crate::database::{self, GUILD_SETTINGS};
use mongodb::{
    IndexModel,
    bson::{DateTime, Document, doc},
    options::IndexOptions,
};
use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, MessageId, UserId};
use serde::{Deserialize, Serialize};
use songbird::Songbird;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};
use tokio::sync::Mutex;

pub mod audio;
pub mod pipeline;
pub const VOICE_USAGE: &str = "voice_usage";

pub fn retention_date() -> DateTime {
    DateTime::from_millis(DateTime::now().timestamp_millis() + 90 * 86_400_000)
}

pub async fn ensure_indexes() -> mongodb::error::Result<()> {
    database::get_collection(VOICE_USAGE)?
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expiresAt": 1})
                .options(
                    IndexOptions::builder()
                        .name("voice_usage_ttl".to_owned())
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await?;
    Ok(())
}

pub struct AudioBuffer {
    pub samples: Vec<i16>,
    pub voiced_ticks: u16,
    pub silent_ticks: u16,
}

impl AudioBuffer {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(16_000 * 2),
            voiced_ticks: 0,
            silent_ticks: 0,
        }
    }
}

pub struct Conversation {
    pub turns: Vec<(String, String)>,
    pub followup_until: Instant,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct VoiceSettings {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub speaking_voice: String,
    pub wake_phrase: String,
    pub followup_seconds: u32,
    pub idle_minutes: u32,
    pub max_spoken_words: u32,
    pub daily_requests: u32,
    pub allowed_voice_channel: Option<u64>,
    pub allowed_role: Option<u64>,
    pub language: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "ollama".to_owned(),
            model: String::new(),
            speaking_voice: "en_US-lessac-medium".to_owned(),
            wake_phrase: "Hey OmniCore".to_owned(),
            followup_seconds: 20,
            idle_minutes: 10,
            max_spoken_words: 80,
            daily_requests: 30,
            allowed_voice_channel: None,
            allowed_role: None,
            language: "en".to_owned(),
        }
    }
}

impl VoiceSettings {
    pub async fn load(guild: GuildId) -> mongodb::error::Result<Self> {
        let stored = database::get_collection(GUILD_SETTINGS)?
            .find_one(doc! { "guild_id": guild.to_string() })
            .await?;
        Ok(stored
            .as_ref()
            .and_then(|document| document.get_document("voice").ok())
            .and_then(|document| mongodb::bson::deserialize_from_document(document.clone()).ok())
            .unwrap_or_default())
    }

    pub async fn save(&self, guild: GuildId) -> mongodb::error::Result<()> {
        let voice: Document =
            mongodb::bson::serialize_to_document(self).map_err(mongodb::error::Error::custom)?;
        database::get_collection(GUILD_SETTINGS)?
            .update_one(
                doc! { "guild_id": guild.to_string() },
                doc! { "$set": { "voice": voice }, "$setOnInsert": { "guild_id": guild.to_string() } },
            )
            .upsert(true)
            .await?;
        Ok(())
    }
}

pub struct VoiceService {
    pub manager: Arc<Songbird>,
    pub active: Mutex<Option<VoiceSession>>,
    pub byok: Option<Arc<crate::byok::storage::ByokService>>,
    pub http: Arc<serenity::Http>,
}

impl VoiceService {
    pub fn new(
        manager: Arc<Songbird>,
        byok: Option<Arc<crate::byok::storage::ByokService>>,
        token: &str,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            manager,
            active: Mutex::new(None),
            byok,
            http: Arc::new(serenity::Http::new(token)),
        });
        Self::start_housekeeping(service.clone());
        service
    }

    fn start_housekeeping(service: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let snapshot = {
                    let mut guard = service.active.lock().await;
                    let Some(session) = guard.as_mut() else {
                        continue;
                    };
                    let now = Instant::now();
                    session
                        .conversations
                        .retain(|_, context| context.followup_until > now);
                    session
                        .queued
                        .retain(|(_, _, queued_at)| queued_at.elapsed().as_secs() < 20);
                    let expired = session.last_activity.elapsed()
                        >= std::time::Duration::from_secs(
                            u64::from(session.settings.idle_minutes) * 60,
                        );
                    let alone = session
                        .empty_since
                        .is_some_and(|since| since.elapsed() >= std::time::Duration::from_secs(60));
                    (session.guild, session.started, expired, alone)
                };
                let (guild, started, expired, alone) = snapshot;
                let disabled = VoiceSettings::load(guild)
                    .await
                    .map(|settings| !settings.enabled)
                    .unwrap_or(false);
                if !(expired || alone || disabled) {
                    continue;
                }
                let old = {
                    let mut guard = service.active.lock().await;
                    if guard
                        .as_ref()
                        .is_some_and(|s| s.guild == guild && s.started == started)
                    {
                        let mut old = guard.take().unwrap();
                        old.stop();
                        Some(old)
                    } else {
                        None
                    }
                };
                if let Some(old) = old {
                    let _ = service.manager.remove(guild).await;
                    let reason = if disabled {
                        "server owner disabled voice"
                    } else if alone {
                        "channel empty"
                    } else {
                        "idle timeout"
                    };
                    let _ = old.text_channel.say(&service.http, format!("OmniCore Voice disconnected ({reason}). Consent and temporary context were cleared.")).await;
                }
            }
        });
    }

    pub async fn voice_state(&self, new: &serenity::VoiceState, bot: UserId) {
        let Some(guild) = new.guild_id else { return };
        let mut guard = self.active.lock().await;
        let Some(session) = guard.as_mut().filter(|s| s.guild == guild) else {
            return;
        };
        if new.user_id == bot && new.channel_id != Some(session.channel) {
            session.stop();
            *guard = None;
            return;
        }
        if new.user_id == bot {
            return;
        }
        if new.channel_id == Some(session.channel)
            && !new.member.as_ref().is_some_and(|m| m.user.bot)
        {
            session.present.insert(new.user_id);
            session.empty_since = None;
        } else {
            session.present.remove(&new.user_id);
            session.revoke(new.user_id);
            if session.present.is_empty() && session.empty_since.is_none() {
                session.empty_since = Some(Instant::now());
            }
        }
    }
}

pub struct VoiceSession {
    pub guild: GuildId,
    pub channel: ChannelId,
    pub text_channel: ChannelId,
    pub status_message: Option<MessageId>,
    pub started: Instant,
    pub last_activity: Instant,
    pub consenting: HashSet<UserId>,
    pub settings: VoiceSettings,
    pub stt: Option<Arc<whisper_rs::WhisperContext>>,
    pub speaker_map: HashMap<u32, UserId>,
    pub decoders: HashMap<UserId, opus2::Decoder>,
    pub buffers: HashMap<UserId, AudioBuffer>,
    pub conversations: HashMap<UserId, Conversation>,
    pub busy: bool,
    pub active_user: Option<UserId>,
    pub queued: VecDeque<(UserId, Vec<i16>, Instant)>,
    pub generation: u64,
    pub track: Option<songbird::tracks::TrackHandle>,
    pub present: HashSet<UserId>,
    pub empty_since: Option<Instant>,
    pub stopped: bool,
}

impl VoiceSession {
    pub fn new(guild: GuildId, channel: ChannelId, text_channel: ChannelId) -> Self {
        Self {
            guild,
            channel,
            text_channel,
            status_message: None,
            started: Instant::now(),
            last_activity: Instant::now(),
            consenting: HashSet::new(),
            settings: VoiceSettings::default(),
            stt: None,
            speaker_map: HashMap::new(),
            decoders: HashMap::new(),
            buffers: HashMap::new(),
            conversations: HashMap::new(),
            busy: false,
            active_user: None,
            queued: VecDeque::new(),
            generation: 0,
            track: None,
            present: HashSet::new(),
            empty_since: None,
            stopped: false,
        }
    }

    pub fn revoke(&mut self, user: UserId) {
        self.consenting.remove(&user);
        self.buffers.remove(&user);
        self.conversations.remove(&user);
        self.speaker_map.retain(|_, speaker| *speaker != user);
        self.decoders.remove(&user);
        self.queued.retain(|(queued, _, _)| *queued != user);
        if self.active_user == Some(user) {
            self.interrupt();
        }
    }

    pub fn interrupt(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.busy = false;
        self.active_user = None;
        self.queued.clear();
        if let Some(track) = self.track.take() {
            let _ = track.stop();
        }
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.interrupt();
        self.consenting.clear();
        self.buffers.clear();
        self.conversations.clear();
        self.speaker_map.clear();
        self.decoders.clear();
        self.stt = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_defaults_are_private_and_limited() {
        let settings = VoiceSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.followup_seconds, 20);
        assert_eq!(settings.idle_minutes, 10);
        let mut session = VoiceSession::new(GuildId::new(1), ChannelId::new(2), ChannelId::new(3));
        session.consenting.insert(UserId::new(4));
        session.active_user = Some(UserId::new(4));
        session.buffers.insert(UserId::new(4), AudioBuffer::new());
        session.conversations.insert(
            UserId::new(4),
            Conversation {
                turns: vec![("question".into(), "answer".into())],
                followup_until: Instant::now(),
            },
        );
        session.revoke(UserId::new(4));
        assert!(session.consenting.is_empty());
        assert!(session.buffers.is_empty());
        assert!(session.conversations.is_empty());
        let generation = session.generation;
        session.revoke(UserId::new(5));
        assert_eq!(session.generation, generation);
    }
}
