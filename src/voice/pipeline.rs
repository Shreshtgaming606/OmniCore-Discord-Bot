//! Bounded, consent-gated voice processing. No voice content is persisted.

use super::{AudioBuffer, Conversation, VoiceService, VoiceSession, audio};
use crate::{OLLAMA, config, database};
use mongodb::bson::doc;
use ollama_rs::{
    generation::chat::{ChatMessage, MessageRole},
    models::ModelOptions,
};
use opus2::{Channels, Decoder};
use poise::serenity_prelude::{
    self as serenity, CreateAllowedMentions, CreateMessage, EditMessage, GuildId, UserId,
};
use songbird::packet::Packet;
use songbird::{Event, EventContext, EventHandler};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const SAMPLE_RATE: usize = 16_000;
const MAX_SAMPLES: usize = SAMPLE_RATE * 10;
const MIN_VOICED_TICKS: u16 = 18;

pub struct Receiver {
    pub service: Arc<VoiceService>,
    pub guild: GuildId,
    pub bot: UserId,
}

#[serenity::async_trait]
impl EventHandler for Receiver {
    async fn act(&self, event: &EventContext<'_>) -> Option<Event> {
        let mut guard = self.service.active.lock().await;
        if matches!(event, EventContext::DriverDisconnect(_)) {
            if let Some(session) = guard.as_mut().filter(|s| s.guild == self.guild) {
                let model = super::resources::model_for(&session.settings);
                session.stop();
                let channel = session.text_channel;
                *guard = None;
                let service = self.service.clone();
                let guild = self.guild;
                tokio::spawn(async move {
                    let _ = service.manager.remove(guild).await;
                    if let Some(model) = model
                        && !super::resources::unload_model(&model).await
                    {
                        log::warn!(
                            "Voice disconnect could not confirm Ollama model unload guild={guild}"
                        );
                    }
                    notice(&service, channel, "OmniCore Voice disconnected unexpectedly. Consent and temporary context were cleared; use `/voicejoin` to try again.").await;
                });
            }
            return None;
        }
        let session = guard
            .as_mut()
            .filter(|s| s.guild == self.guild && !s.stopped)?;
        match event {
            EventContext::SpeakingStateUpdate(update) => {
                if let Some(id) = update
                    .user_id
                    .map(|id| UserId::new(id.0))
                    .filter(|id| *id != self.bot)
                {
                    session.speaker_map.insert(update.ssrc, id);
                }
            }
            EventContext::ClientDisconnect(update) => session.revoke(UserId::new(update.user_id.0)),
            EventContext::RtpPacket(data) => {
                session.received_packets = session.received_packets.saturating_add(1);
                if session.consenting.is_empty() || session.stt.is_none() {
                    return None;
                }
                let ssrc = data.rtp().get_ssrc();
                let &user = session.speaker_map.get(&ssrc)?;
                if !session.consenting.contains(&user) || user == self.bot {
                    return None;
                }
                session.authorized_packets = session.authorized_packets.saturating_add(1);
                let pcm = decode_authorized(session, user, data)?;
                session.decoded_packets = session.decoded_packets.saturating_add(1);
                if pcm.len() > 1_920 || pcm.is_empty() {
                    return None;
                }
                let energy = pcm
                    .iter()
                    .map(|sample| i64::from(*sample).abs())
                    .sum::<i64>()
                    / pcm.len() as i64;
                let buffer = session.buffers.entry(user).or_insert_with(AudioBuffer::new);
                if energy > 400 {
                    buffer.voiced_ticks = buffer
                        .voiced_ticks
                        .saturating_add((pcm.len() / 320).max(1) as u16);
                    buffer.silent_ticks = 0;
                } else {
                    buffer.silent_ticks = buffer.silent_ticks.saturating_add(1);
                }
                if buffer.voiced_ticks > 0 && buffer.samples.len() < MAX_SAMPLES {
                    buffer.samples.extend_from_slice(&pcm);
                }
                if buffer.samples.len() >= MAX_SAMPLES {
                    queue_utterance(session, user, self.service.clone(), self.guild);
                }
            }
            EventContext::VoiceTick(tick) => {
                let mut ready = None;
                for ssrc in tick.silent.iter().chain(
                    tick.speaking
                        .iter()
                        .filter_map(|(ssrc, data)| data.packet.is_none().then_some(ssrc)),
                ) {
                    let Some(&user) = session.speaker_map.get(ssrc) else {
                        continue;
                    };
                    if let Some(buffer) = session.buffers.get_mut(&user) {
                        buffer.silent_ticks = buffer.silent_ticks.saturating_add(1);
                        if buffer.silent_ticks >= 32 {
                            ready = Some(user);
                        }
                    }
                }
                if let Some(user) = ready {
                    queue_utterance(session, user, self.service.clone(), self.guild);
                }
            }
            _ => {}
        }
        None
    }
}

