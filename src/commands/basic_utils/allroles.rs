use crate::{CustomContext, Error};
use poise::CreateReply;
use poise::serenity_prelude::{
    Colour, CreateAllowedMentions, CreateEmbed, CreateEmbedAuthor, Mentionable, Role, Timestamp,
};

const MAX_ROLES_DIGITS_SIZE: usize = 3; // max is 250

#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    description_localized("en-US", "Lists all roles in the server."),
    broadcast_typing,
    category = "Utility",
    rename = "all"
)]
pub async fn roles_all(ctx: CustomContext<'_>) -> Result<(), Error> {
    // First sort the roles
    let mut roles: Vec<Role> = ctx.guild().unwrap().roles.values().cloned().collect();
    roles.sort_by(|a, b| b.position.cmp(&a.position));
    let mut role_list = String::new();
    for role in roles.clone() {
        let position_str = format!("#{:0width$}", role.position, width = MAX_ROLES_DIGITS_SIZE);

        role_list.push_str(&format!("`{}` {}\n", position_str, role.mention()));
    }

    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .author(
                    CreateEmbedAuthor::new(format!("All Roles ({} roles)", roles.len()))
                        .icon_url(ctx.guild().unwrap().icon_url().unwrap_or_default()),
                )
                .description(role_list)
                .timestamp(Timestamp::now())
                .color(Colour::from_rgb(88, 101, 242)),
        )
        .reply(true)
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles());

    ctx.send(res).await?;
    Ok(())
}
