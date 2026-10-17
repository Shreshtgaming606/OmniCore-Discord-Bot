use super::super::{RoleCompareResult, compare_roles};
use crate::{CustomContext, Error};
use poise::CreateReply;
use poise::serenity_prelude::{
    Colour, CreateAllowedMentions, CreateEmbed, Mentionable, Role, Timestamp,
};

#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    description_localized("en-US", "Compares two roles and returns the result."),
    broadcast_typing,
    category = "Utility",
    rename = "compare_roles"
)]
pub async fn compare_roles_f(
    ctx: CustomContext<'_>,
    #[description = "First role to compare"] role1: Role,
    #[description = "Second role to compare"] role2: Role,
) -> Result<(), Error> {
    let result = compare_roles(&role1, &role2);
    let result_str = match result {
        RoleCompareResult::Greater => "greater",
        RoleCompareResult::Equal => "equal",
        RoleCompareResult::Less => "less",
    };
    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .description(format!(
                    "{} is {} than {}",
                    role1.mention(),
                    result_str,
                    role2.mention()
                ))
                .color(Colour::from_rgb(88, 101, 242))
                .timestamp(Timestamp::now())
                .title("Role Comparison Result"),
        )
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles())
        .reply(true);

    ctx.send(res).await?;
    Ok(())
}