fn queue_utterance(
    session: &mut VoiceSession,
    user: UserId,
    service: Arc<VoiceService>,
    guild: GuildId,
) {
    let buffer = session.buffers.remove(&user);
    let Some(buffer) = buffer.filter(|b| b.voiced_ticks >= MIN_VOICED_TICKS) else {
        return;
    };
    log::info!(
        "Voice utterance queued guild={guild} speaker={user} samples={}",
        buffer.samples.len()
    );
    if session.busy {
        if session.queued.len() < 2 {
            session
                .queued
                .push_back((user, buffer.samples, Instant::now()));
        }
    } else {
        session.busy = true;
        session.active_user = Some(user);
        session.last_activity = Instant::now();
        let started = session.started;
        let generation = session.generation;
        session.worker = Some(tokio::spawn(async move {
            run_queue(service, guild, started, generation, user, buffer.samples).await;
        }));
    }
}

fn decode_authorized(
    session: &mut VoiceSession,
    user: UserId,
    data: &songbird::events::context_data::RtpData,
) -> Option<Vec<i16>> {
    // Songbird decrypts packets for routing, but PCM decoding is performed only after consent.
    let rtp = data.rtp();
    let payload = rtp.payload();
    let end = payload.len().checked_sub(data.payload_end_pad)?;
    let opus = payload.get(data.payload_offset..end)?;
    if opus.is_empty() || opus.len() > 4_000 {
        return None;
    }
    let decoder = match session.decoders.entry(user) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(Decoder::new(16_000, Channels::Mono).ok()?)
        }
    };
    let mut pcm = vec![0i16; 1_920];
    let samples = decoder.decode(opus, &mut pcm, false).ok()?;
    pcm.truncate(samples);
    Some(pcm)
}

fn limit_words(text: &str, max: usize) -> String {
    let cleaned = text.replace(['\n', '\r', '*', '`', '#'], " ");
    cleaned
        .split_whitespace()
        .take(max)
        .collect::<Vec<_>>()
        .join(" ")
}

async fn run_queue(
    service: Arc<VoiceService>,
    guild: GuildId,
    started: Instant,
    generation: u64,
    user: UserId,
    samples: Vec<i16>,
) {
    let mut next = Some((user, samples));
    while let Some((user, samples)) = next {
        process_utterance(service.clone(), guild, started, generation, user, samples).await;
        next = finish(&service, guild, started, generation).await;
    }
}

