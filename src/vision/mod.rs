pub mod attachments;
pub mod context;
pub mod reference;

use crate::{Data, Error, byok::storage::ByokService};
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateMessage, Message};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisionError {
    NoImage,
    InvalidReference,
    ReferenceUnavailable,
    TooManyImages,
    TooLarge,
    UnsupportedFormat,
    InvalidImage,
    DownloadFailed,
    Expired,
    TimedOut,
}

impl VisionError {
    fn message(self) -> &'static str {
        match self {
            Self::NoImage => {
                "Please attach the image to your message or reply directly to the image you want me to analyze. 📸"
            }
            Self::InvalidReference => {
                "Please reply directly to an image in this channel. I can't use forwarded or cross-channel images."
            }
            Self::ReferenceUnavailable => {
                "I couldn't access the image message you replied to. It may have been deleted, or I may lack permission to view it."
            }
            Self::TooManyImages => {
                "I can analyze up to 3 images at a time. Please select fewer images."
            }
            Self::TooLarge => {
                "One image is too large. Please use an image under 4 MB and 20 megapixels."
            }
            Self::UnsupportedFormat => {
                "I can analyze PNG, JPEG, WebP, and GIF images. Please attach one of those formats."
            }
            Self::InvalidImage => {
                "I couldn't decode that image safely. Please re-export it as PNG, JPEG, or WebP and try again."
            }
            Self::DownloadFailed => {
                "I couldn't download that Discord image. Please reattach it and try again."
            }
            Self::Expired => {
                "That Discord image link has expired or is unavailable. Please reattach the image."
            }
            Self::TimedOut => {
                "Image analysis took too long. Please try again with a smaller image."
            }
        }
    }
}

fn question(message: &Message, bot_id: serenity::UserId) -> String {
    message
        .content
        .replace(&format!("<@{}>", bot_id), "")
        .replace(&format!("<@!{}>", bot_id), "")
        .trim()
        .to_string()
}

fn dm_server_selection(question: &str) -> (Option<u64>, String) {
    let mut selected = None;
    let mut words = Vec::new();
    for word in question.split_whitespace() {
        let id = word
            .strip_prefix("server:")
            .or_else(|| word.strip_prefix("guild:"))
            .and_then(|id| id.parse::<u64>().ok());
        if id.is_some() && selected.is_none() {
            selected = id;
        } else {
            words.push(word);
        }
    }
    (selected, words.join(" "))
}

async fn reply(ctx: &serenity::Context, message: &Message, text: &str) -> Result<(), Error> {
    message
        .channel_id
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(text)
                .reference_message(message)
                .allowed_mentions(
                    CreateAllowedMentions::new()
                        .empty_users()
                        .empty_roles()
                        .everyone(false),
                ),
        )
        .await?;
    Ok(())
}

