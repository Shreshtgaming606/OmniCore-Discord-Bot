use dotenvy::dotenv;
use once_cell::sync::OnceCell;
use std::env;

pub(crate) static DISCORD_TOKEN: OnceCell<String> = OnceCell::new();
pub(crate) static MONGO_URI: OnceCell<String> = OnceCell::new();
pub(crate) static OLLAMA_BASE_URL: OnceCell<String> = OnceCell::new();
pub(crate) static OLLAMA_MODEL: OnceCell<String> = OnceCell::new();
pub(crate) static BOT_OWNERS: OnceCell<Vec<u64>> = OnceCell::new();
pub(crate) static START_SHARD: OnceCell<u32> = OnceCell::new();
pub(crate) static END_SHARD: OnceCell<u32> = OnceCell::new();
pub(crate) static TOTAL_SHARDS: OnceCell<u32> = OnceCell::new();

pub(crate) fn init_config() {
    dotenv().ok();

    DISCORD_TOKEN
        .set(env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN isn't set."))
        .unwrap();
    MONGO_URI
        .set(env::var("MONGO_URI").expect("MONGO_URI isn't set."))
        .unwrap();
    OLLAMA_BASE_URL
        .set(env::var("OLLAMA_BASE_URL").expect("OLLAMA_BASE_URL isn't set."))
        .unwrap();
    OLLAMA_MODEL
        .set(env::var("OLLAMA_MODEL").expect("OLLAMA_MODEL isn't set."))
        .unwrap();

    if OLLAMA_BASE_URL.get().unwrap().ends_with("/") {
        log::error!("OLLAMA_BASE_URL must not end with a slash");
        std::process::exit(78); // 78 is the exit code for config errors
    }

    let bot_owners = env::var("BOT_OWNERS")
        .unwrap_or("1157083515486220429,1109540816013234256".to_string()) // First is l1fe_wyra and second is Shreshtgaming606.
        .split(',')
        .map(|s| s.trim().parse::<u64>().expect("Invalid BOT_OWNERS value"))
        .collect::<Vec<u64>>();
    BOT_OWNERS.set(bot_owners).unwrap();

    let start_shard = env::var("START_SHARD")
        .unwrap_or("0".to_string())
        .parse::<u32>()
        .expect("Invalid START_SHARD value");
    START_SHARD.set(start_shard).unwrap();

    let end_shard = env::var("END_SHARD")
        .unwrap_or("4".to_string())
        .parse::<u32>()
        .expect("Invalid END_SHARD value");
    END_SHARD.set(end_shard).unwrap();

    let total_shards = env::var("TOTAL_SHARDS")
        .unwrap_or("5".to_string())
        .parse::<u32>()
        .expect("Invalid TOTAL_SHARDS value");
    TOTAL_SHARDS.set(total_shards).unwrap();

    log::info!("Config initialized!");
}
