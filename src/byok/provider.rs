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
}
