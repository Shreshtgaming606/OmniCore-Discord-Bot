use mongodb::bson::{Document, doc};
use std::time::Duration;

pub const MAX_AUTO_TIMEOUT: Duration = Duration::from_secs(48 * 60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sensitivity {
    Low,
    Normal,
    High,
}

impl Sensitivity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AutoModConfig {
    pub enabled: bool,
    pub warnings_enabled: bool,
    pub timeouts_enabled: bool,
    pub sensitivity: Sensitivity,
    pub moderate_timeout: Duration,
    pub serious_timeout: Duration,
    pub severe_timeout: Duration,
    pub extreme_timeout: Duration,
    pub notify_owner: bool,
    pub notify_developers: bool,
    pub log_channel_id: Option<u64>,
}

impl Default for AutoModConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warnings_enabled: true,
            timeouts_enabled: true,
            sensitivity: Sensitivity::Normal,
            moderate_timeout: Duration::from_secs(600),
            serious_timeout: Duration::from_secs(3600),
            severe_timeout: Duration::from_secs(21600),
            extreme_timeout: Duration::from_secs(86400),
            notify_owner: true,
            notify_developers: true,
            log_channel_id: None,
        }
    }
}

impl AutoModConfig {
    pub fn from_guild_document(guild: Option<&Document>) -> Self {
        let mut result = Self::default();
        let Some(guild) = guild else {
            return result;
        };
        let doc = match guild.get_document("automod") {
            Ok(doc) => doc,
            Err(_) if guild.contains_key("automod") => {
                result.enabled = false;
                return result;
            }
            Err(_) => return result,
        };
        result.enabled = match doc.get_bool("enabled") {
            Ok(value) => value,
            Err(_) if doc.contains_key("enabled") => false,
            Err(_) => result.enabled,
        };
        result.warnings_enabled = doc
            .get_bool("warnings_enabled")
            .unwrap_or(result.warnings_enabled);
        result.timeouts_enabled = doc
            .get_bool("timeouts_enabled")
            .unwrap_or(result.timeouts_enabled);
        result.notify_owner = doc.get_bool("notify_owner").unwrap_or(result.notify_owner);
        result.notify_developers = doc
            .get_bool("notify_developers")
            .unwrap_or(result.notify_developers);
        result.sensitivity = doc
            .get_str("sensitivity")
            .ok()
            .and_then(Sensitivity::parse)
            .unwrap_or(result.sensitivity);
        result.moderate_timeout =
            read_duration(doc, "moderate_timeout_seconds", result.moderate_timeout);
        result.serious_timeout =
            read_duration(doc, "serious_timeout_seconds", result.serious_timeout);
        result.severe_timeout = read_duration(doc, "severe_timeout_seconds", result.severe_timeout);
        result.extreme_timeout =
            read_duration(doc, "extreme_timeout_seconds", result.extreme_timeout);
        result.log_channel_id = doc
            .get_str("log_channel_id")
            .ok()
            .and_then(|s| s.parse().ok());
        result
    }

    pub fn to_document(&self) -> Document {
        let mut doc = doc! {
            "enabled": self.enabled,
            "warnings_enabled": self.warnings_enabled,
            "timeouts_enabled": self.timeouts_enabled,
            "sensitivity": self.sensitivity.name(),
            "moderate_timeout_seconds": capped(self.moderate_timeout).as_secs() as i64,
            "serious_timeout_seconds": capped(self.serious_timeout).as_secs() as i64,
            "severe_timeout_seconds": capped(self.severe_timeout).as_secs() as i64,
            "extreme_timeout_seconds": capped(self.extreme_timeout).as_secs() as i64,
            "notify_owner": self.notify_owner,
            "notify_developers": self.notify_developers,
            "updated_at": mongodb::bson::DateTime::now(),
        };
        if let Some(id) = self.log_channel_id {
            doc.insert("log_channel_id", id.to_string());
        }
        doc
    }
}

pub fn capped(duration: Duration) -> Duration {
    duration.min(MAX_AUTO_TIMEOUT)
}

fn read_duration(doc: &Document, name: &str, default: Duration) -> Duration {
    let value = doc
        .get_i64(name)
        .ok()
        .or_else(|| doc.get_i32(name).ok().map(i64::from));
    match value {
        Some(n) if n > 0 => capped(Duration::from_secs(n as u64)),
        _ => default,
    }
}

pub fn parse_owner_duration(input: &str) -> Result<Duration, &'static str> {
    let strict = regex::Regex::new(
        r"(?i)^\d+(s|sec|secs|seconds?|m|min|mins|minutes?|h|hr|hrs|hours?|d|days?|w|weeks?)$",
    )
    .map_err(|_| "Duration parser unavailable.")?;
    if !strict.is_match(input.trim()) {
        return Err("Use a duration such as 10m, 1h, or 2d.");
    }
    let value = crate::commands::parse_duration(input)
        .map_err(|_| "Use a duration such as 10m, 1h, or 2d.")?;
    if value.is_zero() {
        return Err("Duration must be greater than zero.");
    }
    if value > MAX_AUTO_TIMEOUT {
        return Err("Automatic timeout duration cannot exceed 2 days.");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enabled_by_default_and_corrupt_durations_capped() {
        assert!(AutoModConfig::from_guild_document(None).enabled);
        let guild = doc! {"automod": {"severe_timeout_seconds": 1_000_000_i64, "moderate_timeout_seconds": -4_i64}};
        let config = AutoModConfig::from_guild_document(Some(&guild));
        assert_eq!(config.severe_timeout, MAX_AUTO_TIMEOUT);
        assert_eq!(
            config.moderate_timeout,
            AutoModConfig::default().moderate_timeout
        );
        assert!(parse_owner_duration("3d").is_err());
        assert!(parse_owner_duration("garbage10m").is_err());
        let bad_enabled = doc! {"automod": {"enabled": "false"}};
        assert!(!AutoModConfig::from_guild_document(Some(&bad_enabled)).enabled);
        let bad_document = doc! {"automod": "broken"};
        assert!(!AutoModConfig::from_guild_document(Some(&bad_document)).enabled);
    }
}
