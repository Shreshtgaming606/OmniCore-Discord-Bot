use super::super::get_highest_role_from_member;
use crate::{CustomContext, Error};
use poise::CreateReply;
use poise::serenity_prelude::{
    Colour, CreateAllowedMentions, CreateEmbed, CreateEmbedAuthor, Member, Mentionable, RoleId,
    Timestamp,
};

#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    description_localized("en-US", "Gets the highest role from a member."),
    broadcast_typing,
    category = "Utility"
)]
pub async fn highest_role_from_member(
    ctx: CustomContext<'_>,
    #[description = "Member to get the highest role from"] member: Member,
) -> Result<(), Error> {
    let highest_role = get_highest_role_from_member(&member, ctx).unwrap_or(RoleId::new(1));

    let username = member.clone().user.name;
    let avatar_url = member.user.face();

    let desc = match highest_role.get() {
        1 => "This member has no roles.".to_string(),
        _ => format!(
            "{}'s highest role is {}",
            member.mention(),
            highest_role.mention()
        ),
    };

    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .description(desc)
                .timestamp(Timestamp::now())
                .author(CreateEmbedAuthor::new(username).icon_url(avatar_url))
                .color(Colour::from_rgb(88, 101, 242)),
        )
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles())
        .reply(true);

    ctx.send(res).await?;

    Ok(())
}
