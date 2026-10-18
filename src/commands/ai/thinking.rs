//! Per-server thinking control for local Ollama text and voice replies.

use crate::{CustomContext, Error, database};
use mongodb::bson::{Document, doc};
use ollama_rs::{
    generation::chat::{ChatMessage, request::ChatMessageRequest},
    models::ModelOptions,
};
use poise::{CreateReply, serenity_prelude::GuildId};

const SETTINGS: &str = "per_guild_settings";

fn from_document(document: Option<&Document>) -> bool {
    document
        .and_then(|document| document.get_bool("ai_thinking").ok())
        .unwrap_or(false)
}

pub(crate) async fn enabled(guild: GuildId) -> mongodb::error::Result<bool> {
    let document = database::get_collection(SETTINGS)?
        .find_one(doc! {"guild_id": guild.to_string()})
        .await?;
    Ok(from_document(document.as_ref()))
}

pub(crate) fn local_request(
    model: String,
    messages: Vec<ChatMessage>,
    options: ModelOptions,
    thinking: bool,
) -> ChatMessageRequest {
    ChatMessageRequest::new(model, messages)
        .options(options)
        .think(thinking)
}

/// Enable or disable local Ollama thinking for this server's text and voice AI.
#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn thinking(
    ctx: CustomContext<'_>,
    #[description = "Enable or disable local Ollama thinking in this server"] enabled: Option<bool>,
) -> Result<(), Error> {
    let guild = ctx.guild_id().ok_or("Server required")?;
    if guild.to_partial_guild(ctx.http()).await?.owner_id != ctx.author().id {
        ctx.send(
            CreateReply::default()
                .content("Only this server's owner can change AI thinking.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    if let Some(enabled) = enabled {
        database::get_collection(SETTINGS)?
            .update_one(
                doc! {"guild_id": guild.to_string()},
                doc! {"$set": {"ai_thinking": enabled}, "$setOnInsert": {"guild_id": guild.to_string()}},
            )
            .upsert(true)
            .await?;
    }
    let current = self::enabled(guild).await?;
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Local Ollama thinking is {} for this server's text mentions and voice replies. This setting applies to new requests. Connected cloud providers manage their own reasoning behavior.",
                if current { "enabled" } else { "disabled" }
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_defaults_off_for_existing_servers() {
        assert!(!from_document(None));
        assert!(!from_document(Some(&doc! {"guild_id": "123"})));
        assert!(from_document(Some(&doc! {"ai_thinking": true})));
    }

    #[test]
    fn local_requests_put_thinking_at_the_ollama_api_top_level() {
        for thinking in [false, true] {
            let request = local_request(
                "qwen3.5:4b".to_owned(),
                vec![ChatMessage::user("Hi".to_owned())],
                ModelOptions::default().num_ctx(2048),
                thinking,
            );
            let value = serde_json::to_value(request).unwrap();
            assert_eq!(value["think"], thinking);
            assert!(value["options"].get("think").is_none());
        }
    }
}
