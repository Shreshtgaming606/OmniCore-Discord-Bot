# Data retention and migration

OmniCore uses the Rust MongoDB driver. The bot creates indexes at startup. New AI turns are individual `ai_turns` documents with BSON date fields and a 30-day TTL. Guild settings and active channel lock mappings remain while needed; they are removed after confirmed guild departure or a verified deletion request. AutoMod warning state is used for a 24-hour repeat window. Moderation events, delivery metadata, and AI/voice usage records expire after up to 90 days. Raw AutoMod incident text, image bytes, and raw voice audio are not stored as those records. Host logs and Discord messages have separate retention governed by their operators.

The old `messages` collection stored an embedded array of AI turns per guild. MongoDB TTL on that collection would expire a whole guild document, not individual turns, so a dedicated migration is provided. A fresh installation has no legacy data and needs no migration.

## Legacy migration procedure

1. Back up MongoDB and verify restoration before changing records.
2. Run `retention_audit` read-only to inspect the legacy collection and current TTL index.
3. Run `retention_migrate` without arguments for a dry run. Record its expected document, recent-turn, and expired-turn counts.
4. Review all unknown timestamps and backup status. Apply only with `--apply --reviewed-backup-and-counts EXPECTED_DOCUMENTS EXPECTED_RECENT_TURNS EXPECTED_EXPIRED_TURNS`. The tool refuses to write if counts differ or timestamps are unknown.
5. Re-run the audit and verify turn dates, expiry index, and absence of legacy documents before deleting any backup.

Migration preserves original turn timestamps. It uses deterministic IDs for upserts and removes a legacy guild document only if its message array is unchanged. If any step fails, stop and inspect before retrying. The apply command is deliberately not part of normal startup or CI.

`/delete_memory` removes AI history for a server; `/delete_my_data` removes turns linked to the requesting account. Historical free-text mentions or old tool output can require manual review. When Discord reports a true guild removal, the bot deletes that guild's settings and data; temporary unavailability does not trigger deletion. Hosts must separately manage MongoDB backups and infrastructure logs.
