use super::super::{RoleCompareResult, compare_roles, get_highest_role_from_member};
use crate::commands::build_message_reply;
use crate::{CustomContext, Error, get_guild_owner_id};
use poise::CreateReply;
use poise::serenity_prelude::{
    Colour, CreateAllowedMentions, CreateEmbed, CreateEmbedAuthor, DiscordJsonError,
    Error as SError, ErrorResponse, HttpError, Member, Mentionable, StatusCode, Timestamp,
};

#[poise::command(
    slash_command,
    prefix_command,
    required_permissions = "BAN_MEMBERS",
    default_member_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS",
    guild_only,
    broadcast_typing,
    category = "Moderation",
    description_localized("en-US", "Bans a member from the server.")
)]
pub(crate) async fn ban(
    ctx: CustomContext<'_>,
    #[description = "Member to ban"] member: Member,
    #[description = "How much of their recent messages to delete"] delete_message_days: Option<u8>,
    #[description = "Reason for the ban"]
    #[rest]
    reason: Option<String>, // #[rest] uses the rest of the message as the reason
) -> Result<(), Error> {
    //! Ban a member from the server.

    let author_member = ctx.author_member().await.unwrap();
    let guild_owner_id = get_guild_owner_id(&ctx).await;

    // check if the user is higher or lower than the user they're trying to ban, and also check if owner
    if !(author_member.user.id == guild_owner_id) {
        // owner bypasses role check
        let author_highest_role = get_highest_role_from_member(&*author_member, ctx);
        let member_highest_role = get_highest_role_from_member(&member, ctx);

        if let Some(member_highest_role) = member_highest_role {
            if let Some(author_highest_role) = author_highest_role {
                let member_highest_role_role = ctx
                    .guild()
                    .unwrap()
                    .roles
                    .get(&member_highest_role)
                    .unwrap()
                    .clone();
                let author_highest_role = ctx
                    .guild()
                    .unwrap()
                    .roles
                    .get(&author_highest_role)
                    .unwrap()
                    .clone();

                match compare_roles(&author_highest_role, &member_highest_role_role) {
                    RoleCompareResult::Greater => {}
                    RoleCompareResult::Less => {
                        let res = build_message_reply(
                            ":x: Insufficient Permissions",
                            "You cannot ban this user because they have a higher role than you.",
                            Colour::from_rgb(255, 0, 0),
                            true,
                        );

                        ctx.send(res).await?;
                        return Ok(());
                    }
                    RoleCompareResult::Equal => {
                        let res = build_message_reply(
                            ":x: Insufficient Permissions",
                            "You cannot ban this user because they have the same role as you.",
                            Colour::from_rgb(255, 0, 0),
                            true,
                        );

                        ctx.send(res).await?;
                        return Ok(());
                    }
                }
            }
        }
    }

    let reason_pre = reason.unwrap_or_else(|| "No reason provided".to_string());

    let reason = format!("{} | Banned by {}", reason_pre, ctx.author().tag());

    match member
        .ban_with_reason(&ctx.http(), delete_message_days.unwrap_or(0), &reason)
        .await
    {
        Ok(_) => {}
        Err(SError::Http(HttpError::UnsuccessfulRequest(ErrorResponse {
            status_code: StatusCode::FORBIDDEN,
            error: DiscordJsonError { code: 50013, .. },
            ..
        }))) => {
            let res = build_message_reply(
                ":x: Missing Permissions",
                "I do not have the required permissions to ban this user.\n\nThis could mean that the bot's role is lower than the role of the user you are trying to ban.",
                Colour::from_rgb(255, 0, 0),
                true,
            );

            ctx.send(res).await?;
            return Ok(());
        }
        Err(e) => {
            log::error!("Error in banning user: {}", e);
            log::error!("Error Details: {:#?}", e);
            return Err(e.into());
        }
    }

    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .description(format!(
                    "User \"{}\" banned \"{}\" for reason \"{}\"",
                    ctx.author().mention(),
                    member.mention(),
                    reason_pre
                ))
                .title("User Banned Successfully")
                .timestamp(Timestamp::now())
                .author(
                    CreateEmbedAuthor::new(member.display_name()).icon_url(
                        member
                            .user
                            .avatar_url()
                            .unwrap_or_else(|| member.user.default_avatar_url()),
                    ),
                )
                .color(Colour::from_rgb(0, 255, 0)),
        )
        .reply(true)
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles());

    ctx.send(res).await?;

    Ok(())
}
