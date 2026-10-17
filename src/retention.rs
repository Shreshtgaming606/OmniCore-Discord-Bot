use mongodb::bson::DateTime;

/// AI turns are individual documents so TTL removes each turn on its own schedule.
pub const AI_HISTORY_DAYS: i64 = 30;
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;

pub fn expires_at(created_at: DateTime) -> DateTime {
    DateTime::from_millis(created_at.timestamp_millis() + AI_HISTORY_DAYS * MILLIS_PER_DAY)
}

pub fn cutoff(now: DateTime) -> DateTime {
    DateTime::from_millis(now.timestamp_millis() - AI_HISTORY_DAYS * MILLIS_PER_DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_retention_is_thirty_days() {
        let created = DateTime::from_millis(1_700_000_000_000);
        assert_eq!(
            expires_at(created).timestamp_millis() - created.timestamp_millis(),
            30 * MILLIS_PER_DAY
        );
        assert_eq!(cutoff(expires_at(created)), created);
    }
}
