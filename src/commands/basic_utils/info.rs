// info command
use crate::START_TIME;
use crate::config::BOT_OWNERS;
use crate::database::get_collection;
use crate::{CustomContext, Error, get_guild_name};
use futures::stream::{self, StreamExt};
use mongodb::bson::{doc, Document};
use poise::CreateReply;
use poise::serenity_prelude::Colour;
use poise::serenity_prelude::{
    CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, GuildId, Timestamp,
};
use reqwest;
use std::collections::HashMap;
use futures::TryStreamExt;
use mongodb::Collection;

const CONCURRENCY: usize = 5;
async fn get_contributors() -> Result<HashMap<String, String>, reqwest::Error> {
    let url = "https://api.github.com/repos/Shreshtgaming606/OmniCore-Discord-Bot/contributors";
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "firefox")
        .send()
        .await?;

    let contributors: Vec<serde_json::Value> = response.json().await?;
    let mut contributors_map = HashMap::new();

    for contributor in contributors {
        if let (Some(login), Some(html_url)) = (
            contributor.get("login").and_then(|v| v.as_str()),
            contributor.get("html_url").and_then(|v| v.as_str()),
        ) {
            contributors_map.insert(login.to_string(), html_url.to_string());
        }
    }

    Ok(contributors_map)
}

async fn get_channel_and_member_counts(ctx: CustomContext<'_>) -> (usize, usize) {
    let Ok(guilds) = ctx.http().get_guilds(None, None).await else {
        return (0, 0);
    };

    let http = ctx.http();

    let results: Vec<(usize, usize)> = stream::iter(guilds)
        .map(|guild| {
            let http = http;
            async move {
                let channels = http
                    .get_channels(guild.id)
                    .await
                    .map(|c| c.len())
                    .unwrap_or(0);

                let members = http
                    .get_guild_with_counts(guild.id)
                    .await
                    .ok()
                    .and_then(|g| g.approximate_member_count)
                    .unwrap_or(0) as usize;

                (channels, members)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let total_channels: usize = results.iter().map(|(c, _)| c).sum();
    let total_members: usize = results.iter().map(|(_, m)| m).sum();

    (total_channels, total_members)
}

async fn total_messages(coll: &Collection<Document>) -> mongodb::error::Result<i64> {
    let pipeline = vec![
        doc! {
            "$group": {
                "_id": null,
                "total": {
                    "$sum": {
                        // $ifNull protects for docs that dont have messages
                        // $isArray protects against non-array values
                        "$cond": [
                            { "$isArray": "$messages" },
                            { "$size": "$messages" },
                            0
                        ]
                    }
                }
            }
        },
    ];

    let mut cursor = coll.aggregate(pipeline).await?;
    let total = match cursor.try_next().await? {
        Some(d) => d.get_i32("total").map(i64::from)
            .or_else(|_| d.get_i64("total"))
            .unwrap_or(0),
        None => 0, // empty collection
    };
    Ok(total)
}

#[poise::command(
    slash_command,
    prefix_command,
    description_localized("en-US", "Shows some information about the bot."),
    broadcast_typing,
    category = "Utility"
)]
pub(crate) async fn info(ctx: CustomContext<'_>) -> Result<(), Error> {
    //! Shows some information about the bot.
    //!
    //! Also shows contributors, fetched from
    //! https://api.github.com/repos/Shreshtgaming606/OmniCore-Discord-Bot/contributors
    ctx.defer().await?;

    let prefix =
        crate::commands::basic_utils::prefix::get_prefix(ctx.guild_id().unwrap_or(GuildId::new(1)))
            .await;

    let start_time = START_TIME.get().unwrap();

    let member_count_and_channel_count = get_channel_and_member_counts(ctx.clone()).await;

    let owners = BOT_OWNERS
        .get()
        .unwrap()
        .clone()
        .into_iter()
        .map(|id| format!("<@{}>", id))
        .collect::<Vec<String>>()
        .join(", ");

    let desc = format!(
        "\
- Server: {}
- Shard: `{}`
- Bot Owner(s): {}
- Bot Prefix for this server: `{}`
- GitHub: https://github.com/Shreshtgaming606/OmniCore-Discord-Bot/
- Shard Started <t:{}:R>
- In `{}` server(s)
- In `{}` channel(s)
- `{}` (estimated) Total Member(s) across all server(s)
        ",
        get_guild_name(&ctx).await,
        ctx.serenity_context().shard_id.0,
        owners,
        prefix,
        start_time.timestamp(),
        ctx.http().get_guilds(None, None).await?.len(),
        member_count_and_channel_count.0,
        member_count_and_channel_count.1
    );

    let mut ai_approved = false;
    let mut messages_processed = 0;

    let messages_col =
        get_collection("messages").expect("Failed to load messages collection");

    if ctx.guild_id().is_some() {
        let per_guild_settings_col = get_collection("per_guild_settings")
            .expect("Failed to load per_guild_settings collection");

        let guild_settings = per_guild_settings_col
            .find_one(doc! { "guild_id": ctx.guild_id().map(|id| id.get().to_string()) })
            .await?;

        if guild_settings.is_none() {
            crate::setup_guild(ctx.guild_id().unwrap_or(GuildId::new(1))).await;
        } else {
            let guild_settings = guild_settings.unwrap();

            ai_approved = guild_settings
                .get("ai_approved")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let guild_messages = messages_col
                .find_one(doc! { "guild_id": ctx.guild_id().map(|id| id.get().to_string()) })
                .await?;
            if guild_messages.is_none() {
                messages_processed = 0;
            } else {
                let guild_messages = guild_messages.unwrap();
                messages_processed = guild_messages.get_array("messages").unwrap().iter().len();
            }
        }
    }

    let total_messages = total_messages(&messages_col).await?;

    let ai_desc = format!(
        "\
- AI Approved: `{}`
- AI Model: `{}`
- Messages processed (for this server): `{}`
- Messages processed (global): `{}`
        ",
        ai_approved,
        crate::config::OLLAMA_MODEL.get().unwrap(),
        messages_processed,
        total_messages
    );

    let contributors = get_contributors().await?;

    let list = contributors
        .iter()
        .map(|(key, value)| format!("- [{}]({})", key, value))
        .collect::<Vec<_>>()
        .join("\n");

    let contributors_desc = format!(
        "\n\n{}\n-# These people have contributed to the development of OmniCore's Discord Bot",
        list
    );

    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .description(desc)
                .title("Bot Information")
                .color(Colour::from_rgb(88, 101, 242)),
        )
        .embed(
            CreateEmbed::new()
                .title("AI Information")
                .description(ai_desc)
                .timestamp(Timestamp::now())
                .color(Colour::from_rgb(88, 101, 242)),
        )
        .embed(
            CreateEmbed::new()
                .title("Contributors")
                .description(contributors_desc)
                .timestamp(Timestamp::now())
                .color(Colour::from_rgb(0, 255, 0)),
        )
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new_link("https://unloaded.steampirate.life")
                .label("Visit the OmniCore website"),
        ])])
        .reply(true)
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles());

    ctx.send(res).await?;
    Ok(())
}
