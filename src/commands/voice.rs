use crate::{
    CustomContext, Error, database,
    voice::{VoiceSession, VoiceSettings},
};
use poise::{
    CreateReply,
    serenity_prelude::{
        self as serenity, ChannelType, Colour, CreateAllowedMentions, CreateEmbed, EditMessage,
        Permissions,
    },
};
use std::sync::atomic::Ordering;
use std::time::Duration;

#[poise::command(
    slash_command,
    dm_only,
    subcommands("kill", "resume", "approve", "revoke", "admin_status"),
    subcommand_required,
    category = "Voice"
)]
pub async fn voiceadmin(_ctx: CustomContext<'_>) -> Result<(), Error> {
    Ok(())
}

fn bot_owner(ctx: &CustomContext<'_>) -> bool {
    ctx.guild_id().is_none()
        && crate::config::DEVELOPER_IDS
            .get()
            .is_some_and(|owners| owners.contains(&ctx.author().id.get()))
}

async fn disconnect_active(service: &crate::voice::VoiceService, reason: &str) -> Option<String> {
    let old = {
        let mut guard = service.active.lock().await;
        guard.take().map(|mut session| {
            session.stop();
            session
        })
    };
    let old = old?;
    let guild = old.guild;
    let model = crate::voice::resources::model_for(&old.settings);
    let _ = service.manager.remove(guild).await;
    if let Some(message) = old.status_message {
        let _ = old
            .text_channel
            .edit_message(
                &service.http,
                message,
                EditMessage::new().embed(
                    CreateEmbed::new()
                        .title("OmniCore Voice Assistant")
                        .description(reason)
                        .colour(Colour::DARK_GREY),
                ),
            )
            .await;
    }
    let _ = old.text_channel.say(&service.http, reason).await;
    model
}

/// Stop all voice use and free the active local voice models (bot owners, DM only).
#[poise::command(slash_command, dm_only)]
pub async fn kill(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !bot_owner(&ctx) {
        return private(&ctx, "Only OmniCore bot owners can use this in my DMs.").await;
    }
    ctx.defer_ephemeral().await?;
    ctx.data().voice.killed.store(true, Ordering::SeqCst);
    let model = disconnect_active(
        &ctx.data().voice,
        "Voice stopped by an OmniCore bot owner. Consent and temporary voice context were cleared.",
    )
    .await;
    crate::voice::policy::set_killed(true).await?;
    let unloaded = match model {
        Some(model) => crate::voice::resources::unload_model(&model).await,
        None => true,
    };
    let memory = if unloaded {
        "The voice Ollama model is not loaded."
    } else {
        "I could not confirm that Ollama unloaded the voice model; check `ollama ps` on the Mac."
    };
    private(&ctx, &format!("Voice kill switch is on. The active session was disconnected and temporary buffers and speech recognition were released. {memory} Use `/voiceadmin resume` to allow new sessions.")).await
}

/// Allow new voice sessions after an emergency stop (bot owners, DM only).
#[poise::command(slash_command, dm_only)]
pub async fn resume(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !bot_owner(&ctx) {
        return private(&ctx, "Only OmniCore bot owners can use this in my DMs.").await;
    }
    crate::voice::policy::set_killed(false).await?;
    ctx.data().voice.killed.store(false, Ordering::SeqCst);
    private(
        &ctx,
        "Voice is available again in approved servers. No voice session was started automatically.",
    )
    .await
}

fn guild_id(raw: &str) -> Result<serenity::GuildId, &'static str> {
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value != 0)
        .map(serenity::GuildId::new)
        .ok_or("Enter a valid numeric server ID.")
}

/// Approve voice in another server where OmniCore is installed (bot owners, DM only).
#[poise::command(slash_command, dm_only)]
pub async fn approve(
    ctx: CustomContext<'_>,
    #[description = "Numeric Discord server ID"] server_id: String,
) -> Result<(), Error> {
    if !bot_owner(&ctx) {
        return private(&ctx, "Only OmniCore bot owners can use this in my DMs.").await;
    }
    ctx.defer_ephemeral().await?;
    let guild = guild_id(&server_id)?;
    if guild.to_partial_guild(ctx.http()).await.is_err() {
        return private(&ctx, "OmniCore is not in that server or cannot access it.").await;
    }
    crate::voice::policy::approve(guild).await?;
    crate::register_instant_guild_commands(ctx.http(), guild).await;
    private(&ctx, &format!("Voice approved for server {guild}. Its server owner can enable voice with `/voice settings enabled:true`." )).await
}

