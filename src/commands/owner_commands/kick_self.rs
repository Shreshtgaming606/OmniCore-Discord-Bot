use crate::commands::build_message_reply;
use crate::commands::owner_commands::autocomplete_guild;
use crate::{CustomContext, Error};
use mongodb::bson::doc;
use poise::CreateReply;
use poise::serenity_prelude::{
    Colour, ComponentInteractionCollector, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateEmbed, GuildId, Timestamp,
};
use std::time::Duration;

#[poise::command(
    slash_command,
    description_localized("en-US", "Kicks the bot from the specified server."),
    dm_only,
    owners_only,
    category = "Bot Owner Utilities"
)]
pub async fn kick_self(
    ctx: CustomContext<'_>,
    #[description = "Server to kick bot from (search by name or ID)"]
    #[autocomplete = "autocomplete_guild"]
    guild: String,
) -> Result<(), Error> {
    //! Kicks/leaves the bot from the specified server.
    let typing = poise::serenity_prelude::Typing::start(
        ctx.serenity_context().http.clone(),
        ctx.channel_id(),
    );

    let guild_id = match guild.trim().parse::<u64>() {
        Ok(id) => GuildId::new(id),
        Err(_) => {
            let res = build_message_reply(
                ":x: Invalid guild",
                "Invalid Guild, please select a server from the autocomplete suggestions.",
                Colour::from_rgb(255, 0, 0),
                false,
            );
            ctx.send(res).await?;
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
        let res = build_message_reply(
            ":x: Not in guild",
            "This bot is not in that server.",
            Colour::from_rgb(255, 0, 0),
            false,
        );
        ctx.send(res).await?;
        typing.stop();
        return Ok(());
    }

    let guild_name = guild_id
        .name(ctx.cache())
        .unwrap_or_else(|| guild_id.to_string());

    // Build confirmation buttons
    let confirm_id = format!("kick_self_confirm_{}", ctx.id());
    let cancel_id = format!("kick_self_cancel_{}", ctx.id());

    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(&confirm_id)
            .label("Confirm")
            .style(poise::serenity_prelude::ButtonStyle::Danger),
        CreateButton::new(&cancel_id)
            .label("Cancel")
            .style(poise::serenity_prelude::ButtonStyle::Secondary),
    ])];

    let confirm_reply = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .description(format!(
                    "Are you sure you want to leave **{}**? This cannot be undone.",
                    guild_name
                ))
                .title(":warning: Confirm Server Leave")
                .color(Colour::from_rgb(255, 165, 0)),
        )
        .components(components)
        .reply(true)
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles());

    let sent_msg = ctx.send(confirm_reply).await?;
    typing.stop();

    // Wait for a button interaction from the command invoker
    let interaction = ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .timeout(Duration::from_secs(30))
        .filter(move |mci| mci.data.custom_id == confirm_id || mci.data.custom_id == cancel_id)
        .await;

    match interaction {
        Some(mci) => {
            // Acknowledge the button press
            mci.create_response(
                ctx.http(),
                poise::serenity_prelude::CreateInteractionResponse::Acknowledge,
            )
            .await?;

            if mci.data.custom_id.starts_with("kick_self_cancel") {
                sent_msg
                    .edit(
                        ctx,
                        CreateReply::default()
                            .embed(
                                CreateEmbed::new()
                                    .description("Cancelled, the bot will remain in the server.")
                                    .title(":x: Cancelled")
                                    .color(Colour::from_rgb(128, 128, 128)),
                            )
                            .components(vec![]),
                    )
                    .await?;
                return Ok(());
            }
        }
        None => {
            // Timed out
            sent_msg
                .edit(
                    ctx,
                    CreateReply::default()
                        .embed(
                            CreateEmbed::new()
                                .description("Confirmation timed out, no action taken.")
                                .title(":x: Timed Out")
                                .color(Colour::from_rgb(128, 128, 128)),
                        )
                        .components(vec![]),
                )
                .await?;
            return Ok(());
        }
    }

    // Kick the bot from the guild
    match guild_id.leave(&ctx.http()).await {
        Ok(_) => {
            sent_msg
                .edit(
                    ctx,
                    CreateReply::default()
                        .embed(
                            CreateEmbed::new()
                                .description(format!(
                                    "Successfully left the server: **{}**",
                                    guild_name
                                ))
                                .title(":white_check_mark: Left Server")
                                .timestamp(Timestamp::now())
                                .color(Colour::from_rgb(0, 255, 0)),
                        )
                        .components(vec![]),
                )
                .await?;
        }
        Err(e) => {
            log::error!("Error in leaving server: {}", e);
            log::error!("Error Details: {:#?}", e);
            return Err(e.into());
        }
    }

    Ok(())
}
