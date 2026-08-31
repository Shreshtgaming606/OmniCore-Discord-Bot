use super::super::build_message_reply;
use crate::database::get_collection;
use crate::{CustomContext, Error};
use futures::StreamExt;
use mongodb::bson::doc;
use poise::CreateReply;
use poise::serenity_prelude::{
    Channel, Colour, CreateAllowedMentions, CreateEmbed, DiscordJsonError, EditRole,
    Error as SError, ErrorResponse, HttpError, Mentionable, Role, StatusCode, Timestamp,
};

#[poise::command(
    slash_command,
    prefix_command,
    required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_ROLES | MANAGE_CHANNELS",
    default_member_permissions = "MANAGE_CHANNELS",
    guild_only,
    broadcast_typing,
    category = "Moderation",
    description_localized("en-US", "Locks a channel. /help lock for more information")
)]
pub(crate) async fn lock(
    ctx: CustomContext<'_>,
    #[description = "Role to place lock role above"] target_role: Role,
    #[description = "Channel to lock, if not specified, will use the current channel"]
    channel: Channel,
) -> Result<(), Error> {
    //! Locks a channel.
    //!
    //! Locks a channel by placing a lock role above the specified role, applying it to every user, and apply it to the channel.
    ctx.defer().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or("This command must be used in a guild")?;

    let locked_channels_col =
        get_collection("locked_channels").expect("Failed to load locked_channels collection");
    //TODO: Check if we already have a lock role, if so, re use it and apply it to the channel.
    //Also add the role to users that don't have it already.
    let _ = match locked_channels_col
        .find_one(doc! {
            "guild_id": guild_id.get().to_string(),
            "channel_id": channel.id().get().to_string(),
        })
        .await?
    {
        Some(_) => {
            ctx.send(build_message_reply(
                ":x: Channel Already Locked",
                "This channel is already locked.",
                Colour::from_rgb(255, 0, 0),
                false,
            ))
            .await?;
            return Ok(());
        }
        None => {} // Don't create document yet
    };

    let lock_role = guild_id
        .create_role(
            ctx.http(),
            EditRole::new()
                .name("OmniCore Lock Role")
                .permissions(poise::serenity_prelude::Permissions::empty())
                .mentionable(false),
        )
        .await?;

    let channel_id = channel.id();

    channel_id
        .create_permission(
            ctx.http(),
            poise::serenity_prelude::PermissionOverwrite {
                allow: poise::serenity_prelude::Permissions::empty(),
                deny: poise::serenity_prelude::Permissions::SEND_MESSAGES
                    | poise::serenity_prelude::Permissions::ADD_REACTIONS
                    | poise::serenity_prelude::Permissions::CHANGE_NICKNAME
                    | poise::serenity_prelude::Permissions::CONNECT
                    | poise::serenity_prelude::Permissions::SPEAK
                    | poise::serenity_prelude::Permissions::SEND_MESSAGES_IN_THREADS
                    | poise::serenity_prelude::Permissions::MANAGE_THREADS
                    | poise::serenity_prelude::Permissions::CREATE_PUBLIC_THREADS,
                kind: poise::serenity_prelude::PermissionOverwriteType::Role(lock_role.id),
            },
        )
        .await?;

    // Now move the role above the specified role

    let new_position = target_role.position + 1;

    match guild_id
        .edit_role_position(ctx.http(), lock_role.id, new_position)
        .await
    {
        Ok(_) => {}
        Err(SError::Http(HttpError::UnsuccessfulRequest(ErrorResponse {
            status_code: StatusCode::FORBIDDEN,
            error: DiscordJsonError { code: 50013, .. },
            ..
        }))) => {
            let res = CreateReply::default()
                .embed(
                    CreateEmbed::new()
                        .description("I do not have the required permissions to place this role above the specified role.\n\nThis could mean that the bot's role is lower than the role of the role you are trying to place above.")
                        .title(":x: Missing Permissions")
                        .timestamp(Timestamp::now())
                        .color(Colour::from_rgb(255, 0, 0)),
                )
                .reply(true)
                .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles());

            ctx.send(res).await?;
            return Ok(());
        }
        Err(e) => {
            log::error!("Error in placing role: {}", e);
            log::error!("Error Details: {:#?}", e);
            return Err(e.into());
        }
    }

    // Now begin to apply the role to all users, saving failed attempts in a number to report back to the user

    #[allow(unused_assignments)]
    let mut failed_unreportable = 0;

    ctx.defer().await?; // This takes a while

    let mut members = guild_id.members_iter(ctx.http()).boxed();

    while let Some(member_result) = members.next().await {
        match member_result {
            Ok(member) => {
                if !member.roles.contains(&lock_role.id) {
                    match member.add_role(ctx.http(), lock_role.id).await {
                        Ok(_) => {}
                        Err(_) => failed_unreportable += 1,
                    }
                }
            }
            Err(_) => failed_unreportable += 1,
        }
    }

    let _ = locked_channels_col
        .insert_one(doc! {
            "guild_id": guild_id.get().to_string(),
            "channel_id": channel_id.get().to_string(),
            "role_id": lock_role.id.get().to_string(),
        })
        .await?;

    let res = build_message_reply(
        "🔒 Channel Locked",
        format!(
            "Members with or below role \"{}\" are not able to talk or react in {}\n### Summary: Failed to add role to {} users.",
            lock_role.mention(),
            channel.mention(),
            failed_unreportable
        )
        .as_str(),
        Colour::from_rgb(0, 255, 0),
        false,
    );
    ctx.send(res).await?;

    return Ok(());
}
