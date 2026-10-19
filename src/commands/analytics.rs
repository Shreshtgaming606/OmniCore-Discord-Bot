use crate::{CustomContext, Error, config, database, moderation_events};
use futures::TryStreamExt;
use mongodb::bson::{DateTime, Document, doc};
use poise::{
    CreateReply,
    serenity_prelude::{Colour, CreateEmbed},
};
use std::collections::BTreeMap;

#[poise::command(
    slash_command,
    subcommands("moderation", "global", "guild"),
    subcommand_required,
    category = "Analytics"
)]
pub async fn analytics(_ctx: CustomContext<'_>) -> Result<(), Error> {
    Ok(())
}

fn days(input: Option<&str>) -> Option<i64> {
    match input.unwrap_or("7d") {
        "7d" => Some(7),
        "30d" => Some(30),
        "90d" => Some(90),
        _ => None,
    }
}

fn is_developer(id: u64) -> bool {
    developer_match(config::DEVELOPER_IDS.get().map(Vec::as_slice), id)
}

fn developer_match(ids: Option<&[u64]>, id: u64) -> bool {
    ids.is_some_and(|ids| ids.contains(&id))
}

async fn reject(ctx: &CustomContext<'_>, text: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, category = "Analytics")]
pub async fn moderation(
    ctx: CustomContext<'_>,
    #[description = "7d, 30d, or 90d"] period: Option<String>,
) -> Result<(), Error> {
    let Some(period) = days(period.as_deref()) else {
        return reject(&ctx, "Choose 7d, 30d, or 90d.").await;
    };
    let guild_id = ctx.guild_id().ok_or("Guild required")?;
    let guild = guild_id.to_partial_guild(ctx.http()).await?;
    if guild.owner_id != ctx.author().id {
        return reject(
            &ctx,
            "Only this server's owner can view its moderation analytics.",
        )
        .await;
    }
    ctx.defer_ephemeral().await?;
    let enabled = ctx.data().automod.load_config(guild_id).await?.enabled;
    show(
        &ctx,
        Some(guild_id.to_string()),
        period,
        &format!("{} moderation", guild.name),
        Some(enabled),
    )
    .await
}

#[poise::command(slash_command, category = "Analytics")]
pub async fn global(
    ctx: CustomContext<'_>,
    #[description = "7d, 30d, or 90d"] period: Option<String>,
) -> Result<(), Error> {
    if !is_developer(ctx.author().id.get()) {
        return reject(&ctx, "Only OmniCore operators can view global analytics.").await;
    }
    let Some(period) = days(period.as_deref()) else {
        return reject(&ctx, "Choose 7d, 30d, or 90d.").await;
    };
    log::info!(
        "Operator global moderation analytics accessed by {}",
        ctx.author().id
    );
    ctx.defer_ephemeral().await?;
    show(&ctx, None, period, "OmniCore global moderation", None).await
}

#[poise::command(slash_command, category = "Analytics")]
pub async fn guild(
    ctx: CustomContext<'_>,
    #[description = "Numeric Discord guild ID"] guild_id: String,
    #[description = "7d, 30d, or 90d"] period: Option<String>,
) -> Result<(), Error> {
    if !is_developer(ctx.author().id.get()) {
        return reject(
            &ctx,
            "Only OmniCore operators can view per-guild diagnostics.",
        )
        .await;
    }
    let Some(period) = days(period.as_deref()) else {
        return reject(&ctx, "Choose 7d, 30d, or 90d.").await;
    };
    let Ok(parsed) = guild_id.parse::<u64>() else {
        return reject(&ctx, "Enter a numeric guild ID.").await;
    };
    let guild = poise::serenity_prelude::GuildId::new(parsed)
        .to_partial_guild(ctx.http())
        .await?;
    log::info!(
        "Operator per-guild moderation analytics accessed by {} for {}",
        ctx.author().id,
        parsed
    );
    ctx.defer_ephemeral().await?;
    show(
        &ctx,
        Some(parsed.to_string()),
        period,
        &format!("{} moderation", guild.name),
        None,
    )
    .await
}

async fn show(
    ctx: &CustomContext<'_>,
    guild_id: Option<String>,
    period: i64,
    title: &str,
    enabled: Option<bool>,
) -> Result<(), Error> {
    let since = DateTime::from_millis(DateTime::now().timestamp_millis() - period * 86_400_000);
    let mut condition = doc! {"occurredAt": {"$gte": since}};
    if let Some(ref guild_id) = guild_id {
        condition.insert("guild_id", guild_id);
    }
    let pipeline = vec![
        doc! {"$match": condition},
        doc! {"$group": {"_id": {
            "action": "$action", "category": "$category",
            "day": {"$dateToString": {"format": "%Y-%m-%d", "date": "$occurredAt"}}
        }, "count": {"$sum": 1}, "duration": {"$sum": {"$ifNull": ["$duration_seconds", 0]}}}},
        doc! {"$sort": {"_id.day": 1}},
    ];
    let mut cursor = database::get_collection(moderation_events::EVENTS)?
        .aggregate(pipeline)
        .await?;
    let mut actions: BTreeMap<String, i64> = BTreeMap::new();
    let mut categories: BTreeMap<String, i64> = BTreeMap::new();
    let mut daily: BTreeMap<String, i64> = BTreeMap::new();
    let mut timeout_seconds = 0_i64;
    while let Some(row) = cursor.try_next().await? {
        let key: &Document = row.get_document("_id")?;
        let count = row
            .get_i32("count")
            .map(i64::from)
            .or_else(|_| row.get_i64("count"))?;
        *actions
            .entry(key.get_str("action")?.to_string())
            .or_default() += count;
        *categories
            .entry(key.get_str("category")?.to_string())
            .or_default() += count;
        *daily.entry(key.get_str("day")?.to_string()).or_default() += count;
        timeout_seconds += row.get_i64("duration").unwrap_or(0);
    }
    let mut summary = format!(
        "Last {period} days\nWarnings: {}\nSuccessful timeouts: {}\nFailed timeout attempts: {}\nTotal timeout duration: {} minutes",
        actions.get("warning").copied().unwrap_or(0),
        actions.get("timeout").copied().unwrap_or(0),
        actions.get("failed_timeout").copied().unwrap_or(0),
        timeout_seconds / 60
    );
    if let Some(enabled) = enabled {
        summary.push_str(&format!(
            "\nAutoMod: {}",
            if enabled { "Enabled" } else { "Disabled" }
        ));
    }
    let category_summary = categories
        .into_iter()
        .map(|(key, count)| format!("{key}: {count}"))
        .collect::<Vec<_>>()
        .join("\n");
    let trend = daily
        .into_iter()
        .rev()
        .take(14)
        .map(|(day, count)| format!("{day}: {count}"))
        .collect::<Vec<_>>()
        .join("\n");
    let embed = CreateEmbed::new()
        .title(title)
        .description(summary)
        .color(Colour::from_rgb(88, 101, 242))
        .field(
            "Categories",
            if category_summary.is_empty() {
                "No activity"
            } else {
                &category_summary
            },
            false,
        )
        .field(
            "Recent daily activity",
            if trend.is_empty() {
                "No activity"
            } else {
                &trend
            },
            false,
        );
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_supported_periods() {
        assert_eq!(days(None), Some(7));
        assert_eq!(days(Some("30d")), Some(30));
        assert_eq!(days(Some("365d")), None);
    }

    #[test]
    fn global_access_requires_exact_developer_id() {
        let ids = [101, 202];
        assert!(developer_match(Some(&ids), ids[0]));
        assert!(developer_match(Some(&ids), ids[1]));
        assert!(!developer_match(Some(&ids), 123));
        assert!(!developer_match(None, ids[0]));
    }
}