/// Returns true when this invocation was an explicit image-analysis request.
/// The legacy text-chat handler is never entered for these requests.
pub async fn maybe_handle(
    ctx: &serenity::Context,
    message: &Message,
    data: &Data,
    bot_id: serenity::UserId,
) -> Result<bool, Error> {
    let question = question(message, bot_id);
    let has_source = !message.attachments.is_empty() || message.message_reference.is_some();
    let has_attachment = !message.attachments.is_empty()
        || message
            .referenced_message
            .as_ref()
            .is_some_and(|referenced| !referenced.attachments.is_empty());
    let unresolved_reply =
        message.message_reference.is_some() && message.referenced_message.is_none();
    if !context::requests_image_analysis(&question, has_attachment, unresolved_reply) {
        return Ok(false);
    }
    if !has_source {
        reply(ctx, message, VisionError::NoImage.message()).await?;
        return Ok(true);
    }
    let Some(service) = data.byok.as_ref() else {
        reply(ctx, message, "Secure AI setup is not configured. The server owner can run /ai setup to connect a vision-capable provider.").await?;
        return Ok(true);
    };
    let (guild, roles, question, dm_owner) = if let Some(guild) = message.guild_id {
        let member = match guild.member(&ctx.http, message.author.id).await {
            Ok(member) => member,
            Err(_) => {
                reply(
                    ctx,
                    message,
                    "I couldn't verify your server permissions for image analysis.",
                )
                .await?;
                return Ok(true);
            }
        };
        (
            guild.get(),
            member.roles.iter().map(|id| id.get()).collect::<Vec<_>>(),
            question,
            false,
        )
    } else {
        let (requested, question) = dm_server_selection(&question);
        let candidates = match service.dm_vision_guilds(message.author.id.get()).await {
            Ok(guilds) => guilds,
            Err(error) => {
                reply(
                    ctx,
                    message,
                    crate::commands::ai::byok_commands::error_message(error),
                )
                .await?;
                return Ok(true);
            }
        };
        let guild = match requested {
            Some(id) if candidates.iter().any(|(guild, _)| *guild == id) => id,
            Some(_) => {
                reply(
                    ctx,
                    message,
                    "That server is not authorized for DM image analysis with your account.",
                )
                .await?;
                return Ok(true);
            }
            None if candidates.len() == 1 => candidates[0].0,
            None if candidates.is_empty() => {
                reply(ctx, message, "To analyze images in DMs, a server you own must have a vision-capable BYOK model and DM vision enabled with /ai vision. Otherwise, ask me in the server.").await?;
                return Ok(true);
            }
            None => {
                let choices = candidates
                    .iter()
                    .take(10)
                    .map(|(id, name)| format!("{name}: `server:{id}`"))
                    .collect::<Vec<_>>()
                    .join("\n");
                reply(ctx, message, &format!("Choose which server key should pay by adding `server:<ID>` to your DM:\n{choices}")).await?;
                return Ok(true);
            }
        };
        (guild, Vec::new(), question, true)
    };
    let access = if dm_owner {
        service
            .dm_vision_access(guild, message.author.id.get())
            .await
    } else {
        service
            .vision_access(guild, message.channel_id.get(), &roles)
            .await
    };
    let max_images = match access {
        Ok(max) => max,
        Err(error) => {
            reply(
                ctx,
                message,
                crate::commands::ai::byok_commands::error_message(error),
            )
            .await?;
            return Ok(true);
        }
    };
    let _typing = serenity::Typing::start(ctx.http.clone(), message.channel_id);
    let result = tokio::time::timeout(
        Duration::from_secs(55),
        process(
            ctx, message, service, guild, &question, &roles, max_images, dm_owner,
        ),
    )
    .await;
    let answer = match result {
        Ok(Ok(text)) => text,
        Ok(Err(error)) => error,
        Err(_) => VisionError::TimedOut.message().to_string(),
    };
    reply(
        ctx,
        message,
        &answer.chars().take(1_900).collect::<String>(),
    )
    .await?;
    Ok(true)
}

async fn process(
    ctx: &serenity::Context,
    message: &Message,
    service: &ByokService,
    guild: u64,
    question: &str,
    roles: &[u64],
    max_images: usize,
    dm_owner: bool,
) -> Result<String, String> {
    let referenced = reference::exact_reply_attachments(ctx, message)
        .await
        .map_err(|error| error.message().to_string())?;
    let selected = attachments::select(&message.attachments, &referenced, max_images)
        .map_err(|error| if error == VisionError::TooManyImages {
            format!("This server allows up to {max_images} images per request. Please select fewer images.")
        } else { error.message().to_string() })?;
    let mut images = Vec::with_capacity(selected.len());
    for attachment in selected {
        images.push(
            attachments::download(&attachment)
                .await
                .map_err(|error| error.message().to_string())?,
        );
    }
    let response = if dm_owner {
        service
            .ask_vision_dm(
                guild,
                message.author.id.get(),
                message.channel_id.get(),
                question,
                &images,
            )
            .await
    } else {
        service
            .ask_vision(
                guild,
                message.author.id.get(),
                message.channel_id.get(),
                roles,
                question,
                &images,
            )
            .await
    };
    response
        .map(|response| response.text)
        .map_err(|error| crate::commands::ai::byok_commands::error_message(error).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_server_selection_requires_an_explicit_numeric_id() {
        assert_eq!(
            dm_server_selection("What is this image?"),
            (None, "What is this image?".to_string())
        );
        assert_eq!(
            dm_server_selection("server:42 What car is this?"),
            (Some(42), "What car is this?".to_string())
        );
        assert_eq!(
            dm_server_selection("server:wrong What is this?"),
            (None, "server:wrong What is this?".to_string())
        );
    }
}
