use dotenvy::dotenv;
use once_cell::sync::OnceCell;
use std::env;

pub(crate) static DISCORD_TOKEN: OnceCell<String> = OnceCell::new();
pub(crate) static MONGO_URI: OnceCell<String> = OnceCell::new();
pub(crate) static OLLAMA_BASE_URL: OnceCell<String> = OnceCell::new();
pub(crate) static OLLAMA_MODEL: OnceCell<String> = OnceCell::new();
pub(crate) static BOT_OWNERS: OnceCell<Vec<u64>> = OnceCell::new();
pub(crate) static DEVELOPER_IDS: OnceCell<Vec<u64>> = OnceCell::new();
pub(crate) static VOICE_BUILTIN_GUILDS: OnceCell<Vec<u64>> = OnceCell::new();
pub(crate) static START_SHARD: OnceCell<u32> = OnceCell::new();
pub(crate) static END_SHARD: OnceCell<u32> = OnceCell::new();
pub(crate) static TOTAL_SHARDS: OnceCell<u32> = OnceCell::new();

fn id_list(value: &str, name: &str, allow_empty: bool) -> Vec<u64> {
    if allow_empty && value.trim().is_empty() {
        return Vec::new();
    }
    let ids: Vec<u64> = value
        .split(',')
        .map(|part| {
            let id = part
                .trim()
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("Invalid {name} value"));
            assert!(id > 0, "{name} IDs must be nonzero");
            id
        })
        .collect();
    assert!(
        !ids.is_empty(),
        "{name} must contain at least one Discord ID"
    );
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "{name} contains duplicate IDs");
    ids
}

pub(crate) fn init_config() {
    dotenv().ok();

    DISCORD_TOKEN
        .set(env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN isn't set."))
        .unwrap();
    MONGO_URI
        .set(env::var("MONGO_URI").expect("MONGO_URI isn't set."))
        .unwrap();
    let ollama_url =
        env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    assert!(
        !ollama_url.ends_with('/'),
        "OLLAMA_BASE_URL must not end with a slash"
    );
    OLLAMA_BASE_URL.set(ollama_url).unwrap();
    OLLAMA_MODEL
        .set(env::var("OLLAMA_MODEL").unwrap_or_default())
        .unwrap();

    // Keep the legacy two-variable form for existing installations.
    let developers = match env::var("OMNICORE_OWNER_IDS") {
        Ok(value) => id_list(&value, "OMNICORE_OWNER_IDS", false),
        Err(_) => {
            let first = env::var("OMNICORE_OWNER_1_ID")
                .expect("Set OMNICORE_OWNER_IDS (or legacy owner IDs)");
            let second = env::var("OMNICORE_OWNER_2_ID")
                .expect("Set OMNICORE_OWNER_IDS (or legacy owner IDs)");
            id_list(&format!("{first},{second}"), "OMNICORE_OWNER_IDS", false)
        }
    };
    let chat_owners = match env::var("BOT_OWNERS") {
        Ok(value) => id_list(&value, "BOT_OWNERS", false),
        Err(_) => developers.clone(),
    };
    DEVELOPER_IDS.set(developers).unwrap();
    BOT_OWNERS.set(chat_owners).unwrap();
    VOICE_BUILTIN_GUILDS
        .set(id_list(
            &env::var("OMNICORE_VOICE_BUILTIN_GUILDS").unwrap_or_default(),
            "OMNICORE_VOICE_BUILTIN_GUILDS",
            true,
        ))
        .unwrap();

    let start_shard = env::var("START_SHARD")
        .unwrap_or_else(|_| "0".into())
        .parse::<u32>()
        .expect("Invalid START_SHARD value");
    let end_shard = env::var("END_SHARD")
        .unwrap_or_else(|_| "1".into())
        .parse::<u32>()
        .expect("Invalid END_SHARD value");
    let total_shards = env::var("TOTAL_SHARDS")
        .unwrap_or_else(|_| "1".into())
        .parse::<u32>()
        .expect("Invalid TOTAL_SHARDS value");
    assert!(
        start_shard < end_shard && end_shard <= total_shards,
        "Shard range must satisfy START_SHARD < END_SHARD <= TOTAL_SHARDS"
    );
    START_SHARD.set(start_shard).unwrap();
    END_SHARD.set(end_shard).unwrap();
    TOTAL_SHARDS.set(total_shards).unwrap();

    log::info!("Config initialized!");
}

#[cfg(test)]
mod tests {
    use super::id_list;

    #[test]
    fn parses_private_id_lists_without_builtin_accounts() {
        assert_eq!(id_list("10, 20", "OWNERS", false), vec![10, 20]);
        assert!(id_list("", "GUILDS", true).is_empty());
        assert!(std::panic::catch_unwind(|| id_list("10,10", "OWNERS", false)).is_err());
        assert!(std::panic::catch_unwind(|| id_list("0", "OWNERS", false)).is_err());
    }
}
