use crate::{CustomContext, Error};
use mongodb::bson::doc;
use poise::{
    CreateReply,
    serenity_prelude::{Colour, CreateAllowedMentions, CreateEmbed, Timestamp},
};

#[poise::command(
    slash_command,
    prefix_command,
    description_localized(
        "en-US",
        "Lists all servers the bot is in, along with their member counts, owner, image, and ID."
    ),
    dm_only,
    owners_only,
    broadcast_typing,
    category = "Bot Owner Utilities"
)]
pub async fn all_servers(ctx: CustomContext<'_>) -> Result<(), Error> {
    //! Lists all servers the bot is in, along with their member counts, owner, image, and ID.
    ctx.defer().await?;

    let mut guilds = ctx.http().get_guilds(None, None).await?;
    guilds.sort_by(|a, b| a.name.cmp(&b.name));

    let http = ctx.http();

    // Get server info
    let mut server_info: Vec<String> = vec![];
    for guild in guilds {
        let partial_guild = guild.id.to_partial_guild_with_counts(http).await?;

        let members = partial_guild.approximate_member_count.unwrap_or(0);

        let guild_owner_id = partial_guild.owner_id;

        let image = partial_guild.icon_url().unwrap_or_default();

        let image = match image.is_empty() {
            true => "https://cdn.discordapp.com/embed/avatars/0.png",
            false => &*image,
        };

        server_info.push(format!("## **{}**\n - Server ID: {}\n - Members: {}\n - Owner: <@{}>\n - Image: [Server Image]({})", guild.name, guild.id, members, guild_owner_id, image));
    }

    // Build the embed
    let mut parts: Vec<String> = vec![];
    let mut current = String::new();

    // break up the embeds if it exceeds 4096 characters
    for entry in &server_info {
        // +1 for the "\n" that will join this entry to current
        let added_len = entry.len() + if current.is_empty() { 0 } else { 1 };

        if current.len() + added_len > 4096 {
            parts.push(std::mem::take(&mut current));
        }

        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(entry);
    }

    if !current.is_empty() {
        parts.push(current);
    }

    let first_embed = CreateEmbed::new()
        .title("All Servers")
        .description(parts.get(0).unwrap().to_string())
        .color(Colour::from_rgb(88, 101, 242))
        .timestamp(Timestamp::now());

    if parts.len() > 1 {
        let mut embeds = vec![first_embed];
        for part in parts.iter().skip(1) {
            embeds.push(CreateEmbed::new().description(part.to_string()));
        }
        // Send all embeds
        for embed in embeds {
            ctx.send(
                CreateReply::default()
                    .embed(embed)
                    .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles()),
            )
            .await?;
        }
    } else {
        // Send the first embed
        ctx.send(
            CreateReply::default()
                .embed(first_embed)
                .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles()),
        )
        .await?;
    }

    return Ok(());
}