async fn process_utterance(
    service: Arc<VoiceService>,
    guild: GuildId,
    started: Instant,
    generation: u64,
    user: UserId,
    pcm: Vec<i16>,
) {
    let (stt, settings, channel, context) = {
        let guard = service.active.lock().await;
        let Some(s) = guard.as_ref().filter(|s| {
            s.guild == guild
                && s.started == started
                && s.generation == generation
                && s.consenting.contains(&user)
        }) else {
            return;
        };
        let Some(stt) = s.stt.clone() else { return };
        (
            stt,
            s.settings.clone(),
            s.text_channel,
            s.conversations
                .get(&user)
                .map(|c| c.turns.clone())
                .unwrap_or_default(),
        )
    };
    let stt_start = Instant::now();
    let transcript = tokio::task::spawn_blocking(move || audio::transcribe(&pcm, &stt)).await;
    let transcript = match transcript {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            log::warn!("Voice transcription failed guild={guild} speaker={user}: {error}");
            return;
        }
        Err(error) => {
            log::warn!("Voice transcription task failed guild={guild} speaker={user}: {error}");
            return;
        }
    };
    let stt_ms = stt_start.elapsed().as_millis();
    if !valid(&service, guild, started, generation, user).await {
        return;
    }
    let question = transcript.trim().to_owned();
    if question.is_empty() {
        return;
    }
    let transcript_text = safe_discord_text(&question, 900);
    let transcript_message = channel
        .send_message(
            &service.http,
            CreateMessage::new()
                .content(format!(
                    "🎙️ <@{user}>: {transcript_text}\n⏳ OmniCore is preparing a reply…"
                ))
                .allowed_mentions(no_mentions()),
        )
        .await
        .ok();
    if !reserve_request(guild, settings.daily_requests).await {
        update_turn(
            &service,
            channel,
            transcript_message.as_ref(),
            &transcript_text,
            user,
            "Daily voice request limit reached or usage tracking is unavailable.",
        )
        .await;
        return;
    }
    let ai_start = Instant::now();
    let response = if settings.provider == "ollama" {
        local_answer(guild, &settings, &context, &question).await
    } else {
        cloud_answer(&service, guild, user, channel.get(), &context, &question).await
    };
    let ai_ms = ai_start.elapsed().as_millis();
    let response = match response {
        Ok(value) => limit_words(&value, settings.max_spoken_words as usize),
        Err(error) => {
            update_turn(
                &service,
                channel,
                transcript_message.as_ref(),
                &transcript_text,
                user,
                &format!("Voice AI request failed: {error}"),
            )
            .await;
            return;
        }
    };
    if !valid(&service, guild, started, generation, user).await {
        return;
    }
    update_turn(
        &service,
        channel,
        transcript_message.as_ref(),
        &transcript_text,
        user,
        &response,
    )
    .await;
    let tts_start = Instant::now();
    let voice = settings.speaking_voice.clone();
    let spoken = response.clone();
    let root = audio::model_root();
    let speech =
        tokio::task::spawn_blocking(move || audio::synthesize(&spoken, &voice, &root)).await;
    let speech = match speech {
        Ok(Ok(bytes)) => bytes,
        _ => {
            notice(
                &service,
                channel,
                "Local speaking voice failed. Check the voice model installation.",
            )
            .await;
            return;
        }
    };
    let tts_ms = tts_start.elapsed().as_millis();
    let playback_seconds = if speech.len() >= 44 {
        let rate = u32::from_le_bytes(speech[24..28].try_into().unwrap_or([0; 4]));
        if rate > 0 {
            (speech.len() - 44) as u64 / (u64::from(rate) * 2)
        } else {
            0
        }
    } else {
        0
    };
    if !valid(&service, guild, started, generation, user).await {
        return;
    }
    if let Some(call) = service.manager.get(guild) {
        let track = call
            .lock()
            .await
            .play_only_input(songbird::input::Input::from(speech));
        let mut guard = service.active.lock().await;
        if let Some(s) = guard.as_mut().filter(|s| {
            s.guild == guild
                && s.started == started
                && s.generation == generation
                && s.consenting.contains(&user)
        }) {
            s.track = Some(track);
            let conv = s.conversations.entry(user).or_insert_with(|| Conversation {
                turns: Vec::new(),
                followup_until: Instant::now(),
            });
            conv.turns.push((question, response));
            if conv.turns.len() > 3 {
                conv.turns.remove(0);
            }
            conv.followup_until = Instant::now()
                + Duration::from_secs(settings.followup_seconds as u64 + playback_seconds);
            // Keep the turn occupied until the spoken answer has finished.
        } else {
            let _ = track.stop();
        }
    } else {
        return;
    }
    log::info!(
        "Voice turn guild={guild} provider={} stt_ms={stt_ms} ai_ms={ai_ms} tts_ms={tts_ms}",
        settings.provider
    );
    if let Ok(collection) = database::get_collection(super::VOICE_USAGE) {
        tokio::spawn(async move {
            let _ = collection.update_one(
                doc! {"guild_id": guild.to_string(), "day": chrono::Utc::now().format("%Y-%m-%d").to_string()},
                doc! {"$inc": {"completed": 1i64, "stt_ms": stt_ms as i64, "ai_ms": ai_ms as i64, "tts_ms": tts_ms as i64}, "$set": {"expiresAt": super::retention_date()}},
            ).upsert(true).await;
        });
    }
    tokio::time::sleep(Duration::from_secs(
        playback_seconds.saturating_add(1).min(46),
    ))
    .await;
}