/// Revoke a previous voice approval (bot owners, DM only).
#[poise::command(slash_command, dm_only)]
pub async fn revoke(
    ctx: CustomContext<'_>,
    #[description = "Numeric Discord server ID"] server_id: String,
) -> Result<(), Error> {
    if !bot_owner(&ctx) {
        return private(&ctx, "Only OmniCore bot owners can use this in my DMs.").await;
    }
    ctx.defer_ephemeral().await?;
    let guild = guild_id(&server_id)?;
    if crate::voice::policy::builtin(guild) {
        return private(&ctx, "This is one of the three built-in voice servers. Use `/voiceadmin kill` to stop all voice use.").await;
    }
    crate::voice::policy::revoke(guild).await?;
    crate::register_instant_guild_commands(ctx.http(), guild).await;
    let active = ctx
        .data()
        .voice
        .active
        .lock()
        .await
        .as_ref()
        .is_some_and(|s| s.guild == guild);
    if active {
        let model = disconnect_active(
            &ctx.data().voice,
            "Voice access was revoked by an OmniCore bot owner.",
        )
        .await;
        if let Some(model) = model {
            let _ = crate::voice::resources::unload_model(&model).await;
        }
    }
    private(&ctx, &format!("Voice approval revoked for server {guild}.")).await
}

/// View the voice kill switch and active server (bot owners, DM only).
#[poise::command(slash_command, dm_only, rename = "status")]
pub async fn admin_status(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !bot_owner(&ctx) {
        return private(&ctx, "Only OmniCore bot owners can use this in my DMs.").await;
    }
    let active = ctx
        .data()
        .voice
        .active
        .lock()
        .await
        .as_ref()
        .map(|s| s.guild.to_string());
    private(&ctx, &format!("Kill switch: {}\nActive server: {}\nBuilt-in voice servers: SMS, Minecraft Server, OmniCore Technologies.",
        if ctx.data().voice.killed.load(Ordering::SeqCst) { "On" } else { "Off" }, active.as_deref().unwrap_or("None"))).await
}

#[poise::command(
    slash_command,
    guild_only,
    subcommands("status", "settings", "consent", "stop", "test", "stats"),
    subcommand_required,
    category = "Voice"
)]
pub async fn voice(_ctx: CustomContext<'_>) -> Result<(), Error> {
    Ok(())
}

async fn private(ctx: &CustomContext<'_>, message: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(message).ephemeral(true))
        .await?;
    Ok(())
}

fn caller_voice_channel(ctx: &CustomContext<'_>) -> Option<serenity::ChannelId> {
    ctx.guild()?
        .voice_states
        .get(&ctx.author().id)
        .and_then(|state| state.channel_id)
}

async fn is_guild_owner(ctx: &CustomContext<'_>) -> Result<bool, Error> {
    let guild = ctx.guild_id().ok_or("Server required")?;
    Ok(guild.to_partial_guild(ctx.http()).await?.owner_id == ctx.author().id)
}

fn notice(channel: serenity::ChannelId, settings: &VoiceSettings, count: usize) -> CreateEmbed {
    CreateEmbed::new()
        .title("OmniCore Voice Assistant")
        .colour(Colour::from_rgb(105, 92, 230))
        .field("Connected", format!("<#{channel}>"), true)
        .field("AI provider", &settings.provider, true)
        .field("Model", if settings.model.is_empty() { "Server's selected model" } else { &settings.model }, true)
        .field("Speaking voice", &settings.speaking_voice, true)
        .field("Opted-in participants", count.to_string(), true)
        .field("Status", if count == 0 { "Waiting for consent" } else { "Ready" }, true)
        .description("Voice processing is opt-in. After you consent, speak normally; no wake phrase is needed. Recognized speech and OmniCore's text response are posted in this text channel. The session disconnects after about two minutes without voice use, releases local voice resources, and can be restarted with `/voicejoin`. Use `/voice consent enabled:false` to opt out or `/voiceleave` to disconnect.")
}

