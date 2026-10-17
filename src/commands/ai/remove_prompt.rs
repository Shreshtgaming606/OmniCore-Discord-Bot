use super::super::build_message_reply;
use crate::{CustomContext, Error, database::get_collection, setup_guild};
use mongodb::bson::doc;
use poise::serenity_prelude::Colour;

#[poise::command(
    slash_command,
    prefix_command,
    description_localized("en-US", "Removes the current system prompt for the bot."),
    guild_only,
    broadcast_typing,
    category = "AI"
)]
pub(crate) async fn remove_prompt(ctx: CustomContext<'_>) -> Result<(), Error> {
    //! Removes the current system prompt for the bot
    let per_guild_settings_col =
        get_collection("per_guild_settings").expect("Failed to load per_guild_settings collection");

    if per_guild_settings_col
        .find_one(doc! {"guild_id": ctx.guild_id().unwrap().to_string()})
        .await?
        .is_none()
    {
        setup_guild(ctx.guild_id().unwrap()).await;
    }

    let _ = per_guild_settings_col
        .update_one(
            doc! {"guild_id": ctx.guild_id().unwrap().to_string()},
            doc! {"$unset": {"ai_prompt": ""}},
        )
        .await;

    ctx.send(build_message_reply(
        "Removed Prompt",
        "Successfully removed the AI prompt for this server.",
        Colour::from_rgb(0, 255, 0),
        false,
    ))
    .await?;

    return Ok(());
}
