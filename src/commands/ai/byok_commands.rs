use crate::{
    CustomContext, Error,
    byok::{
        portal,
        provider::ProviderError,
        storage::{AiError, ByokService},
    },
};
use poise::{
    CreateReply,
    serenity_prelude::{
        self as serenity, CreateAllowedMentions, CreateEmbed, CreateMessage, GuildChannel, Message,
        Role,
    },
};
use std::sync::Arc;

#[poise::command(
    slash_command,
    guild_only,
    subcommands(
        "setup",
        "status",
        "settings",
        "provider",
        "model",
        "usage",
        "limits",
        "channel",
        "role",
        "disconnect"
    ),
    subcommand_required,
    category = "AI"
)]
pub async fn ai(_ctx: CustomContext<'_>) -> Result<(), Error> {
    Ok(())
}

fn service<'a>(ctx: &'a CustomContext<'_>) -> Option<&'a Arc<ByokService>> {
    ctx.data().byok.as_ref()
}

async fn owner(ctx: &CustomContext<'_>) -> Result<bool, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(false);
    };
    let guild = guild_id.to_partial_guild(ctx.http()).await?;
    if guild.owner_id == ctx.author().id {
        return Ok(true);
    }
    respond(
        ctx,
        "Only this server's owner can change its AI provider settings.",
    )
    .await?;
    Ok(false)
}

