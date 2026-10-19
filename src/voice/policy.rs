//! Bot-owner voice access and persistent emergency stop.

use crate::{config, database};
use mongodb::bson::doc;
use poise::serenity_prelude::GuildId;

const ACCESS: &str = "voice_access";
const CONTROL: &str = "voice_control";

pub fn builtin(guild: GuildId) -> bool {
    config::VOICE_BUILTIN_GUILDS
        .get()
        .is_some_and(|guilds| guilds.contains(&guild.get()))
}

pub async fn allowed(guild: GuildId) -> mongodb::error::Result<bool> {
    if builtin(guild) {
        return Ok(true);
    }
    let record = database::get_collection(ACCESS)?
        .find_one(doc! {"_id": guild.to_string()})
        .await?;
    Ok(record
        .as_ref()
        .and_then(|d| d.get_bool("approved").ok())
        .unwrap_or(false))
}

pub async fn approve(guild: GuildId) -> mongodb::error::Result<()> {
    database::get_collection(ACCESS)?
        .update_one(
            doc! {"_id": guild.to_string()},
            doc! {"$set": {"approved": true}},
        )
        .upsert(true)
        .await?;
    Ok(())
}

pub async fn revoke(guild: GuildId) -> mongodb::error::Result<()> {
    database::get_collection(ACCESS)?
        .delete_one(doc! {"_id": guild.to_string()})
        .await?;
    Ok(())
}

pub async fn killed() -> mongodb::error::Result<bool> {
    let record = database::get_collection(CONTROL)?
        .find_one(doc! {"_id": "global"})
        .await?;
    Ok(record
        .as_ref()
        .and_then(|d| d.get_bool("killed").ok())
        .unwrap_or(false))
}

pub async fn set_killed(value: bool) -> mongodb::error::Result<()> {
    database::get_collection(CONTROL)?
        .update_one(doc! {"_id": "global"}, doc! {"$set": {"killed": value}})
        .upsert(true)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guild_without_configured_access_is_not_builtin() {
        assert!(!builtin(GuildId::new(123)));
    }
}