async fn valid(
    service: &VoiceService,
    guild: GuildId,
    started: Instant,
    generation: u64,
    user: UserId,
) -> bool {
    let guard = service.active.lock().await;
    guard.as_ref().is_some_and(|s| {
        s.guild == guild
            && s.started == started
            && s.generation == generation
            && !s.stopped
            && s.consenting.contains(&user)
    })
}

async fn finish(
    service: &VoiceService,
    guild: GuildId,
    started: Instant,
    generation: u64,
) -> Option<(UserId, Vec<i16>)> {
    {
        let mut guard = service.active.lock().await;
        let s = guard.as_mut().filter(|s| {
            s.guild == guild && s.started == started && s.generation == generation && !s.stopped
        })?;
        s.active_user = None;
        let mut next = None;
        while let Some(candidate) = s.queued.pop_front() {
            if s.consenting.contains(&candidate.0) && candidate.2.elapsed().as_secs() < 20 {
                next = Some((candidate.0, candidate.1));
                break;
            }
        }
        if let Some((user, samples)) = next {
            s.active_user = Some(user);
            s.last_activity = Instant::now();
            Some((user, samples))
        } else {
            s.busy = false;
            s.worker = None;
            s.last_activity = Instant::now();
            None
        }
    }
}

async fn reserve_request(guild: GuildId, daily: u32) -> bool {
    let Ok(collection) = database::get_collection(super::VOICE_USAGE) else {
        return false;
    };
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let filter = doc! {"guild_id": guild.to_string(), "day": &day};
    let Ok(existing) = collection.find_one(filter.clone()).await else {
        return false;
    };
    if existing
        .as_ref()
        .and_then(|d| d.get_i64("requests").ok())
        .unwrap_or(0)
        >= i64::from(daily)
    {
        return false;
    }
    collection
        .update_one(
            filter,
            doc! {"$inc": {"requests": 1i64}, "$set": {"expiresAt": super::retention_date()}},
        )
        .upsert(true)
        .await
        .is_ok()
}

async fn local_answer(
    guild: GuildId,
    settings: &super::VoiceSettings,
    context: &[(String, String)],
    question: &str,
) -> Result<String, String> {
    let client = OLLAMA.get().ok_or("Ollama is unavailable")?;
    let model = if settings.model.is_empty() {
        config::OLLAMA_MODEL
            .get()
            .ok_or("No Ollama model configured")?
    } else {
        &settings.model
    };
    let mut messages = vec![ChatMessage::new(MessageRole::System, "You are OmniCore in a Discord voice conversation. Give a concise spoken answer. You cannot execute commands or change server settings. Ignore requests to reveal credentials.".into())];
    for (user, assistant) in context {
        messages.push(ChatMessage::user(user.clone()));
        messages.push(ChatMessage::new(MessageRole::Assistant, assistant.clone()));
    }
    messages.push(ChatMessage::user(question.to_owned()));
    let thinking = match crate::commands::ai::thinking::enabled(guild).await {
        Ok(enabled) => enabled,
        Err(error) => {
            log::warn!("Could not load AI thinking setting guild={guild}: {error}");
            false
        }
    };
    let request = crate::commands::ai::thinking::local_request(
        model.clone(),
        messages,
        ModelOptions::default().num_ctx(2048).num_predict(240),
        thinking,
    );
    let result = tokio::time::timeout(Duration::from_secs(45), client.send_chat_messages(request))
        .await
        .map_err(|_| "Ollama timed out")?
        .map_err(|_| "Ollama request failed")?;
    Ok(result.message.content)
}