async fn refresh_notice(service: &crate::voice::VoiceService, guild: serenity::GuildId) {
    let snapshot = {
        let guard = service.active.lock().await;
        guard.as_ref().filter(|s| s.guild == guild).and_then(|s| {
            s.status_message.map(|id| {
                (
                    s.text_channel,
                    id,
                    s.channel,
                    s.settings.clone(),
                    s.consenting.len(),
                )
            })
        })
    };
    if let Some((text, message, channel, settings, count)) = snapshot {
        let _ = text
            .edit_message(
                &service.http,
                message,
                EditMessage::new().embed(notice(channel, &settings, count)),
            )
            .await;
    }
}

async fn clear_failed_join(
    service: &crate::voice::VoiceService,
    guild: serenity::GuildId,
    started: std::time::Instant,
) {
    {
        let mut active = service.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|session| session.guild == guild && session.started == started)
        {
            if let Some(session) = active.as_mut() {
                session.stop();
            }
            *active = None;
        }
    }
    let _ = service.manager.remove(guild).await;
}

/// Join your current voice channel for opt-in voice conversation.
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn voicejoin(ctx: CustomContext<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Server required")?;
    if ctx.data().voice.killed.load(Ordering::SeqCst) {
        return private(&ctx, "Voice is paused by an OmniCore bot owner.").await;
    }
    if !crate::voice::policy::allowed(guild_id).await? {
        return private(
            &ctx,
            "Voice has not been approved for this server by an OmniCore bot owner.",
        )
        .await;
    }
    let Some(channel_id) = caller_voice_channel(&ctx) else {
        return private(&ctx, "Join a voice channel first, then run `/voicejoin`.").await;
    };
    let settings = VoiceSettings::load(guild_id).await?;
    if !settings.enabled {
        return private(&ctx, "Voice is disabled for this server. The server owner can enable it with `/voice settings`.").await;
    }
    if settings
        .allowed_voice_channel
        .is_some_and(|allowed| allowed != channel_id.get())
    {
        return private(
            &ctx,
            "Voice is restricted to a different channel in this server.",
        )
        .await;
    }
    if let Some(role) = settings.allowed_role {
        let member = ctx.author_member().await.ok_or("Member unavailable")?;
        if !member.roles.iter().any(|id| id.get() == role) {
            return private(
                &ctx,
                "Your role is not allowed to start a voice session here.",
            )
            .await;
        }
    }
    if let Err(error) = crate::voice::audio::check_models(&settings.speaking_voice) {
        log::warn!("Voice model missing: {error}");
        return private(&ctx, "Local speech models are not installed yet. Ask an OmniCore bot owner to finish voice setup.").await;
    }
    if settings.provider == "ollama" {
        if !database::is_ai_approved(guild_id.get()).await? {
            return private(&ctx, "Local AI is not approved for this server. Ask an OmniCore bot owner to approve it, or ask the server owner to choose a connected provider in `/voice settings`.").await;
        }
    } else {
        let Some(byok) = &ctx.data().byok else {
            return private(
                &ctx,
                "A connected AI provider is unavailable. The server owner can run `/ai setup`.",
            )
            .await;
        };
        let connection = byok
            .connection(guild_id.get())
            .await
            .map_err(|_| "AI provider settings unavailable")?;
        if connection
            .as_ref()
            .is_none_or(|connected| connected.provider.name() != settings.provider)
        {
            return private(&ctx, "The selected AI provider is not connected. The server owner can run `/ai setup` or change `/voice settings`.").await;
        }
    }
    let bot_id = ctx.cache().current_user().id;
    let bot_member = guild_id.member(ctx.http(), bot_id).await?;
    let (channel, permissions) = {
        let guild = ctx.guild().ok_or("Server unavailable")?;
        let channel = guild
            .channels
            .get(&channel_id)
            .ok_or("Voice channel unavailable")?
            .clone();
        let permissions = guild.user_permissions_in(&channel, &bot_member);
        (channel, permissions)
    };
    if channel.kind != ChannelType::Voice {
        return private(
            &ctx,
            "Join a standard voice channel. Stage channels are not supported.",
        )
        .await;
    }
    if !permissions.contains(Permissions::VIEW_CHANNEL | Permissions::CONNECT | Permissions::SPEAK)
    {
        return private(
            &ctx,
            "I need View Channel, Connect, and Speak permissions in that voice channel.",
        )
        .await;
    }
    let active = ctx.data().voice.active.lock().await;
    if active.is_some() {
        drop(active);
        return private(
            &ctx,
            "OmniCore is already in a voice session. Only one server can use voice at a time.",
        )
        .await;
    }
    if crate::commands::ai::imagegen::generation_active() {
        drop(active);
        return private(&ctx, "Local image generation is running. Try `/voicejoin` when it finishes so both models do not compete for memory.").await;
    }
    drop(active);
    ctx.defer().await?;
    let stt_path = crate::voice::audio::model_root().join("ggml-base.en.bin");
    let stt =
        match tokio::task::spawn_blocking(move || crate::voice::audio::load_stt(&stt_path)).await {
            Ok(Ok(model)) => std::sync::Arc::new(model),
            _ => {
                return private(
                    &ctx,
                    "Local speech recognition could not load. Check the voice model installation.",
                )
                .await;
            }
        };
    let mut session = VoiceSession::new(guild_id, channel_id, ctx.channel_id());
    session.settings = settings.clone();
    session.stt = Some(stt);
    if let Some(guild) = ctx.guild() {
        session.present = guild
            .voice_states
            .iter()
            .filter_map(|(user, state)| {
                (state.channel_id == Some(channel_id)
                    && *user != bot_id
                    && !guild
                        .members
                        .get(user)
                        .is_some_and(|member| member.user.bot))
                .then_some(*user)
            })
            .collect();
    }
    if session.present.is_empty() {
        session.empty_since = Some(std::time::Instant::now());
    }
    let started = session.started;
    {
        let mut active = ctx.data().voice.active.lock().await;
        if ctx.data().voice.killed.load(Ordering::SeqCst)
            || !crate::voice::policy::allowed(guild_id).await?
        {
            return private(&ctx, "Voice access changed while speech recognition loaded. Try again after a bot owner enables voice.").await;
        }
        if active.is_some() || crate::commands::ai::imagegen::generation_active() {
            drop(active);
            return private(&ctx, "Another voice or image-generation session started while speech recognition loaded. Try again shortly.").await;
        }
        *active = Some(session);
    }
    let call = ctx.data().voice.manager.get_or_insert(guild_id);
    {
        let mut call = call.lock().await;
        call.set_config(
            songbird::Config::default().decode_mode(songbird::driver::DecodeMode::Decrypt),
        );
        let receiver = || crate::voice::pipeline::Receiver {
            service: ctx.data().voice.clone(),
            guild: guild_id,
            bot: bot_id,
        };
        call.add_global_event(
            songbird::Event::Core(songbird::events::CoreEvent::SpeakingStateUpdate),
            receiver(),
        );
        call.add_global_event(
            songbird::Event::Core(songbird::events::CoreEvent::VoiceTick),
            receiver(),
        );
        call.add_global_event(
            songbird::Event::Core(songbird::events::CoreEvent::RtpPacket),
            receiver(),
        );
        call.add_global_event(
            songbird::Event::Core(songbird::events::CoreEvent::ClientDisconnect),
            receiver(),
        );
        call.add_global_event(
            songbird::Event::Core(songbird::events::CoreEvent::DriverDisconnect),
            receiver(),
        );
    }
    match tokio::time::timeout(
        Duration::from_secs(25),
        ctx.data().voice.manager.join(guild_id, channel_id),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            clear_failed_join(&ctx.data().voice, guild_id, started).await;
            log::warn!("Voice join failed guild={guild_id} channel={channel_id}: {error:?}");
            private(
                &ctx,
                "Discord voice connection failed. Check voice permissions and try again.",
            )
            .await?;
            return Ok(());
        }
        Err(_) => {
            clear_failed_join(&ctx.data().voice, guild_id, started).await;
            private(
                &ctx,
                "Discord voice connection timed out. Please try again later.",
            )
            .await?;
            return Ok(());
        }
    }
    let reply = ctx
        .send(
            CreateReply::default()
                .embed(notice(channel_id, &settings, 0))
                .allowed_mentions(
                    CreateAllowedMentions::new()
                        .empty_users()
                        .empty_roles()
                        .everyone(false),
                ),
        )
        .await;
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            clear_failed_join(&ctx.data().voice, guild_id, started).await;
            return Err(error.into());
        }
    };
    let status_message = reply.message().await.ok().map(|message| message.id);
    {
        let mut active = ctx.data().voice.active.lock().await;
        let Some(session) = active
            .as_mut()
            .filter(|session| session.guild == guild_id && session.started == started)
        else {
            drop(active);
            let _ = ctx.data().voice.manager.remove(guild_id).await;
            return private(
                &ctx,
                "Voice connection ended during setup. Try `/voicejoin` again.",
            )
            .await;
        };
        session.status_message = status_message;
    }
    log::info!("Voice joined guild={guild_id} channel={channel_id}");
    if let Ok(collection) = database::get_collection(crate::voice::VOICE_USAGE) {
        let _ = collection.update_one(
            mongodb::bson::doc! {"guild_id": guild_id.to_string(), "day": chrono::Utc::now().format("%Y-%m-%d").to_string()},
            mongodb::bson::doc! {"$inc": {"sessions": 1i64}, "$set": {"expiresAt": crate::voice::retention_date()}},
        ).upsert(true).await;
    }
    Ok(())
}

