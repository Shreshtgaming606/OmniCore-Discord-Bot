pub mod attachments;
pub mod context;
pub mod ollama;
pub mod reference;

use crate::{Data, Error, byok::storage::ByokService, database};
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateMessage, Message};
use std::{sync::OnceLock, time::Duration};
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisionRoute {
    Local,
    Provider,
    ProviderDm,
}

fn guild_route(approved: bool, provider_connected: bool) -> Option<VisionRoute> {
    if approved {
        Some(VisionRoute::Local)
    } else if provider_connected {
        Some(VisionRoute::Provider)
    } else {
        None
    }
}

const SETUP_GUIDANCE: &str = "This server is not approved for locally hosted AI. The server owner can run `/ai setup` to connect a vision-capable provider, or ask the bot owners to approve this server.";

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
    let service = data.byok.as_deref();
    let (guild, roles, question, route, max_images, max_output_tokens) = if let Some(guild) =
        message.guild_id
    {
        let approved = match database::is_ai_approved(guild.get()).await {
            Ok(approved) => approved,
            Err(_) => {
                reply(ctx, message, "I couldn't check this server's AI approval right now. Please try again shortly.").await?;
                return Ok(true);
            }
        };
        let provider_connected = if approved {
            false
        } else if let Some(service) = service {
            match service.connection(guild.get()).await {
                Ok(connection) => connection.is_some(),
                Err(_) => {
                    reply(ctx, message, "I couldn't check this server's AI provider right now. Please try again shortly.").await?;
                    return Ok(true);
                }
            }
        } else {
            false
        };
        let Some(route) = guild_route(approved, provider_connected) else {
            reply(ctx, message, SETUP_GUIDANCE).await?;
            return Ok(true);
        };
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
        let roles = member.roles.iter().map(|id| id.get()).collect::<Vec<_>>();
        let access = if let Some(service) = service {
            match route {
                VisionRoute::Local => {
                    service
                        .local_vision_access(guild.get(), message.channel_id.get(), &roles)
                        .await
                }
                VisionRoute::Provider => service
                    .vision_access(guild.get(), message.channel_id.get(), &roles)
                    .await
                    .map(|max| (max, 512)),
                VisionRoute::ProviderDm => unreachable!(),
            }
        } else {
            Ok((attachments::HARD_MAX_IMAGES, 512))
        };
        let (max_images, max_output_tokens) = match access {
            Ok(access) => access,
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
        (
            guild.get(),
            roles,
            question,
            route,
            max_images,
            max_output_tokens,
        )
    } else {
        let Some(service) = service else {
            reply(ctx, message, "DM image analysis requires a server owner's configured provider key and explicit DM opt-in.").await?;
            return Ok(true);
        };
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
        let max_images = match service
            .dm_vision_access(guild, message.author.id.get())
            .await
        {
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
        (
            guild,
            Vec::new(),
            question,
            VisionRoute::ProviderDm,
            max_images,
            512,
        )
    };
    let _typing = serenity::Typing::start(ctx.http.clone(), message.channel_id);
    let result = tokio::time::timeout(
        Duration::from_secs(55),
        process(
            ctx,
            message,
            service,
            guild,
            &question,
            &roles,
            max_images,
            max_output_tokens,
            route,
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
    service: Option<&ByokService>,
    guild: u64,
    question: &str,
    roles: &[u64],
    max_images: usize,
    max_output_tokens: u32,
    route: VisionRoute,
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
    if route == VisionRoute::Local {
        static LOCAL_SLOTS: OnceLock<Semaphore> = OnceLock::new();
        let _slot = LOCAL_SLOTS
            .get_or_init(|| Semaphore::new(2))
            .try_acquire()
            .map_err(|_| "The local image model is busy. Please try again shortly.".to_string())?;
        if !database::is_ai_approved(guild).await.unwrap_or(false) {
            return Err(SETUP_GUIDANCE.to_string());
        }
        let _active = if let Some(service) = service {
            Some(
                service
                    .begin_local_vision(
                        guild,
                        message.author.id.get(),
                        message.channel_id.get(),
                        roles,
                    )
                    .await
                    .map_err(|error| {
                        crate::commands::ai::byok_commands::error_message(error).to_string()
                    })?,
            )
        } else {
            None
        };
        return ollama::complete(guild, question, &images, max_output_tokens)
            .await
            .map_err(str::to_string);
    }
    let service = service.ok_or_else(|| SETUP_GUIDANCE.to_string())?;
    let response = match route {
        VisionRoute::ProviderDm => {
            service
                .ask_vision_dm(
                    guild,
                    message.author.id.get(),
                    message.channel_id.get(),
                    question,
                    &images,
                )
                .await
        }
        VisionRoute::Provider => {
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
        }
        VisionRoute::Local => unreachable!(),
    };
    response
        .map(|response| response.text)
        .map_err(|error| crate::commands::ai::byok_commands::error_message(error).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_guild_uses_local_vision_even_without_a_provider() {
        assert_eq!(guild_route(true, false), Some(VisionRoute::Local));
        assert_eq!(guild_route(true, true), Some(VisionRoute::Local));
        assert_eq!(guild_route(false, true), Some(VisionRoute::Provider));
        assert_eq!(guild_route(false, false), None);
    }

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
