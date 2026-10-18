//! Release the Ollama model selected for a local voice session after it ends.

use super::VoiceSettings;
use crate::config;
use std::time::Duration;

pub fn model_for(settings: &VoiceSettings) -> Option<String> {
    if settings.provider != "ollama" {
        return None;
    }
    if settings.model.is_empty() {
        config::OLLAMA_MODEL.get().cloned()
    } else {
        Some(settings.model.clone())
    }
}

fn model_loaded(body: &serde_json::Value, model: &str) -> Option<bool> {
    let models = body.get("models")?.as_array()?;
    Some(models.iter().any(|entry| {
        entry
            .get("name")
            .or_else(|| entry.get("model"))
            .and_then(serde_json::Value::as_str)
            == Some(model)
    }))
}

/// Return true when the model is confirmed absent from Ollama's loaded models.
pub async fn unload_model(model: &str) -> bool {
    if model.is_empty() {
        return true;
    }
    let Some(base) = config::OLLAMA_BASE_URL.get() else {
        return false;
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    let ps_url = format!("{base}/api/ps");
    let Ok(response) = client.get(&ps_url).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(body) = response.json::<serde_json::Value>().await else {
        return false;
    };
    let Some(loaded) = model_loaded(&body, model) else {
        return false;
    };
    if !loaded {
        return true;
    }
    let Ok(response) = client
        .post(format!("{base}/api/generate"))
        .json(&serde_json::json!({"model": model, "prompt": "", "stream": false, "keep_alive": 0}))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    for _ in 0..5 {
        let Ok(response) = client.get(&ps_url).send().await else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        let Ok(body) = response.json::<serde_json::Value>().await else {
            return false;
        };
        let Some(loaded) = model_loaded(&body, model) else {
            return false;
        };
        if !loaded {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_voice_has_an_ollama_model_to_unload() {
        let mut settings = VoiceSettings {
            provider: "gemini".to_owned(),
            model: "qwen3.5:4b".to_owned(),
            ..VoiceSettings::default()
        };
        assert_eq!(model_for(&settings), None);
        settings.provider = "ollama".to_owned();
        assert_eq!(model_for(&settings).as_deref(), Some("qwen3.5:4b"));
    }

    #[test]
    fn unload_confirmation_requires_valid_status() {
        let loaded = serde_json::json!({"models": [{"name": "qwen3.5:4b"}]});
        assert_eq!(model_loaded(&loaded, "qwen3.5:4b"), Some(true));
        assert_eq!(model_loaded(&loaded, "other:latest"), Some(false));
        assert_eq!(model_loaded(&serde_json::json!({}), "qwen3.5:4b"), None);
    }
}