/// Leave the current voice channel and clear the voice session.
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn voiceleave(ctx: CustomContext<'_>) -> Result<(), Error> {
    let guild = ctx.guild_id().ok_or("Server required")?;
    let owner = is_guild_owner(&ctx).await?;
    let bot_owner = ctx.framework().options().owners.contains(&ctx.author().id);
    ctx.defer_ephemeral().await?;
    let mut active = ctx.data().voice.active.lock().await;
    let Some(session) = active.as_mut().filter(|session| session.guild == guild) else {
        return private(&ctx, "OmniCore has no voice session in this server.").await;
    };
    if !owner && !bot_owner && caller_voice_channel(&ctx) != Some(session.channel) {
        return private(
            &ctx,
            "Only a participant in this voice channel or a server owner can disconnect OmniCore.",
        )
        .await;
    }
    let model = crate::voice::resources::model_for(&session.settings);
    session.stop();
    let started = session.started;
    let status = session
        .status_message
        .map(|message| (session.text_channel, message));
    drop(active);
    let removal_failed = if let Err(error) = ctx.data().voice.manager.remove(guild).await {
        log::warn!("Voice leave failed guild={guild}: {error:?}");
        true
    } else {
        false
    };
    {
        let mut active = ctx.data().voice.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|session| session.guild == guild && session.started == started)
        {
            *active = None;
        }
    }
    if let Some(model) = model
        && !crate::voice::resources::unload_model(&model).await
    {
        log::warn!("Voice model unload could not be confirmed after /voiceleave guild={guild}");
    }
    if let Some((text, message)) = status {
        let _ = text
            .edit_message(
                ctx.http(),
                message,
                EditMessage::new().embed(
                    CreateEmbed::new()
                        .title("OmniCore Voice Assistant")
                        .description("Disconnected. Consent and temporary context were cleared.")
                        .colour(Colour::DARK_GREY),
                ),
            )
            .await;
    }
    if removal_failed {
        private(&ctx, "Voice processing and local resources stopped, but Discord did not confirm disconnection. Please retry `/voiceleave`.").await
    } else {
        private(
            &ctx,
            "Disconnected from voice. Consent and temporary session context were cleared.",
        )
        .await
    }
}

