use mongodb::{Client, bson::doc, options::ClientOptions};
use tokio::sync::OnceCell;

/// Shared MongoDB client used by the bot.
pub static CLIENT: OnceCell<mongodb::Client> = OnceCell::const_new();

/// Placeholder for collection/index setup logic.
pub async fn ensure_indexes() -> mongodb::error::Result<()> {
    Ok(())
}

/// Connects to MongoDB, verifies the server is reachable, and stores the client.
pub async fn mongo_connect() -> mongodb::error::Result<()> {
    let mongodb_uri = crate::config::MONGO_URI.get().unwrap();

    let client_options = ClientOptions::parse(mongodb_uri)
        .await
        .map_err(|e| mongodb::error::Error::custom(format!("Invalid MongoDB URI: {}", e)))?;
    let client = Client::with_options(client_options)?;
    client
        .database("admin")
        .run_command(doc! { "ping": 1 })
        .await?;

    CLIENT
        .set(client)
        .map_err(|_| mongodb::error::Error::custom("MongoDB client already initialized"))?;

    log::info!("Connected to MongoDB!");
    Ok(())
}

/// Returns a typed handle to a collection in the bot's database.
pub fn get_collection(
    collection_name: &str,
) -> Result<mongodb::Collection<mongodb::bson::Document>, mongodb::error::Error> {
    let client = CLIENT
        .get()
        .ok_or_else(|| mongodb::error::Error::custom("MongoDB client not initialized"))?;
    Ok(client
        .database("omnicore_bot")
        .collection::<mongodb::bson::Document>(collection_name))
}

/// Shuts down the shared MongoDB client if it was initialized.
pub async fn mongo_shutdown() {
    log::info!("Attempting MongoDB shutdown...");

    let client = match CLIENT.get() {
        Some(c) => c.clone(),
        None => {
            log::warn!("mongo_shutdown() called but MongoDB was never initialized.");
            return;
        }
    };

    client.shutdown().await;

    log::info!("MongoDB shutdown complete.");
}
