//! Bot-owner voice access and persistent emergency stop.

use crate::database;
use mongodb::bson::doc;
use poise::serenity_prelude::GuildId;

const ACCESS: &str = "voice_access";
const CONTROL: &str = "voice_control";

// Official Scoggins MS (SMS), Minecraft Server, OmniCore Technologies.
pub const BUILTIN_GUILDS: [u64; 3] = [
    1370968847577780296,
    1485752556981453053,
    1554320480478167050,
];

pub fn builtin(guild: GuildId) -> bool {
    BUILTIN_GUILDS.contains(&guild.get())
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
    fn only_three_named_servers_have_builtin_voice_access() {
        assert_eq!(BUILTIN_GUILDS.len(), 3);
        for id in BUILTIN_GUILDS {
            assert!(builtin(GuildId::new(id)));
        }
        assert!(!builtin(GuildId::new(1547715641501290566))); // SMS NJHS is a separate server.
        assert!(!builtin(GuildId::new(1375850064110551090))); // SMS Orchestra is a separate server.
        assert!(!builtin(GuildId::new(123)));
    }
}
