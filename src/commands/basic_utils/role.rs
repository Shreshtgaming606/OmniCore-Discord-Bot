// just a subcommand thingy

use super::allroles::roles_all;
use crate::{CustomContext, Error};

#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    description_localized("en-US", "Lists all roles in the server."),
    broadcast_typing,
    subcommand_required,
    subcommands("roles_all"),
    aliases("roles"),
    category = "Utility"
)]
pub async fn role(_: CustomContext<'_>) -> Result<(), Error> {
    Ok(())
}
