use crate::{
    CustomContext, Error,
    automod::config::{AutoModConfig, Sensitivity, parse_owner_duration},
};
use poise::{
    CreateReply,
    serenity_prelude::{GuildChannel, PermissionOverwriteType, Permissions},
};

#[poise::command(
    slash_command,
    guild_only,
    subcommands(
        "status",
        "enable",
        "disable",
        "settings",
        "sensitivity",
        "timeout",
        "warnings",
        "timeouts",
        "notifications",
        "diagnostics",
        "log_channel"
    ),
    subcommand_required,
    category = "Moderation"
)]
pub async fn automod(_ctx: CustomContext<'_>) -> Result<(), Error> {
    Ok(())
}

fn is_owner(owner: u64, caller: u64) -> bool {
    owner == caller
}

async fn owner_check(ctx: &CustomContext<'_>) -> Result<bool, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(false);
    };
    let guild = guild_id.to_partial_guild(ctx.http()).await?;
    if is_owner(guild.owner_id.get(), ctx.author().id.get()) {
        return Ok(true);
    }
    respond(ctx, "Only this server's owner can change AutoMod settings.").await?;
    Ok(false)
}

async fn respond(ctx: &CustomContext<'_>, text: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

async fn get(ctx: &CustomContext<'_>) -> Result<AutoModConfig, Error> {
    Ok(ctx
        .data()
        .automod
        .load_config(ctx.guild_id().ok_or("Guild required")?)
        .await?)
}

async fn save(ctx: &CustomContext<'_>, config: &AutoModConfig) -> Result<(), Error> {
    ctx.data()
        .automod
        .save_config(ctx.guild_id().ok_or("Guild required")?, config)
        .await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn status(ctx: CustomContext<'_>) -> Result<(), Error> {
    show_settings(&ctx).await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn settings(ctx: CustomContext<'_>) -> Result<(), Error> {
    show_settings(&ctx).await
}

async fn show_settings(ctx: &CustomContext<'_>) -> Result<(), Error> {
    let c = get(ctx).await?;
    let log = c
        .log_channel_id
        .map_or("None".to_string(), |id| format!("<#{id}>"));
    respond(ctx, &format!("**OmniCore Auto Moderation**\nStatus: {}\nSensitivity: {}\nWarnings: {}\nAutomatic timeouts: {}\nModerate: {} minutes\nSerious: {} minutes\nSevere: {} hours\nExtreme: {} hours\nMaximum: 2 days\nOwner notifications: {}\nOperator diagnostic alerts: {}\nPrivate log channel: {}",
        if c.enabled { "Enabled" } else { "Disabled" }, c.sensitivity.name(),
        if c.warnings_enabled { "Enabled" } else { "Disabled" },
        if c.timeouts_enabled { "Enabled" } else { "Disabled" },
        c.moderate_timeout.as_secs()/60, c.serious_timeout.as_secs()/60,
        c.severe_timeout.as_secs()/3600, c.extreme_timeout.as_secs()/3600,
        if c.notify_owner { "Enabled" } else { "Disabled" },
        if c.notify_developers { "Enabled" } else { "Disabled" }, log)).await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn enable(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let mut c = get(&ctx).await?;
    c.enabled = true;
    save(&ctx, &c).await?;
    respond(&ctx, "AutoMod enabled.").await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn disable(ctx: CustomContext<'_>) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let mut c = get(&ctx).await?;
    c.enabled = false;
    save(&ctx, &c).await?;
    respond(&ctx, "AutoMod disabled.").await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn sensitivity(
    ctx: CustomContext<'_>,
    #[description = "low, normal, or high"] level: String,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let Some(level) = Sensitivity::parse(&level) else {
        return respond(&ctx, "Choose low, normal, or high.").await;
    };
    let mut c = get(&ctx).await?;
    c.sensitivity = level;
    save(&ctx, &c).await?;
    respond(
        &ctx,
        &format!("AutoMod sensitivity set to {}.", level.name()),
    )
    .await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn timeout(
    ctx: CustomContext<'_>,
    #[description = "moderate, serious, severe, or extreme"] level: String,
    #[description = "Duration such as 10m, 1h, or 2d"] duration: String,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let duration = match parse_owner_duration(&duration) {
        Ok(value) => value,
        Err(error) => return respond(&ctx, error).await,
    };
    let mut c = get(&ctx).await?;
    match level.to_ascii_lowercase().as_str() {
        "moderate" => c.moderate_timeout = duration,
        "serious" => c.serious_timeout = duration,
        "severe" => c.severe_timeout = duration,
        "extreme" => c.extreme_timeout = duration,
        _ => return respond(&ctx, "Choose moderate, serious, severe, or extreme.").await,
    }
    save(&ctx, &c).await?;
    respond(&ctx, "AutoMod timeout duration updated.").await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn warnings(
    ctx: CustomContext<'_>,
    #[description = "Send warnings to users"] enabled: bool,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let mut c = get(&ctx).await?;
    c.warnings_enabled = enabled;
    save(&ctx, &c).await?;
    respond(&ctx, "Warning setting updated.").await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn timeouts(
    ctx: CustomContext<'_>,
    #[description = "Apply automatic timeouts"] enabled: bool,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let mut c = get(&ctx).await?;
    c.timeouts_enabled = enabled;
    save(&ctx, &c).await?;
    respond(&ctx, "Automatic timeout setting updated.").await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn notifications(
    ctx: CustomContext<'_>,
    #[description = "Notify the server owner of actions"] enabled: bool,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let mut c = get(&ctx).await?;
    c.notify_owner = enabled;
    save(&ctx, &c).await?;
    respond(&ctx, "Owner notification setting updated.").await
}

#[poise::command(slash_command, guild_only, category = "Moderation")]
pub async fn diagnostics(
    ctx: CustomContext<'_>,
    #[description = "Send metadata-only AutoMod alerts to the two OmniCore operators"]
    enabled: bool,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    let mut c = get(&ctx).await?;
    c.notify_developers = enabled;
    save(&ctx, &c).await?;
    respond(
        &ctx,
        if enabled {
            "Operator diagnostic alerts enabled. They exclude message text and member identity."
        } else {
            "Operator diagnostic alerts disabled for this server."
        },
    )
    .await
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "log-channel",
    category = "Moderation"
)]
pub async fn log_channel(
    ctx: CustomContext<'_>,
    #[description = "Private fallback channel; omit to clear"] channel: Option<GuildChannel>,
) -> Result<(), Error> {
    if !owner_check(&ctx).await? {
        return Ok(());
    }
    if let Some(ref channel) = channel {
        let guild_id = ctx.guild_id().ok_or("Guild required")?;
        let private = channel.guild_id == guild_id
            && channel.permission_overwrites.iter().any(|overwrite| {
                overwrite.kind == PermissionOverwriteType::Role(guild_id.everyone_role())
                    && overwrite.deny.contains(Permissions::VIEW_CHANNEL)
            });
        if !private {
            return respond(
                &ctx,
                "The channel must explicitly deny View Channel to @everyone.",
            )
            .await;
        }
    }
    let mut c = get(&ctx).await?;
    c.log_channel_id = channel.map(|channel| channel.id.get());
    save(&ctx, &c).await?;
    respond(&ctx, "Private fallback log channel updated.").await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guild_owner_only() {
        assert!(is_owner(1, 1));
        assert!(!is_owner(1, 2));
    }
}
