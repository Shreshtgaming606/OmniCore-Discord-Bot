use super::super::build_message_reply;
use crate::{CustomContext, Error, database::get_collection, setup_guild};
use mongodb::bson::doc;
use poise::serenity_prelude::{Colour, GuildId};

#[poise::command(
    slash_command,
    prefix_command,
    description_localized("en-US", "Sets the prefix for the bot"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    broadcast_typing,
    category = "Utility"
)]
pub(crate) async fn set_prefix(
    ctx: CustomContext<'_>,
    #[description = "What you want to set the prefix to"] new_prefix: String,
) -> Result<(), Error> {
    //! Sets the prefix for the bot
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
            doc! {"$set": {"prefix": &new_prefix}},
        )
        .await;

    ctx.send(build_message_reply(
        "Updated Prefix",
        &format!("Successfully updated prefix to `{}`", new_prefix),
        Colour::from_rgb(0, 255, 0),
        false,
    ))
    .await?;

    return Ok(());
}

pub(crate) async fn get_prefix(id: GuildId) -> String {
    let per_guild_settings_col =
        get_collection("per_guild_settings").expect("Failed to load per_guild_settings collection");

    let prefix = per_guild_settings_col
        .find_one(doc! {"guild_id": id.to_string()})
        .await
        .unwrap();

    if prefix.is_none() {
        crate::setup_guild(id).await;
        return "!".to_string();
    }

    let prefix = prefix
        .unwrap()
        .get_str("prefix")
        .unwrap()
        .to_string()
        .replace("\"", "");

    prefix
}
