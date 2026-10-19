use crate::OLLAMA;
use crate::config::{OLLAMA_BASE_URL, OLLAMA_MODEL};
use ollama_rs::Ollama;
use reqwest::Client;

pub(crate) async fn init_ollama() {
    let ollama_model = OLLAMA_MODEL.get().unwrap();
    if ollama_model.is_empty() {
        log::info!("No local Ollama model configured; local AI features are unavailable");
        return;
    }
    // First ping the Ollama server to make sure it's up and running
    let client = Client::new();

    let ollama_base_url = OLLAMA_BASE_URL.get().unwrap();
    let response = client.get(&*ollama_base_url).send().await;
    if let Err(e) = response {
        log::warn!(
            "Ollama is unavailable at startup; AI chat and AutoMod classification will fail safely: {e}"
        );
        return;
    }

    // Create the Ollama client
    let ollama = match Ollama::try_new(ollama_base_url) {
        Ok(client) => client,
        Err(error) => {
            log::warn!("Unable to initialize Ollama: {error}");
            return;
        }
    };

    // List models and validate that the chosen model actually exists
    let models = match ollama.list_local_models().await {
        Ok(models) => models,
        Err(error) => {
            log::warn!("Unable to list Ollama models: {error}");
            return;
        }
    }
    .iter()
    .map(|m| m.name.clone())
    .collect::<Vec<String>>();

    if !models.contains(&ollama_model) {
        log::error!(
            "Ollama model {} does not exist, check your OLLAMA_MODEL",
            ollama_model
        );
        log::error!("Available models: {:?}", models);
        return;
    }

    let _ = OLLAMA.set(ollama);
    log::info!("Ollama model `{}` is valid", ollama_model);
}