async fn respond(ctx: &CustomContext<'_>, text: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

fn error_message(error: AiError) -> &'static str {
    match error {
        AiError::NotConnected => "No AI provider is connected. The server owner can run /ai setup.",
        AiError::Forbidden => {
            "AI is not available to you in this channel or role, or the question is too long."
        }
        AiError::Quota => "This server's AI request limit has been reached for today.",
        AiError::Busy => "This server has too many AI requests in progress. Try again shortly.",
        AiError::InvalidModel => "That model is not available for this provider account.",
        AiError::Provider(ProviderError::InvalidKey) => {
            "The provider rejected the connected API key. Ask the server owner to reconnect it."
        }
        AiError::Provider(ProviderError::RateLimited) => {
            "The AI provider is rate limiting this server. Try again later."
        }
        AiError::Provider(_) => "The AI provider could not complete this request.",
        AiError::Storage => "The AI settings service is temporarily unavailable.",
    }
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn setup(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(_service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let Some(url) =
        portal::create_setup_link(ctx.guild_id().ok_or("Guild required")?.get()).await?
    else {
        return respond(&ctx, "The secure pairing portal is not configured yet. Ask the OmniCore operator to finish OAuth setup.").await;
    };
    respond(&ctx, &format!("Open this private, single-use HTTPS setup link within 10 minutes:\n{url}\n\nChoose OpenAI, Anthropic, or Gemini and enter your API key on the authenticated page. API usage may be billed directly by your provider. Do not paste your key into Discord.")).await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn status(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let connection = service
        .connection(guild)
        .await
        .map_err(|_| "AI settings unavailable")?;
    let Some(connection) = connection else {
        return respond(&ctx, "No guild AI provider is connected. Run /ai setup to connect one. Existing locally hosted OmniCore AI may still be available if approved.").await;
    };
    let usage = service
        .usage(guild)
        .await
        .map_err(|_| "AI usage unavailable")?;
    let limits = service
        .limits(guild)
        .await
        .map_err(|_| "AI limits unavailable")?;
    let embed = CreateEmbed::new()
        .title("OmniCore AI configuration")
        .field("Provider", connection.provider.name(), true)
        .field("Model", connection.model, true)
        .field("Connection", "Connected", true)
        .field("Key", "Stored securely; never displayed", false)
        .field(
            "Usage, last 30 days",
            format!(
                "{} requests, {} successful",
                usage.requests, usage.successful
            ),
            false,
        )
        .field(
            "Daily limits",
            format!(
                "{} per member, {} per server; {} concurrent",
                limits.user_daily, limits.guild_daily, limits.concurrency
            ),
            false,
        )
        .field(
            "Last validation",
            format!(
                "<t:{}:F>",
                connection.validated_at.timestamp_millis() / 1_000
            ),
            false,
        );
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn settings(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let limits = service
        .limits(guild)
        .await
        .map_err(|_| "AI settings unavailable")?;
    respond(&ctx, &format!("AI limits: {} requests per member per day, {} per server per day, {} output tokens per answer, {} concurrent requests. Allowed channels: {:?}. Allowed roles: {:?}. Run /ai setup to manage your provider.", limits.user_daily, limits.guild_daily, limits.output_tokens, limits.concurrency, limits.channels, limits.roles)).await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn provider(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let current = service
        .connection(guild)
        .await
        .map_err(|_| "AI settings unavailable")?;
    respond(&ctx, &format!("Provider: {}. Run /ai setup to connect or replace a provider through the secure portal.",
        current.map_or("not connected", |connection| connection.provider.name()))).await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn model(
    ctx: CustomContext<'_>,
    #[description = "Model ID returned by your provider"] model: String,
) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    match service
        .change_model(ctx.guild_id().ok_or("Guild required")?.get(), &model)
        .await
    {
        Ok(()) => respond(&ctx, "The server's AI model was updated.").await,
        Err(error) => respond(&ctx, error_message(error)).await,
    }
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn usage(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let summary = service
        .usage(guild)
        .await
        .map_err(|_| "AI usage unavailable")?;
    let tokens = if summary.missing_tokens > 0 {
        format!(
            "At least {} input and {} output; some provider token counts were unavailable",
            summary.input_tokens, summary.output_tokens
        )
    } else {
        format!(
            "{} input, {} output, {} total",
            summary.input_tokens,
            summary.output_tokens,
            summary.input_tokens + summary.output_tokens
        )
    };
    respond(&ctx, &format!("**OmniCore AI usage — last 30 days**\nRequests: {}\nSuccessful: {}\nFailures/rate limits: {}\nTokens: {}\nCharges are billed by your provider. OmniCore does not calculate an exact bill.",
        summary.requests, summary.successful, summary.failures, tokens)).await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn limits(
    ctx: CustomContext<'_>,
    #[description = "Max requests per member per day"] user_daily: Option<i32>,
    #[description = "Max requests per server per day"] guild_daily: Option<i32>,
    #[description = "Max output tokens per request"] output_tokens: Option<i32>,
    #[description = "Max simultaneous requests"] concurrency: Option<i32>,
) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let mut current = service
        .limits(guild)
        .await
        .map_err(|_| "AI limits unavailable")?;
    let changed = user_daily.is_some()
        || guild_daily.is_some()
        || output_tokens.is_some()
        || concurrency.is_some();
    if let Some(value) = user_daily {
        if !(1..=1_000).contains(&value) {
            return respond(&ctx, "Member daily limit must be 1–1000.").await;
        }
        current.user_daily = value;
    }
    if let Some(value) = guild_daily {
        if !(1..=100_000).contains(&value) {
            return respond(&ctx, "Server daily limit must be 1–100000.").await;
        }
        current.guild_daily = value;
    }
    if let Some(value) = output_tokens {
        if !(32..=4_096).contains(&value) {
            return respond(&ctx, "Output token limit must be 32–4096.").await;
        }
        current.output_tokens = value;
    }
    if let Some(value) = concurrency {
        if !(1..=16).contains(&value) {
            return respond(&ctx, "Concurrent limit must be 1–16.").await;
        }
        current.concurrency = value as u32;
    }
    if changed {
        service
            .save_limits(guild, &current)
            .await
            .map_err(|_| "AI limit update failed")?;
    }
    respond(&ctx, &format!("AI limits: {} requests/member/day, {} requests/server/day, {} output tokens, {} concurrent. Allowed channels: {}. Allowed roles: {}.",
        current.user_daily, current.guild_daily, current.output_tokens, current.concurrency,
        if current.channels.is_empty() { "all" } else { "selected" }, if current.roles.is_empty() { "all" } else { "selected" })).await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn channel(
    ctx: CustomContext<'_>,
    #[description = "Channel to allow; omit to clear channel restrictions"] channel: Option<
        GuildChannel,
    >,
) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let mut limits = service
        .limits(guild)
        .await
        .map_err(|_| "AI limits unavailable")?;
    if let Some(channel) = channel {
        if channel.guild_id.get() != guild {
            return respond(&ctx, "Choose a channel in this server.").await;
        }
        if !limits.channels.contains(&channel.id.get()) {
            limits.channels.push(channel.id.get());
        }
    } else {
        limits.channels.clear();
    }
    service
        .save_limits(guild, &limits)
        .await
        .map_err(|_| "AI channel update failed")?;
    respond(
        &ctx,
        if limits.channels.is_empty() {
            "AI channel restrictions cleared."
        } else {
            "Channel added to the AI allowlist."
        },
    )
    .await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn role(
    ctx: CustomContext<'_>,
    #[description = "Role to allow; omit to clear role restrictions"] role: Option<Role>,
) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?.get();
    let mut limits = service
        .limits(guild)
        .await
        .map_err(|_| "AI limits unavailable")?;
    if let Some(role) = role {
        if !limits.roles.contains(&role.id.get()) {
            limits.roles.push(role.id.get());
        }
    } else {
        limits.roles.clear();
    }
    service
        .save_limits(guild, &limits)
        .await
        .map_err(|_| "AI role update failed")?;
    respond(
        &ctx,
        if limits.roles.is_empty() {
            "AI role restrictions cleared."
        } else {
            "Role added to the AI allowlist."
        },
    )
    .await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn disconnect(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner(&ctx).await? {
        return Ok(());
    }
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    service
        .disconnect(ctx.guild_id().ok_or("Guild required")?.get())
        .await
        .map_err(|_| "AI disconnect failed")?;
    respond(&ctx, "The server's stored AI key was deleted and new BYOK requests are disabled. To revoke the key completely, also revoke it at your AI provider. Minimal usage metadata remains for up to 90 days.").await
}

#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn ask(
    ctx: CustomContext<'_>,
    #[description = "Question for this server's connected AI provider"] question: String,
) -> Result<(), Error> {
    let Some(service) = service(&ctx) else {
        return respond(&ctx, "Secure AI setup is not configured yet.").await;
    };
    let guild = ctx.guild_id().ok_or("Guild required")?;
    let member = guild.member(ctx.http(), ctx.author().id).await?;
    ctx.defer().await?;
    let roles = member.roles.iter().map(|id| id.get()).collect::<Vec<_>>();
    let answer = service
        .ask(
            guild.get(),
            ctx.author().id.get(),
            ctx.channel_id().get(),
            &roles,
            question.trim(),
        )
        .await;
    let text = match answer {
        Ok(response) => response.text.chars().take(1_900).collect::<String>(),
        Err(error) => error_message(error).to_string(),
    };
    ctx.send(
        CreateReply::default().content(text).allowed_mentions(
            CreateAllowedMentions::new()
                .empty_users()
                .empty_roles()
                .everyone(false),
        ),
    )
    .await?;
    Ok(())
}

pub async fn reply_to_mention(
    ctx: &serenity::Context,
    message: &Message,
    service: &ByokService,
) -> Result<(), Error> {
    let guild = message.guild_id.ok_or("Guild required")?;
    let member = guild.member(&ctx.http, message.author.id).await?;
    let roles = member.roles.iter().map(|id| id.get()).collect::<Vec<_>>();
    let bot_id = ctx.cache.current_user().id;
    let question = message
        .content
        .replace(&format!("<@{}>", bot_id), "")
        .replace(&format!("<@!{}>", bot_id), "");
    let answer = service
        .ask(
            guild.get(),
            message.author.id.get(),
            message.channel_id.get(),
            &roles,
            question.trim(),
        )
        .await;
    let text = match answer {
        Ok(response) => response.text.chars().take(1_900).collect::<String>(),
        Err(error) => error_message(error).to_string(),
    };
    message
        .channel_id
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(text)
                .reference_message(message)
                .allowed_mentions(
                    CreateAllowedMentions::new()
                        .empty_users()
                        .empty_roles()
                        .everyone(false),
                ),
        )
        .await?;
    Ok(())
}