/// Show the current voice session without revealing private speech.
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn status(ctx: CustomContext<'_>) -> Result<(), Error> {
    let guild = ctx.guild_id().ok_or("Server required")?;
    let active = ctx.data().voice.active.lock().await;
    let Some(session) = active.as_ref().filter(|session| session.guild == guild) else {
        return private(&ctx, "OmniCore is not connected to voice in this server.").await;
    };
    let settings = &session.settings;
    private(&ctx, &format!("Connected: <#{}>\nProcessing: {}\nAI provider: {}\nModel: {}\nWake phrase: none\nOpted-in participants: {}\nSession duration: {} minutes\nQueue: {}\nAudio packets: received {}, opted-in {}, decoded {}", session.channel, if session.stopped { "Stopped" } else if session.consenting.is_empty() { "Waiting for consent" } else { "Ready" }, settings.provider, if settings.model.is_empty() { "Server's selected model" } else { &settings.model }, session.consenting.len(), session.started.elapsed().as_secs() / 60, if session.busy { "Processing one request" } else { "Idle" }, session.received_packets, session.authorized_packets, session.decoded_packets)).await
}

/// Configure server voice behavior (server owner only).
#[allow(clippy::too_many_arguments)] // Each option is an independently named Discord slash-command field.
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn settings(
    ctx: CustomContext<'_>,
    #[description = "Enable or disable voice"] enabled: Option<bool>,
    #[description = "ollama, openai, anthropic, or gemini"] provider: Option<String>,
    #[description = "Local Ollama model; cloud models use /ai model"] model: Option<String>,
    #[description = "Local speaking voice"] speaking_voice: Option<String>,
    #[description = "Follow-up window, 5–60 seconds"] followup_seconds: Option<u32>,
    #[description = "Idle disconnect, 1–2 minutes"] idle_minutes: Option<u32>,
    #[description = "Maximum spoken answer, 20–150 words"] max_spoken_words: Option<u32>,
    #[description = "Daily voice AI requests, 1–200"] daily_requests: Option<u32>,
    #[description = "Restrict voice to this channel"] allowed_voice_channel: Option<
        serenity::GuildChannel,
    >,
    #[description = "Restrict voice starts to this role"] allowed_role: Option<serenity::Role>,
) -> Result<(), Error> {
    if !is_guild_owner(&ctx).await? {
        return private(&ctx, "Only this server's owner can change voice settings.").await;
    }
    ctx.defer_ephemeral().await?;
    let guild = ctx.guild_id().ok_or("Server required")?;
    let mut current = VoiceSettings::load(guild).await?;
    let changed = enabled.is_some()
        || provider.is_some()
        || model.is_some()
        || speaking_voice.is_some()
        || followup_seconds.is_some()
        || idle_minutes.is_some()
        || max_spoken_words.is_some()
        || daily_requests.is_some()
        || allowed_voice_channel.is_some()
        || allowed_role.is_some();
    if enabled == Some(true) && !crate::voice::policy::allowed(guild).await? {
        return private(
            &ctx,
            "An OmniCore bot owner must approve voice for this server first.",
        )
        .await;
    }
    if let Some(value) = enabled {
        current.enabled = value;
    }
    if let Some(value) = provider {
        if !matches!(value.as_str(), "ollama" | "openai" | "anthropic" | "gemini") {
            return private(&ctx, "Choose ollama, openai, anthropic, or gemini.").await;
        }
        current.provider = value;
    }
    if let Some(value) = model {
        if current.provider != "ollama" {
            return private(&ctx, "Cloud voice uses the server's existing `/ai model` selection. Set voice provider to ollama to choose a local model here.").await;
        }
        if value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
            })
        {
            return private(
                &ctx,
                "The model ID contains unsupported characters or is too long.",
            )
            .await;
        }
        current.model = value;
    }
    if current.provider != "ollama" {
        current.model.clear();
    }
    if let Some(value) = speaking_voice {
        if !matches!(value.as_str(), "en_US-lessac-medium" | "en_US-amy-low") {
            return private(&ctx, "Choose en_US-lessac-medium or en_US-amy-low.").await;
        }
        current.speaking_voice = value;
    }
    if let Some(value) = followup_seconds {
        if !(5..=60).contains(&value) {
            return private(&ctx, "Follow-up window must be 5–60 seconds.").await;
        }
        current.followup_seconds = value;
    }
    if let Some(value) = idle_minutes {
        if !(1..=2).contains(&value) {
            return private(&ctx, "Idle disconnect must be 1–2 minutes.").await;
        }
        current.idle_minutes = value;
    }
    if let Some(value) = max_spoken_words {
        if !(20..=150).contains(&value) {
            return private(&ctx, "Spoken answers must be limited to 20–150 words.").await;
        }
        current.max_spoken_words = value;
    }
    if let Some(value) = daily_requests {
        if !(1..=200).contains(&value) {
            return private(&ctx, "Daily voice requests must be 1–200.").await;
        }
        current.daily_requests = value;
    }
    if let Some(channel) = allowed_voice_channel {
        if channel.guild_id != guild || channel.kind != ChannelType::Voice {
            return private(&ctx, "Choose a standard voice channel in this server.").await;
        }
        current.allowed_voice_channel = Some(channel.id.get());
    }
    if let Some(role) = allowed_role {
        current.allowed_role = Some(role.id.get());
    }
    if changed {
        current.save(guild).await?;
        if !current.enabled {
            let mut guard = ctx.data().voice.active.lock().await;
            if let Some(session) = guard.as_mut().filter(|s| s.guild == guild) {
                let model = crate::voice::resources::model_for(&session.settings);
                session.stop();
                *guard = None;
                drop(guard);
                let _ = ctx.data().voice.manager.remove(guild).await;
                if let Some(model) = model {
                    let _ = crate::voice::resources::unload_model(&model).await;
                }
            }
        }
    }
    private(&ctx, &format!("Voice: {}\nAI provider: {}\nModel: {}\nSpeaking voice: {}\nWake phrase: none\nFollow-ups: {} seconds\nIdle disconnect: {} minutes\nMaximum spoken answer: {} words\nDaily request limit: {}\nAllowed voice channel: {}\nAllowed role: {}\nLocal Ollama thinking: set with `/ai thinking` (shared with text mentions)\nSpeech recognition and TTS: local\nConsent: required for every participant; transcripts and text responses appear in the voice session's text channel.\nCloud AI may incur provider charges when selected.", if current.enabled { "Enabled" } else { "Disabled" }, current.provider, if current.model.is_empty() { "Existing selected model" } else { &current.model }, current.speaking_voice, current.followup_seconds, current.idle_minutes, current.max_spoken_words, current.daily_requests, current.allowed_voice_channel.map_or("Any".to_owned(), |id| format!("<#{id}>")), current.allowed_role.map_or("Any".to_owned(), |id| format!("<@&{id}>")))).await
}

