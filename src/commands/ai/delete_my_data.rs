use crate::{CustomContext, Error, database};
use poise::{
    CreateReply,
    serenity_prelude::{Colour, CreateEmbed, Timestamp},
};

#[poise::command(
    slash_command,
    description_localized("en-US", "Delete your stored OmniCore AI data."),
    category = "AI"
)]
pub(crate) async fn delete_my_data(ctx: CustomContext<'_>) -> Result<(), Error> {
    //! Deletes AI records attributable to the account invoking this command.
    database::delete_user_data(&ctx.author().id.to_string()).await?;
    ctx.send(CreateReply::default()
        .embed(CreateEmbed::new()
            .title("AI data deletion")
            .description("Your stored AI prompts and linked AI responses have been removed. Older tool output and text mentioning you may require manual review; use /info to find an OmniCore bot owner for a full deletion request.")
            .timestamp(Timestamp::now())
            .color(Colour::from_rgb(0, 255, 0)))
        .ephemeral(true)).await?;
    Ok(())
}