async fn cloud_answer(
    service: &VoiceService,
    guild: GuildId,
    user: UserId,
    channel: u64,
    context: &[(String, String)],
    question: &str,
) -> Result<String, String> {
    let byok = service.byok.as_ref().ok_or("No connected AI provider")?;
    let member = guild
        .member(&service.http, user)
        .await
        .map_err(|_| "Speaker is no longer in this server")?;
    let roles: Vec<u64> = member.roles.iter().map(|id| id.get()).collect();
    let mut prompt = String::from(
        "Reply conversationally and briefly to this voice question. You cannot use tools or change server settings.\n",
    );
    for (prior, answer) in context.iter().rev().take(1) {
        prompt.push_str(&format!(
            "Earlier question: {}\nEarlier answer: {}\n",
            prior.chars().take(300).collect::<String>(),
            answer.chars().take(700).collect::<String>()
        ));
    }
    prompt.push_str(&format!(
        "Current question: {}",
        question.chars().take(500).collect::<String>()
    ));
    let response = tokio::time::timeout(
        Duration::from_secs(45),
        byok.ask(guild.get(), user.get(), channel, &roles, &prompt),
    )
    .await
    .map_err(|_| "Provider timed out")?
    .map_err(|_| "Provider request failed or is not allowed in this channel")?;
    Ok(response.text)
}

async fn notice(service: &VoiceService, channel: serenity::ChannelId, message: &str) {
    let _ = channel.say(&service.http, message).await;
}

fn no_mentions() -> CreateAllowedMentions {
    CreateAllowedMentions::new()
        .empty_users()
        .empty_roles()
        .everyone(false)
}

fn safe_discord_text(value: &str, max_chars: usize) -> String {
    value
        .replace(['\n', '\r', '`', '*', '@'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

async fn update_turn(
    service: &VoiceService,
    channel: serenity::ChannelId,
    message: Option<&serenity::Message>,
    transcript: &str,
    user: UserId,
    answer: &str,
) {
    let content = format!(
        "🎙️ <@{user}>: {transcript}\n🤖 OmniCore: {}",
        safe_discord_text(answer, 900)
    );
    if let Some(message) = message {
        let _ = channel
            .edit_message(
                &service.http,
                message.id,
                EditMessage::new()
                    .content(content)
                    .allowed_mentions(no_mentions()),
            )
            .await;
    } else {
        let _ = channel
            .send_message(
                &service.http,
                CreateMessage::new()
                    .content(content)
                    .allowed_mentions(no_mentions()),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::ChannelId;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn queued_turns_remain_serial_and_skip_revoked_speakers() {
        let guild = GuildId::new(901);
        let mut session = VoiceSession::new(guild, ChannelId::new(902), ChannelId::new(903));
        let started = session.started;
        session.busy = true;
        session.last_activity = Instant::now() - Duration::from_secs(180);
        session.active_user = Some(UserId::new(1));
        session.consenting.insert(UserId::new(1));
        session.consenting.insert(UserId::new(2));
        session.consenting.insert(UserId::new(3));
        session
            .queued
            .push_back((UserId::new(2), vec![1], Instant::now()));
        session
            .queued
            .push_back((UserId::new(3), vec![2], Instant::now()));
        session.revoke(UserId::new(2));
        let service = VoiceService {
            manager: songbird::Songbird::serenity(),
            active: Mutex::new(Some(session)),
            byok: None,
            http: Arc::new(serenity::Http::new("test-token")),
            killed: std::sync::atomic::AtomicBool::new(false),
        };
        assert_eq!(
            finish(&service, guild, started, 0).await,
            Some((UserId::new(3), vec![2]))
        );
        let guard = service.active.lock().await;
        let session = guard.as_ref().unwrap();
        assert!(session.busy);
        assert_eq!(session.active_user, Some(UserId::new(3)));
        assert!(session.queued.is_empty());
        assert!(session.last_activity.elapsed() < Duration::from_secs(2));
        drop(guard);
        assert_eq!(finish(&service, guild, started, 0).await, None);
        let guard = service.active.lock().await;
        let session = guard.as_ref().unwrap();
        assert!(!session.busy);
        assert!(session.last_activity.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn transcript_display_disables_mentions_and_markup() {
        assert_eq!(
            safe_discord_text("hi @everyone *wow*", 100),
            "hi everyone wow"
        );
    }
}