/// Opt into or out of voice processing for the active session.
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn consent(
    ctx: CustomContext<'_>,
    #[description = "Grant or revoke voice processing consent"] enabled: bool,
) -> Result<(), Error> {
    let guild = ctx.guild_id().ok_or("Server required")?;
    let mut active = ctx.data().voice.active.lock().await;
    let Some(session) = active
        .as_mut()
        .filter(|session| session.guild == guild && !session.stopped)
    else {
        return private(&ctx, "There is no active voice session in this server.").await;
    };
    if enabled {
        if caller_voice_channel(&ctx) != Some(session.channel) {
            return private(&ctx, "Join OmniCore's voice channel before opting in.").await;
        }
        if let Some(role) = session.settings.allowed_role {
            let member = ctx.author_member().await.ok_or("Member unavailable")?;
            if !member.roles.iter().any(|id| id.get() == role) {
                return private(
                    &ctx,
                    "Your role is not allowed to use voice in this server.",
                )
                .await;
            }
        }
        session.consenting.insert(ctx.author().id);
        session.last_activity = std::time::Instant::now();
        drop(active);
        refresh_notice(&ctx.data().voice, guild).await;
        private(&ctx, "You opted in for this voice session. Speak normally; no wake phrase is needed. Your recognized speech and OmniCore's text responses will appear in the text channel where `/voicejoin` was run. After about two minutes without use, the session disconnects; use `/voicejoin` and opt in again. Use `/voice consent enabled:false` to opt out immediately.").await
    } else {
        session.revoke(ctx.author().id);
        drop(active);
        refresh_notice(&ctx.data().voice, guild).await;
        private(&ctx, "You opted out. Your temporary voice buffer and conversation context have been cleared.").await
    }
}

