use crate::vision::{attachments::VisionImage, context::VISION_INSTRUCTIONS};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    Openai,
    Anthropic,
    Gemini,
}

impl AiProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(Self::Openai),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

pub struct ProviderResponse {
    pub text: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ProviderError {
    InvalidKey,
    RateLimited,
    Unavailable,
    InvalidResponse,
    UnsupportedModel,
}

pub fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(35))
        .user_agent("OmniCore/1.0 (+https://discordbot.omnicoretech.org)")
        .redirect(Policy::none())
        .build()
        .expect("HTTP client")
}

fn classify(status: StatusCode) -> ProviderError {
    match status.as_u16() {
        401 | 403 => ProviderError::InvalidKey,
        429 => ProviderError::RateLimited,
        _ => ProviderError::Unavailable,
    }
}

async fn body(response: reqwest::Response) -> Result<Value, ProviderError> {
    if !response.status().is_success() {
        return Err(classify(response.status()));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > 512_000 {
            return Err(ProviderError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)
}

fn valid_model(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Conservative allowlist of current text-output model families with image
/// input. The account's model list is checked separately when owners select a
/// model; this check prevents text-only/specialized models from being billed.
pub fn supports_vision(provider: AiProvider, model: &str) -> bool {
    if !valid_model(model) {
        return false;
    }
    match provider {
        AiProvider::Openai => {
            let family = ["gpt-4o", "gpt-4.1", "gpt-5", "gpt-6", "o1", "o3", "o4"]
                .iter()
                .any(|prefix| model == *prefix || model.starts_with(&format!("{prefix}-")));
            family
                && ![
                    "realtime",
                    "audio",
                    "transcribe",
                    "tts",
                    "image",
                    "embedding",
                    "search",
                ]
                .iter()
                .any(|kind| model.contains(kind))
        }
        AiProvider::Anthropic => {
            model.starts_with("claude-3")
                || model.starts_with("claude-4")
                || model.starts_with("claude-5")
                || [
                    "claude-sonnet-",
                    "claude-opus-",
                    "claude-haiku-",
                    "claude-fable-",
                ]
                .iter()
                .any(|prefix| model.starts_with(prefix))
        }
        AiProvider::Gemini => {
            model.starts_with("gemini-")
                && ![
                    "-image",
                    "-tts",
                    "-embedding",
                    "-audio",
                    "-live",
                    "-transcribe",
                ]
                .iter()
                .any(|kind| model.contains(kind))
        }
    }
}

fn vision_payload(
    provider: AiProvider,
    model: &str,
    prompt: &str,
    max_output_tokens: u32,
    images: &[VisionImage],
) -> Value {
    match provider {
        AiProvider::Openai => {
            let mut content = vec![json!({"type": "input_text", "text": prompt})];
            for image in images {
                let data = STANDARD.encode(image.bytes.as_slice());
                content.push(json!({"type": "input_image", "image_url":
                    format!("data:{};base64,{data}", image.mime), "detail": "auto"}));
            }
            json!({"model": model, "instructions": VISION_INSTRUCTIONS,
                "input": [{"role": "user", "content": content}], "store": false,
                "max_output_tokens": max_output_tokens})
        }
        AiProvider::Anthropic => {
            let mut content = Vec::with_capacity(images.len() * 2 + 1);
            for (index, image) in images.iter().enumerate() {
                content.push(json!({"type": "text", "text": format!("Image {}:", index + 1)}));
                content.push(json!({"type": "image", "source": {"type": "base64",
                    "media_type": image.mime, "data": STANDARD.encode(image.bytes.as_slice())}}));
            }
            content.push(json!({"type": "text", "text": prompt}));
            json!({"model": model, "system": VISION_INSTRUCTIONS,
                "max_tokens": max_output_tokens,
                "messages": [{"role": "user", "content": content}]})
        }
        AiProvider::Gemini => {
            let mut parts = Vec::with_capacity(images.len() + 1);
            for image in images {
                parts.push(json!({"inlineData": {"mimeType": image.mime,
                    "data": STANDARD.encode(image.bytes.as_slice())}}));
            }
            parts.push(json!({"text": prompt}));
            json!({"systemInstruction": {"parts": [{"text": VISION_INSTRUCTIONS}]},
                "contents": [{"role": "user", "parts": parts}],
                "generationConfig": {"maxOutputTokens": max_output_tokens}})
        }
    }
}

pub async fn complete_vision(
    http: &Client,
    provider: AiProvider,
    key: &str,
    model: &str,
    prompt: &str,
    max_output_tokens: u32,
    images: &[VisionImage],
) -> Result<ProviderResponse, ProviderError> {
    if !supports_vision(provider, model) || images.is_empty() {
        return Err(ProviderError::UnsupportedModel);
    }
    let endpoint = match provider {
        AiProvider::Openai => "https://api.openai.com/v1/responses".to_string(),
        AiProvider::Anthropic => "https://api.anthropic.com/v1/messages".to_string(),
        AiProvider::Gemini => format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
        ),
    };
    complete_vision_at(
        http,
        provider,
        key,
        model,
        prompt,
        max_output_tokens,
        images,
        &endpoint,
    )
    .await
}

/// Production endpoints are fixed above; tests use a loopback mock.
#[allow(clippy::too_many_arguments)]
async fn complete_vision_at(
    http: &Client,
    provider: AiProvider,
    key: &str,
    model: &str,
    prompt: &str,
    max_output_tokens: u32,
    images: &[VisionImage],
    endpoint: &str,
) -> Result<ProviderResponse, ProviderError> {
    let payload = vision_payload(provider, model, prompt, max_output_tokens, images);
    let request = match provider {
        AiProvider::Openai => http.post(endpoint).bearer_auth(key).json(&payload),
        AiProvider::Anthropic => http
            .post(endpoint)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload),
        AiProvider::Gemini => http
            .post(endpoint)
            .header("x-goog-api-key", key)
            .json(&payload),
    };
    let response = request
        .send()
        .await
        .map_err(|_| ProviderError::Unavailable)?;
    parse_response(provider, &body(response).await?)
}

pub async fn models(
    http: &Client,
    provider: AiProvider,
    key: &str,
) -> Result<Vec<String>, ProviderError> {
    let request = match provider {
        AiProvider::Openai => http
            .get("https://api.openai.com/v1/models")
            .bearer_auth(key),
        AiProvider::Anthropic => http
            .get("https://api.anthropic.com/v1/models?limit=100")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        AiProvider::Gemini => http
            .get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000")
            .header("x-goog-api-key", key),
    };
    let response = request
        .send()
        .await
        .map_err(|_| ProviderError::Unavailable)?;
    let value = body(response).await?;
    parse_models(provider, &value)
}

fn parse_models(provider: AiProvider, value: &Value) -> Result<Vec<String>, ProviderError> {
    let items = match provider {
        AiProvider::Gemini => value.get("models"),
        _ => value.get("data"),
    }
    .and_then(Value::as_array)
    .ok_or(ProviderError::InvalidResponse)?;
    let mut result = Vec::new();
    for item in items {
        let id = match provider {
            AiProvider::Gemini => item
                .get("name")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("models/")),
            _ => item.get("id").and_then(Value::as_str),
        };
        let Some(id) = id else {
            continue;
        };
        let supported = match provider {
            AiProvider::Openai => id.starts_with("gpt-") || id.starts_with('o'),
            AiProvider::Anthropic => {
                item.get("lifecycle").and_then(Value::as_str) != Some("retired")
            }
            AiProvider::Gemini => item
                .get("supportedGenerationMethods")
                .or_else(|| item.get("supportedActions"))
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|m| m.as_str() == Some("generateContent"))
                }),
        };
        if valid_model(id) && supported {
            result.push(id.to_string());
        }
    }
    result.sort();
    result.dedup();
    if result.is_empty() {
        return Err(ProviderError::UnsupportedModel);
    }
    Ok(result)
}

