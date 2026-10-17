use super::VisionError;
use poise::serenity_prelude::{
    self as serenity, Attachment, ChannelId, GuildId, MessageId, MessageReference,
    MessageReferenceKind,
};

/// A direct Discord reply authorizes one exact message ID in the same channel.
/// Forwards and cross-channel references do not authorize image access.
pub fn plan_for(
    reference: Option<&MessageReference>,
    channel: ChannelId,
    guild: Option<GuildId>,
) -> Result<Option<MessageId>, VisionError> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    if reference.kind != MessageReferenceKind::Default
        || reference.channel_id != channel
        || reference.guild_id.is_some_and(|id| Some(id) != guild)
    {
        return Err(VisionError::InvalidReference);
    }
    reference
        .message_id
        .map(Some)
        .ok_or(VisionError::InvalidReference)
}

pub async fn exact_reply_attachments(
    ctx: &serenity::Context,
    message: &serenity::Message,
) -> Result<Vec<Attachment>, VisionError> {
    let Some(id) = plan_for(
        message.message_reference.as_ref(),
        message.channel_id,
        message.guild_id,
    )?
    else {
        return Ok(Vec::new());
    };
    // Fetch this exact ID through Discord REST. Never traverse a reply chain or
    // read channel history; the embedded referenced_message is not trusted here.
    let target = message
        .channel_id
        .message(&ctx.http, id)
        .await
        .map_err(|_| VisionError::ReferenceUnavailable)?;
    if target.id != id
        || target.channel_id != message.channel_id
        || target.guild_id != message.guild_id
    {
        return Err(VisionError::InvalidReference);
    }
    Ok(target.attachments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reply_never_authorizes_history_lookup() {
        assert_eq!(
            plan_for(None, ChannelId::new(2), Some(GuildId::new(3))),
            Ok(None)
        );
    }

    #[test]
    fn only_exact_same_channel_and_guild_reply_is_allowed() {
        let reference = MessageReference::new(MessageReferenceKind::Default, ChannelId::new(2))
            .message_id(MessageId::new(7))
            .guild_id(GuildId::new(3));
        assert_eq!(
            plan_for(Some(&reference), ChannelId::new(2), Some(GuildId::new(3))),
            Ok(Some(MessageId::new(7)))
        );
        assert_eq!(
            plan_for(Some(&reference), ChannelId::new(4), Some(GuildId::new(3))),
            Err(VisionError::InvalidReference)
        );
        assert_eq!(
            plan_for(Some(&reference), ChannelId::new(2), Some(GuildId::new(5))),
            Err(VisionError::InvalidReference)
        );
    }

    #[test]
    fn forwarded_message_is_not_direct_reply() {
        let reference = MessageReference::new(MessageReferenceKind::Forward, ChannelId::new(2))
            .message_id(MessageId::new(7));
        assert_eq!(
            plan_for(Some(&reference), ChannelId::new(2), None),
            Err(VisionError::InvalidReference)
        );
    }
}
