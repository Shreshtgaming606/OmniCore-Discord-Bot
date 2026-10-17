use super::{attachments::VisionImage, context::VISION_INSTRUCTIONS};
use crate::{byok::storage, config, database};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mongodb::bson::{DateTime, doc};
use reqwest::{Client, Response, redirect::Policy};
use serde_json::{Value, json};
use std::{sync::OnceLock, time::Duration};

const RESPONSE_LIMIT: usize = 512_000;

struct LocalResponse {
    text: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(50))
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .expect("Ollama vision client")
    })
}

async fn json_body(mut response: Response) -> Result<Value, &'static str> {
    if !response.status().is_success() {
        return Err("The local image model could not complete this request. Try again shortly.");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The local image model stopped responding.")?
    {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err("The local image model returned an oversized response.");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "The local image model returned an invalid response.")
}

async fn complete_at(
    http: &Client,
    base_url: &str,
    model: &str,
    question: &str,
    images: &[VisionImage],
    max_output_tokens: u32,
) -> Result<LocalResponse, &'static str> {
    let show = http
        .post(format!("{base_url}/api/show"))
        .json(&json!({"model": model}))
        .send()
        .await
        .map_err(|_| "The local image model is unavailable right now.")?;
    let details = json_body(show).await?;
    let supports_vision = details
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some("vision"))
        });
    if !supports_vision {
        return Err(
            "The configured local AI model does not support images. Ask the bot operators to select a vision-capable Ollama model.",
        );
    }
    let encoded: Vec<String> = images
        .iter()
        .map(|image| STANDARD.encode(image.bytes.as_slice()))
        .collect();
    let payload = json!({
        "model": model,
        "stream": false,
        "think": false,
        "options": {"num_ctx": 8192, "num_predict": max_output_tokens},
        "messages": [
            {"role": "system", "content": VISION_INSTRUCTIONS},
            {"role": "user", "content": question, "images": encoded}
        ]
    });
    let response = http
        .post(format!("{base_url}/api/chat"))
        .json(&payload)
        .send()
        .await
        .map_err(|_| "The local image model timed out or is unavailable.")?;
    let value = json_body(response).await?;
    let text = value
        .pointer("/message/content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or("The local image model returned no description. Please try again.")?;
    Ok(LocalResponse {
        text: text.to_owned(),
        input_tokens: value.get("prompt_eval_count").and_then(Value::as_i64),
        output_tokens: value.get("eval_count").and_then(Value::as_i64),
    })
}

async fn record_usage(
    guild: u64,
    model: &str,
    images: usize,
    result: &Result<LocalResponse, &'static str>,
) -> mongodb::error::Result<()> {
    let now = DateTime::now();
    let mut record = doc! {
        "guild_id": guild.to_string(), "provider": "ollama", "model": model,
        "kind": "vision", "status": if result.is_ok() { "succeeded" } else { "failed" },
        "images": images as i32, "occurredAt": now,
        "expiresAt": DateTime::from_millis(now.timestamp_millis() + storage::RETENTION_DAYS * 86_400_000),
    };
    if let Ok(response) = result {
        if let Some(input) = response.input_tokens {
            record.insert("input_tokens", input.max(0));
        }
        if let Some(output) = response.output_tokens {
            record.insert("output_tokens", output.max(0));
        }
    }
    database::get_collection(storage::USAGE)?
        .insert_one(record)
        .await?;
    Ok(())
}

pub async fn complete(
    guild: u64,
    question: &str,
    images: &[VisionImage],
    max_output_tokens: u32,
) -> Result<String, &'static str> {
    let base_url = config::OLLAMA_BASE_URL
        .get()
        .ok_or("The local image model is not configured.")?;
    let model = config::OLLAMA_MODEL
        .get()
        .ok_or("The local image model is not configured.")?;
    let result = complete_at(
        client(),
        base_url,
        model,
        question,
        images,
        max_output_tokens,
    )
    .await;
    if record_usage(guild, model, images.len(), &result)
        .await
        .is_err()
    {
        log::warn!("Local vision usage metadata could not be saved for guild {guild}");
    }
    result.map(|response| response.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};
    use std::sync::{Arc, Mutex};
    use zeroize::Zeroizing;

    #[tokio::test]
    async fn local_vision_sends_only_supplied_images_and_parses_answer() {
        let seen = Arc::new(Mutex::new(None::<Value>));
        let captured = seen.clone();
        let app = Router::new()
            .route("/api/show", post(|| async { Json(json!({"capabilities":["completion","vision"]})) }))
            .route("/api/chat", post(move |Json(payload): Json<Value>| {
                let seen = captured.clone();
                async move {
                    *seen.lock().unwrap() = Some(payload);
                    Json(json!({"message":{"content":"A red square."},"prompt_eval_count":12,"eval_count":5}))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let image = VisionImage {
            mime: "image/png",
            bytes: Zeroizing::new(vec![137, 80, 78, 71]),
        };
        let response = complete_at(client(), &url, "qwen3.5:4b", "What is this?", &[image], 200)
            .await
            .unwrap();
        assert_eq!(response.text, "A red square.");
        assert_eq!(response.input_tokens, Some(12));
        let payload = seen.lock().unwrap();
        let payload = payload.as_ref().unwrap();
        assert_eq!(
            payload["messages"][1]["images"].as_array().unwrap().len(),
            1
        );
        assert_eq!(payload["messages"][1]["images"][0], "iVBORw==");
        assert_eq!(payload["stream"], false);
        server.abort();
    }

    #[tokio::test]
    async fn text_only_local_model_cannot_claim_to_see_an_image() {
        let app = Router::new().route(
            "/api/show",
            post(|| async { Json(json!({"capabilities":["completion"]})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let image = VisionImage {
            mime: "image/png",
            bytes: Zeroizing::new(vec![1]),
        };
        let result = complete_at(client(), &url, "text-only", "What is this?", &[image], 200).await;
        assert!(result.is_err_and(|error| error.contains("does not support images")));
        server.abort();
    }
}
