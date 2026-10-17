//! A direct, owner-authorized conversational mute. The model never chooses a target or duration.

use crate::automod::{config::parse_owner_duration, highest_role};
use poise::serenity_prelude::{
    Context, CreateAllowedMentions, CreateMessage, EditMember, Message, Permissions, UserId,
};
use regex::Regex;
use std::time::Duration;

fn is_mute_request(content: &str) -> bool {
    Regex::new(
        r"(?i)^\s*(?:<@!?\d+>\s*)?(?:(?:please|hey|can you|could you|would you|(?:you're|youre|your|you are)\s+supposed\s+to)\s+)*(?:mute|timeout|time\s+out|time)\b",
    )
    .expect("valid mute request pattern")
    .is_match(content)
}

fn requested_duration(content: &str) -> Result<Duration, &'static str> {
    let pattern = Regex::new(
        r"(?i)\b(\d{1,4})\s*(seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)\b",
    )
    .expect("valid duration pattern");
    let captures = pattern
        .captures(content)
        .ok_or("Include a duration, such as `10m` or `1 hour` (maximum 2 days).")?;
    parse_owner_duration(&format!("{}{}", &captures[1], &captures[2]))
}

fn target_id(
    mentions: &[UserId],
    bot_id: UserId,
    reply_author: Option<UserId>,
) -> Result<UserId, &'static str> {
    let people: Vec<UserId> = mentions
        .iter()
        .copied()
        .filter(|id| *id != bot_id)
        .collect();
    match people.as_slice() {
        [person] => Ok(*person),
        [] => reply_author
            .filter(|id| *id != bot_id)
            .ok_or("Mention one member to mute, or reply to that member's message."),
        _ => Err("Mention only one member so I know whom to mute."),
    }
}

async fn respond(ctx: &Context, msg: &Message, text: &str) {
    let result = msg
        .channel_id
        .send_message(
            &ctx.http,
            CreateMessage::new().content(text).allowed_mentions(
                CreateAllowedMentions::new()
                    .empty_users()
                    .empty_roles()
                    .everyone(false),
            ),
        )
        .await;
    if let Err(error) = result {
        log::warn!("Owner mute response failed: {error}");
    }
}

/// Returns true only when an explicit request from the current guild owner was handled.
pub async fn maybe_handle(ctx: &Context, msg: &Message, bot_id: UserId) -> bool {
    if !is_mute_request(&msg.content) {
        return false;
    }
    let Some(guild_id) = msg.guild_id else {
        return false;
    };
    let guild = match guild_id.to_partial_guild(&ctx.http).await {
        Ok(guild) => guild,
        Err(error) => {
            log::warn!("Owner mute guild lookup failed: {error}");
            respond(
                ctx,
                msg,
                "I couldn't verify server ownership right now. Try again shortly.",
            )
            .await;
            return true;
        }
    };
    if msg.author.id != guild.owner_id {
        return false;
    }
    let mentions: Vec<UserId> = msg.mentions.iter().map(|user| user.id).collect();
    let target_id = match target_id(
        &mentions,
        bot_id,
        msg.referenced_message
            .as_ref()
            .map(|message| message.author.id),
    ) {
        Ok(id) => id,
        Err(message) => {
            respond(ctx, msg, message).await;
            return true;
        }
    };
    let duration = match requested_duration(&msg.content) {
        Ok(duration) => duration,
        Err(message) => {
            respond(ctx, msg, message).await;
            return true;
        }
    };
    if target_id == guild.owner_id || target_id == bot_id {
        respond(ctx, msg, "I can't mute the server owner or myself.").await;
        return true;
    }
    let target = match guild_id.member(&ctx.http, target_id).await {
        Ok(member) => member,
        Err(_) => {
            respond(ctx, msg, "I couldn't find that member in this server.").await;
            return true;
        }
    };
    if target.user.bot
        || guild
            .member_permissions(&target)
            .contains(Permissions::ADMINISTRATOR)
    {
        respond(ctx, msg, "I can't mute a bot or an administrator.").await;
        return true;
    }
    let bot = match guild_id.member(&ctx.http, bot_id).await {
        Ok(member) => member,
        Err(_) => {
            respond(
                ctx,
                msg,
                "I couldn't verify my server permissions right now.",
            )
            .await;
            return true;
        }
    };
    if !guild
        .member_permissions(&bot)
        .contains(Permissions::MODERATE_MEMBERS)
        || highest_role(&bot, &guild.roles) <= highest_role(&target, &guild.roles)
    {
        respond(
            ctx,
            msg,
            "I need Moderate Members permission and a role above that member to mute them.",
        )
        .await;
        return true;
    }
    let until = chrono::Utc::now() + chrono::Duration::seconds(duration.as_secs() as i64);
    let reason = format!(
        "Owner-requested mute via OmniCore by user {}",
        msg.author.id
    );
    match ctx
        .http
        .edit_member(
            guild_id,
            target_id,
            &EditMember::new().disable_communication_until(until.to_rfc3339()),
            Some(&reason),
        )
        .await
    {
        Ok(_) => {
            log::info!(
                "Owner mute applied guild={guild_id} actor={} target={target_id} duration={}",
                msg.author.id,
                duration.as_secs()
            );
            respond(
                ctx,
                msg,
                &format!(
                    "Muted member `{target_id}` for {} minutes.",
                    duration.as_secs() / 60
                ),
            )
            .await;
        }
        Err(error) => {
            log::warn!("Owner mute failed guild={guild_id} target={target_id}: {error}");
            respond(ctx, msg, "Discord rejected the mute. Check my role position and Moderate Members permission.").await;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_request_needs_clear_target_and_bounded_duration() {
        assert!(is_mute_request("<@123> please mute <@456> for 1 hour"));
        assert!(is_mute_request("<@123> time <@456> for 10m"));
        assert!(is_mute_request("your supposed to time them gngng"));
        assert!(!is_mute_request("<@123> would you ever mute someone?"));
        assert_eq!(
            requested_duration("<@123> mute <@456> for 1 hour")
                .unwrap()
                .as_secs(),
            3600
        );
        assert!(requested_duration("mute <@456> for 3d").is_err());
        let bot = UserId::new(123);
        let target = UserId::new(456);
        assert_eq!(target_id(&[bot, target], bot, None).unwrap(), target);
        assert_eq!(target_id(&[bot], bot, Some(target)).unwrap(), target);
        assert!(target_id(&[bot], bot, None).is_err());
    }
}