pub async fn complete(
    http: &Client,
    provider: AiProvider,
    key: &str,
    model: &str,
    prompt: &str,
    max_output_tokens: u32,
) -> Result<ProviderResponse, ProviderError> {
    if !valid_model(model) {
        return Err(ProviderError::UnsupportedModel);
    }
    let request = match provider {
        AiProvider::Openai => http.post("https://api.openai.com/v1/responses").bearer_auth(key)
            .json(&json!({"model": model, "input": prompt, "store": false, "max_output_tokens": max_output_tokens})),
        AiProvider::Anthropic => http.post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key).header("anthropic-version", "2023-06-01")
            .json(&json!({"model": model, "max_tokens": max_output_tokens,
                "messages": [{"role": "user", "content": prompt}]})),
        AiProvider::Gemini => http.post(format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"))
            .header("x-goog-api-key", key)
            .json(&json!({"contents": [{"role": "user", "parts": [{"text": prompt}]}],
                "generationConfig": {"maxOutputTokens": max_output_tokens}})),
    };
    let response = request
        .send()
        .await
        .map_err(|_| ProviderError::Unavailable)?;
    let value = body(response).await?;
    parse_response(provider, &value)
}

fn parse_response(provider: AiProvider, value: &Value) -> Result<ProviderResponse, ProviderError> {
    let text = match provider {
        AiProvider::Openai => value
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        AiProvider::Anthropic => value
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        AiProvider::Gemini => value
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    if text.is_empty() {
        return Err(ProviderError::InvalidResponse);
    }
    let (input, output) = match provider {
        AiProvider::Openai => (
            value.pointer("/usage/input_tokens"),
            value.pointer("/usage/output_tokens"),
        ),
        AiProvider::Anthropic => (
            value.pointer("/usage/input_tokens"),
            value.pointer("/usage/output_tokens"),
        ),
        AiProvider::Gemini => (
            value.pointer("/usageMetadata/promptTokenCount"),
            value.pointer("/usageMetadata/candidatesTokenCount"),
        ),
    };
    Ok(ProviderResponse {
        text,
        input_tokens: input.and_then(Value::as_i64),
        output_tokens: output.and_then(Value::as_i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};
    use std::sync::{Arc, Mutex};
    use zeroize::Zeroizing;
    #[test]
    fn parses_each_provider_without_secrets() {
        let openai = parse_response(AiProvider::Openai, &json!({"output":[{"content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":3,"output_tokens":4}})).unwrap();
        let anthropic = parse_response(AiProvider::Anthropic, &json!({"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":2,"output_tokens":1}})).unwrap();
        let gemini = parse_response(AiProvider::Gemini, &json!({"candidates":[{"content":{"parts":[{"text":"hey"}]}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":6}})).unwrap();
        assert_eq!(
            (
                openai.text.as_str(),
                anthropic.text.as_str(),
                gemini.text.as_str()
            ),
            ("hello", "hi", "hey")
        );
        assert_eq!(gemini.output_tokens, Some(6));
        assert!(!valid_model("../../evil"));
    }

    #[test]
    fn discovers_text_models_from_each_provider_and_rejects_unsafe_ids() {
        let openai = json!({"data":[{"id":"gpt-test"},{"id":"../../bad"}]});
        let anthropic =
            json!({"data":[{"id":"claude-test"},{"id":"claude-old","lifecycle":"retired"}]});
        let gemini = json!({"models":[{"name":"models/gemini-test","supportedGenerationMethods":["generateContent"]},{"name":"models/embed-test","supportedGenerationMethods":["embedContent"]}]});
        assert_eq!(
            parse_models(AiProvider::Openai, &openai).unwrap(),
            vec!["gpt-test"]
        );
        assert_eq!(
            parse_models(AiProvider::Anthropic, &anthropic).unwrap(),
            vec!["claude-test"]
        );
        assert_eq!(
            parse_models(AiProvider::Gemini, &gemini).unwrap(),
            vec!["gemini-test"]
        );
        assert_eq!(
            classify(StatusCode::TOO_MANY_REQUESTS),
            ProviderError::RateLimited
        );
        assert_eq!(
            classify(StatusCode::UNAUTHORIZED),
            ProviderError::InvalidKey
        );
    }

    #[test]
    fn vision_capability_excludes_text_only_and_specialized_models() {
        assert!(supports_vision(AiProvider::Openai, "gpt-6-astra"));
        assert!(supports_vision(AiProvider::Anthropic, "claude-sonnet-5-5"));
        assert!(supports_vision(AiProvider::Gemini, "gemini-3.8-flash"));
        assert!(!supports_vision(AiProvider::Openai, "gpt-3.5-turbo"));
        assert!(!supports_vision(AiProvider::Openai, "gpt-4o-mini-tts"));
        assert!(!supports_vision(AiProvider::Gemini, "gemini-embedding-001"));
        assert!(!supports_vision(AiProvider::Gemini, "gemini-3.8-live"));
    }

    #[tokio::test]
    async fn mocked_vision_apis_receive_only_current_images() {
        let image = VisionImage {
            mime: "image/png",
            bytes: Zeroizing::new(vec![137, 80, 78, 71]),
        };
        let fixtures = [
            (
                AiProvider::Openai,
                "gpt-6-astra",
                json!({"output":[{"content":[{"text":"car"}]}],"usage":{"input_tokens":10,"output_tokens":2}}),
            ),
            (
                AiProvider::Anthropic,
                "claude-sonnet-5-5",
                json!({"content":[{"text":"car"}],"usage":{"input_tokens":10,"output_tokens":2}}),
            ),
            (
                AiProvider::Gemini,
                "gemini-3.8-flash",
                json!({"candidates":[{"content":{"parts":[{"text":"car"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2}}),
            ),
        ];
        for (provider, model, fixture) in fixtures {
            let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
            let captured = seen.clone();
            let app = Router::new().route(
                "/",
                post(move |Json(payload): Json<Value>| {
                    let seen = captured.clone();
                    let fixture = fixture.clone();
                    async move {
                        seen.lock().unwrap().push(payload);
                        Json(fixture)
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let http = Client::builder().no_proxy().build().unwrap();
            let result = complete_vision_at(
                &http,
                provider,
                "mock-key",
                model,
                "What car is this?",
                200,
                std::slice::from_ref(&image),
                &url,
            )
            .await
            .unwrap();
            assert_eq!(result.text, "car");
            assert_eq!(result.input_tokens, Some(10));
            let requests = seen.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let payload = &requests[0];
            assert!(!payload.to_string().contains("mock-key"));
            let part = match provider {
                AiProvider::Openai => &payload["input"][0]["content"][1],
                AiProvider::Anthropic => &payload["messages"][0]["content"][1],
                AiProvider::Gemini => &payload["contents"][0]["parts"][0],
            };
            assert!(part.to_string().contains("iVBORw"));
            server.abort();
        }
    }

    #[tokio::test]
    async fn vision_provider_rate_limit_is_reported_without_a_fake_answer() {
        let app = Router::new().route("/", post(|| async { StatusCode::TOO_MANY_REQUESTS }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let image = VisionImage {
            mime: "image/png",
            bytes: Zeroizing::new(vec![137, 80, 78, 71]),
        };
        let http = Client::builder().no_proxy().build().unwrap();
        let result = complete_vision_at(
            &http,
            AiProvider::Openai,
            "mock-key",
            "gpt-6-astra",
            "Describe this image",
            200,
            &[image],
            &url,
        )
        .await;
        assert!(matches!(result, Err(ProviderError::RateLimited)));
        server.abort();
    }
}
