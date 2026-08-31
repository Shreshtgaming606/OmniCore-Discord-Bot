use super::SYSTEM_PROMPT;
use super::tools::{timeout::Timeout, wikipedia::Wikipedia, wikipedia_search::WikipediaSearch};
use crate::OLLAMA;
use crate::config::{BOT_OWNERS, OLLAMA_MODEL};
use crate::database::get_collection;
use crate::{Data, Error};
use chrono::Utc;
use mongodb::bson::Bson;
use mongodb::bson::doc;
use mongodb::options::UpdateOptions;
use ollama_rs::generation::chat::{ChatMessage, MessageRole, request::ChatMessageRequest};
use ollama_rs::generation::tools::Tool;
use ollama_rs::models::ModelOptions;
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::Context;
use serde_json::Value;

/// Max amount of chars per message, as defined in Discord API docs.
const DISCORD_MESSAGE_LIMIT: usize = 2000;

/// Max number of messages to keep in the context window. Excludes sys prompt
const HISTORY_WINDOW: i64 = 40;

/// Max number of tool<->call round trips per mention, to bound runaway tool loops.
const MAX_TOOL_HOPS: u8 = 8;

/// Splits a message into chunks that fit Discord's per-message character limit.
/// Prefers splitting on paragraph/line boundaries over splitting mid-word.
fn chunk_message(content: &str, limit: usize) -> Vec<String> {
    if content.chars().count() <= limit {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in content.split_inclusive('\n') {
        if current.chars().count() + line.chars().count() > limit {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            // A single line longer than the limit still needs hard-splitting.
            if line.chars().count() > limit {
                for chunk in line.chars().collect::<Vec<_>>().chunks(limit) {
                    chunks.push(chunk.iter().collect());
                }
                continue;
            }
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Called when a message is sent in a guild that has AI approved.
pub(crate) async fn on_mention(
    ctx: &Context,
    msg: &serenity::Message,
    _data: &Data,
    _event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, Error>,
) -> Result<(), Error> {
    // Don't allow DMs
    let Some(guild_id) = msg.guild_id else {
        msg.reply(ctx.http.clone(), "I only work inside servers, not DMs.")
            .await?;
        return Ok(());
    };

    let owners = BOT_OWNERS
        .get()
        .unwrap()
        .clone()
        .into_iter()
        .map(|id| format!("<@{}>", id))
        .collect::<Vec<String>>()
        .join(", ");

    // Check if the guild is approved for AI usage
    let per_guild_settings_col =
        get_collection("per_guild_settings").expect("Failed to load per_guild_settings collection");
    let guild_settings = per_guild_settings_col
        .find_one(doc! {"guild_id": guild_id.to_string()})
        .await?
        .ok_or("Guild settings not found")?;
    if guild_settings.get_bool("ai_approved").unwrap_or(false) == false {
        msg.reply(ctx.http.clone(), format!("This server is not approved for AI usage.\nYou can DM the users below with your Guild ID and ask to approve AI usage:\n{}\nYour Guild ID is `{}`", owners, guild_id.to_string())).await?;
        return Ok(());
    }

    log::info!("Mentioned in guild {}", guild_id);

    let typing = poise::serenity_prelude::Typing::start(ctx.http.clone(), msg.channel_id);

    let guild_owner_id = ctx
        .cache
        .guild(guild_id)
        .map(|g| g.owner_id.to_string())
        .unwrap_or_default();

    let channel_name = msg
        .channel_id
        .name(ctx.http.clone())
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    // Build the doc representing the current message.
    let mut prompt = mongodb::bson::Document::new();
    prompt.insert("content", Bson::String(msg.content.clone()));
    prompt.insert(
        "author",
        Bson::Document(doc! {
            "id": msg.author.id.to_string(),
            "name": msg.author.name.clone(),
        }),
    );
    prompt.insert("timestamp", Bson::String(msg.timestamp.to_string()));
    prompt.insert(
        "channel",
        Bson::Document(doc! {
            "id": msg.channel_id.to_string(),
            "name": channel_name.clone(),
        }),
    );
    prompt.insert(
        "guild",
        Bson::Document(doc! {
            "owner": { "id": guild_owner_id.clone() }
        }),
    );
    prompt.insert("role", "USER");

    let mut message_doc_content = doc! { "id": msg.id.to_string() };
    if let Some(referenced) = &msg.referenced_message {
        message_doc_content.insert("replying_to", doc! { "id": referenced.id.to_string() });
    } else {
        message_doc_content.insert("replying_to", Bson::Null);
    }
    prompt.insert("message", Bson::Document(message_doc_content));

    let messages_col = get_collection("messages").expect("Failed to load messages collection");

    // Atomically ensure the guild doc exists and push the new message in one round trip.
    // $setOnInsert only applies when the upsert creates a new doc, so no race with concurrent
    // mentions in a guild that has no doc yet.
    messages_col
        .update_one(
            doc! { "guild_id": guild_id.to_string() },
            doc! {
                "$setOnInsert": { "guild_id": guild_id.to_string() },
                "$push": { "messages": prompt.clone() },
            },
        )
        .with_options(UpdateOptions::builder().upsert(true).build())
        .await?;

    // Fetch back only the last HISTORY_WINDOW messages via $slice, keeping context bounded.
    let messages_doc = messages_col
        .find_one(doc! { "guild_id": guild_id.to_string() })
        .projection(doc! { "messages": { "$slice": -HISTORY_WINDOW } })
        .await?
        .ok_or("Guild message document missing immediately after upsert")?;

    let mut messages = messages_doc.get_array("messages")?.clone();

    // Changeable with /change_prompt
    let custom_system_prompt = guild_settings.get_str("ai_prompt").unwrap_or_default();

    // Add system prompt to beginning of message history
    messages.insert(
        0,
        Bson::Document(doc! {
            "content": SYSTEM_PROMPT.to_string().replace("<BOT_USER_ID>", &framework.bot_id.to_string()).replace("<CUSTOM_SYSTEM_PROMPT>", &custom_system_prompt).replace("<BOT_OWNERS>", &owners),
            "author": {
                "id": "0".to_string(),
                "name": "System".to_string(),
            },
            "timestamp": Utc::now().timestamp(),
            "human_time": Utc::now().to_rfc3339(),
            "channel": {
                "id": "0".to_string(),
                "name": "System".to_string(),
            },
            "guild": {
                "owner": { "id": "0".to_string() }
            },
            "role": "SYSTEM".to_string()
        }),
    );

    // Only USER turns carry the full JSON envelope (author/channel/guild metadata the model
    // needs to act correctly). SYSTEM and ASSISTANT turns are plain text - if assistant replies
    // are stored back into history as JSON docs, the model starts pattern-matching on its own
    // prior turns and echoes JSON back to users instead of plain text, no matter what the system
    // prompt instructs. TOOL turns keep the structured form since tool output is inherently data.
    let mut chat_history = messages
        .iter()
        .map(|b| {
            let doc = b.as_document().unwrap();
            let role = doc.get_str("role").unwrap();
            match role {
                "USER" => Ok(ChatMessage::new(MessageRole::User, b.to_string())),
                "SYSTEM" => {
                    let content = doc.get_str("content").unwrap_or_default().to_string();
                    Ok(ChatMessage::new(MessageRole::System, content))
                }
                "ASSISTANT" => {
                    let content = doc.get_str("content").unwrap_or_default().to_string();
                    Ok(ChatMessage::new(MessageRole::Assistant, content))
                }
                "TOOL" => Ok(ChatMessage::new(MessageRole::Tool, b.to_string())),
                other => Err(format!("Invalid role in stored message: {}", other)),
            }
        })
        .collect::<Result<Vec<ChatMessage>, String>>()?;

    let model = OLLAMA_MODEL.get().unwrap().clone();
    let ollama = OLLAMA.get().unwrap().clone();

    let mut wikipedia = Wikipedia::new();
    let mut timeout = Timeout::new(ctx.clone(), msg.clone(), guild_id.clone());
    let mut wikipedia_search = WikipediaSearch::new();

    // Tool-call loop: send the request, and if the model asks for a tool, run it, append the
    // result, and re-send. `send_chat_messages_with_history` appends both the outgoing turns in
    // `request.messages` and the incoming response to `chat_history` itself, so after the first
    // hop we only need to pass the new tool-result turn(s), not the whole history again.
    let mut next_turns: Vec<ChatMessage> = vec![];
    let mut final_res = None;

    for hop in 0..=MAX_TOOL_HOPS {
        let request = ChatMessageRequest::new(model.clone(), next_turns.clone())
            .options(
                ModelOptions::default()
                    .num_ctx(16384)
                    .num_predict(-1)
                    .extra("think", Value::Bool(false)),
            )
            .add_tool(Wikipedia::new())
            .add_tool(WikipediaSearch::new())
            .add_tool(Timeout::new(ctx.clone(), msg.clone(), guild_id.clone()));

        let res = ollama
            .send_chat_messages_with_history(&mut chat_history, request)
            .await;

        let res = match res {
            Ok(res) => res,
            Err(e) => {
                typing.stop();
                log::error!("Ollama request failed for guild {}: {}", guild_id, e);
                msg.reply(
                    ctx.http.clone(),
                    "Sorry, I couldn't process that — something went wrong on my end.",
                )
                .await?;
                return Err(e.into());
            }
        };

        if res.message.tool_calls.is_empty() {
            final_res = Some(res);
            break;
        }

        if hop == MAX_TOOL_HOPS {
            // AI wants more tool calls, but we've hit the max hops limit. Return the current response.
            final_res = Some(res);
            break;
        }

        next_turns.clear();
        for call in &res.message.tool_calls {
            let result = if call.function.name == Wikipedia::name() {
                match serde_json::from_value(call.function.arguments.clone()) {
                    Ok(params) => wikipedia
                        .call(params)
                        .await
                        .unwrap_or_else(|e| format!("Wikipedia lookup failed: {}", e)),
                    Err(e) => format!("Invalid arguments for wikipedia: {}", e),
                }
            } else if call.function.name == Timeout::name() {
                match serde_json::from_value(call.function.arguments.clone()) {
                    Ok(params) => timeout
                        .call(params)
                        .await
                        .unwrap_or_else(|e| format!("Timeout failed: {}", e)),
                    Err(e) => format!("Invalid arguments for timeout: {}", e),
                }
            } else if call.function.name == WikipediaSearch::name() {
                match serde_json::from_value(call.function.arguments.clone()) {
                    Ok(params) => wikipedia_search
                        .call(params)
                        .await
                        .unwrap_or_else(|e| format!("Wikipedia search failed: {}", e)),
                    Err(e) => format!("Invalid arguments for wikipedia_search: {}", e),
                }
            } else {
                format!("Unknown tool: {}", call.function.name)
            };

            // Persist the tool turn to Mongo, mirroring response_doc's shape below, so the next
            // mention's rebuilt history includes this hop.
            let tool_doc = doc! {
                "content": &result,
                "author": { "id": "Tool", "name": call.function.name.clone() },
                "timestamp": Utc::now().timestamp(),
                "human_time": Utc::now().to_rfc3339(),
                "channel": { "id": msg.channel_id.to_string(), "name": channel_name.clone() },
                "guild": { "owner": { "id": guild_owner_id.clone() } },
                "role": "TOOL".to_string()
            };
            if let Err(e) = messages_col
                .update_one(
                    doc! { "guild_id": guild_id.to_string() },
                    doc! { "$push": { "messages": tool_doc.clone() } },
                )
                .await
            {
                log::error!("Failed to persist tool turn for guild {}: {}", guild_id, e);
            }

            next_turns.push(ChatMessage::tool(tool_doc.to_string()));
            next_turns.push(ChatMessage::new(
                MessageRole::System,
                "Now respond to the user in plain text using the tool result above.".to_string(),
            ));
        }
    }

    typing.stop();

    let res = match final_res {
        Some(res) => res,
        None => {
            log::error!(
                "Tool loop ended without a final response for guild {}",
                guild_id
            );
            msg.reply(
                ctx.http.clone(),
                "Sorry, I couldn't process that - something went wrong on my end.",
            )
            .await?;
            return Err("Tool loop ended without a final response".into());
        }
    };

    let mut message = res.message.content.clone();
    if message.trim().is_empty() {
        message = "AI returned no response.".to_string();
    }

    let chunks = chunk_message(&message, DISCORD_MESSAGE_LIMIT);

    // Only the first chunk threads off the original message; the rest are follow-up sends in the
    // same channel so we don't try to "reply" to our own reply repeatedly.
    let mut sent = match msg.reply(ctx.http.clone(), &chunks[0]).await {
        Ok(sent) => sent,
        Err(e) => {
            log::warn!(
                "Reply failed (original message likely deleted) in channel {}: {}. Falling back to plain send.",
                msg.channel_id,
                e
            );
            msg.channel_id.say(ctx.http.clone(), &chunks[0]).await?
        }
    };

    // continue the message in the same channel
    for chunk in &chunks[1..] {
        sent = msg.channel_id.say(ctx.http.clone(), chunk).await?;
    }

    let response_doc = doc! {
        "content": &message,
        "author": {
            "id": "Assistant",
            "name": "Assistant",
        },
        "timestamp": Utc::now().timestamp(),
        "human_time": Utc::now().to_rfc3339(),
        "channel": {
            "id": msg.channel_id.to_string(),
            "name": channel_name,
        },
        "guild": {
            "owner": { "id": guild_owner_id }
        },
        "role": "ASSISTANT".to_string(),
        "message": {
            "id": sent.id.to_string(),
            "replying_to": { "id": msg.id.to_string() }
        }
    };

    if let Err(e) = messages_col
        .update_one(
            doc! { "guild_id": guild_id.to_string() },
            doc! { "$push": { "messages": response_doc } },
        )
        .await
    {
        // Don't fail as it won't do anything, but still log the error.
        log::error!(
            "Failed to persist assistant response for guild {}: {}",
            guild_id,
            e
        );
    }

    log::info!("Finished mention in guild {}", guild_id);
    Ok(())
}