/// Interrupt the current spoken response immediately.
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn stop(ctx: CustomContext<'_>) -> Result<(), Error> {
    let guild = ctx.guild_id().ok_or("Server required")?;
    let mut guard = ctx.data().voice.active.lock().await;
    let Some(session) = guard.as_mut().filter(|s| s.guild == guild) else {
        return private(&ctx, "No voice session is active here.").await;
    };
    if caller_voice_channel(&ctx) != Some(session.channel) && !is_guild_owner(&ctx).await? {
        return private(&ctx, "Join the active voice channel to stop playback.").await;
    }
    session.interrupt();
    session.last_activity = std::time::Instant::now();
    private(&ctx, "Voice response stopped.").await
}

/// Play a short local voice sample (server owner only).
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn test(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !is_guild_owner(&ctx).await? {
        return private(&ctx, "Only the server owner can play the voice test.").await;
    }
    let guild = ctx.guild_id().ok_or("Server required")?;
    let (voice, started) = {
        let guard = ctx.data().voice.active.lock().await;
        let Some(s) = guard.as_ref().filter(|s| s.guild == guild && !s.stopped) else {
            return private(&ctx, "Use `/voicejoin` first.").await;
        };
        (s.settings.speaking_voice.clone(), s.started)
    };
    ctx.defer_ephemeral().await?;
    let audio = tokio::task::spawn_blocking(move || {
        crate::voice::audio::synthesize(
            "Hello! I'm OmniCore. Voice chat is working.",
            &voice,
            &crate::voice::audio::model_root(),
        )
    })
    .await;
    let Ok(Ok(bytes)) = audio else {
        return private(
            &ctx,
            "The local speaking voice could not generate the test sample.",
        )
        .await;
    };
    let Some(call) = ctx.data().voice.manager.get(guild) else {
        return private(&ctx, "Voice connection was lost.").await;
    };
    let track = call
        .lock()
        .await
        .play_only_input(songbird::input::Input::from(bytes));
    let mut guard = ctx.data().voice.active.lock().await;
    if let Some(s) = guard
        .as_mut()
        .filter(|s| s.guild == guild && s.started == started)
    {
        s.interrupt();
        s.track = Some(track);
        s.last_activity = std::time::Instant::now();
    } else {
        let _ = track.stop();
    }
    private(&ctx, "Playing the local voice test.").await
}

