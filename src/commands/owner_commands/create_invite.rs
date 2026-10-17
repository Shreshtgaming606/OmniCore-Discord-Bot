use super::autocomplete_guild;
use crate::{CustomContext, Error};
use poise::serenity_prelude::GuildId;
use poise::serenity_prelude::{ChannelType, CreateInvite};

#[poise::command(
    slash_command,
    description_localized("en-US", "Creates an invite for the specified server."),
    dm_only,
    owners_only,
    category = "Bot Owner Utilities"
)]
pub async fn create_invite(
    ctx: CustomContext<'_>,
    #[description = "Server to create an invite for (search by name or ID)"]
    #[autocomplete = "autocomplete_guild"]
    guild: String,
) -> Result<(), Error> {
    //! Creates an invite for the specified server.
    let typing = poise::serenity_prelude::Typing::start(
        ctx.serenity_context().http.clone(),
        ctx.channel_id(),
    );

    let guild_id = match guild.trim().parse::<u64>() {
        Ok(id) => GuildId::new(id),
        Err(_) => {
            ctx.say(
                ":x: Invalid guild — please select a server from the autocomplete suggestions.",
            )
            .await?;
            typing.stop();
            return Ok(());
        }
    };

    // Make sure the bot is actually in this guild.
    let in_guild = ctx
        .http()
        .get_guilds(None, None)
        .await?
        .into_iter()
        .any(|g| g.id == guild_id);

    if !in_guild {
        ctx.say(":x: This bot is not in that server.").await?;
        typing.stop();
        return Ok(());
    }

    // Just use the first text channel you can get, invites will still let you join
    let channels = guild_id.channels(ctx.http()).await?;

    let first_channel = match channels.values().find(|c| c.kind == ChannelType::Text) {
        Some(channel) => channel,
        None => {
            ctx.say("No channels found in this guild.").await?;
            typing.stop();
            return Ok(());
        }
    };

    let invite = match first_channel
        .create_invite(
            ctx.http(),
            CreateInvite::default()
                .max_age(3600) // 1 hour
                .max_uses(1) // 1 use
                .unique(true),
        )
        .await
    {
        Ok(invite) => invite,
        Err(error) => {
            ctx.say(&format!("Failed to create invite: \n ```{:?}```", error))
                .await?;
            typing.stop();
            return Ok(());
        }
    };

    let invite_url = invite.url();

    ctx.say(format!("Invite created: {}", invite_url)).await?;
    typing.stop();

    Ok(())
}
