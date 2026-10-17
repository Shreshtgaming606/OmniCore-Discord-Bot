use super::super::build_message_reply;
use crate::{CustomContext, Error, database};
use poise::serenity_prelude::Colour;

#[poise::command(
    slash_command,
    prefix_command,
    description_localized("en-US", "Deletes all memories from the bot."),
    guild_only,
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR",
    broadcast_typing,
    category = "AI"
)]
pub(crate) async fn delete_memory(ctx: CustomContext<'_>) -> Result<(), Error> {
    //! Deletes all memories from the bot
    database::delete_guild_ai_history(&ctx.guild_id().unwrap().to_string()).await?;

    ctx.send(build_message_reply(
        "Deleted Memories",
        "Successfully deleted all memories (for this server) from the bot.",
        Colour::from_rgb(0, 255, 0),
        false,
    ))
    .await?;

    return Ok(());
}