/// Show measured voice usage for this month (server owner only).
#[poise::command(slash_command, guild_only, category = "Voice")]
pub async fn stats(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !is_guild_owner(&ctx).await? {
        return private(&ctx, "Only the server owner can view voice analytics.").await;
    }
    let guild = ctx.guild_id().ok_or("Server required")?;
    let start = chrono::Utc::now().format("%Y-%m-01").to_string();
    let collection = database::get_collection("voice_usage")?;
    let mut cursor = collection
        .find(mongodb::bson::doc! {"guild_id": guild.to_string(), "day": {"$gte": start}})
        .await?;
    use futures::TryStreamExt;
    let mut requests = 0i64;
    let mut completed = 0i64;
    let mut sessions = 0i64;
    let mut stt_ms = 0i64;
    let mut ai_ms = 0i64;
    let mut tts_ms = 0i64;
    while let Some(doc) = cursor.try_next().await? {
        requests += doc.get_i64("requests").unwrap_or(0);
        completed += doc.get_i64("completed").unwrap_or(0);
        sessions += doc.get_i64("sessions").unwrap_or(0);
        stt_ms += doc.get_i64("stt_ms").unwrap_or(0);
        ai_ms += doc.get_i64("ai_ms").unwrap_or(0);
        tts_ms += doc.get_i64("tts_ms").unwrap_or(0);
    }
    let timing = if completed > 0 {
        format!(
            "Measured mean processing time for completed turns: STT {} ms, AI {} ms, TTS {} ms.",
            stt_ms / completed,
            ai_ms / completed,
            tts_ms / completed
        )
    } else {
        "No completed turns yet, so processing times are not available.".to_owned()
    };
    private(&ctx, &format!("This month: {sessions} sessions, {requests} AI requests, {completed} completed.\n{timing} Audio and transcripts are not stored in usage analytics.")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_controls_are_dm_only_and_bounded_to_known_subcommands() {
        let command = voiceadmin();
        assert!(command.dm_only);
        assert!(command.subcommand_required);
        let names: Vec<_> = command
            .subcommands
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, ["kill", "resume", "approve", "revoke", "status"]);
        assert!(command.subcommands.iter().all(|item| item.dm_only));
        assert!(guild_id("not-an-id").is_err());
        assert_eq!(
            guild_id("1554320480478167050").unwrap().get(),
            1554320480478167050
        );
    }
}
