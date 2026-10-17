# OmniCore retention audit and rollout

Audited and deployed 2026-10-04. The bot uses the Rust MongoDB driver directly; there are no Mongoose schemas. The initial inventory was read-only; the reviewed migration was applied after a verified backup and matching preflight counts.

| Collection / model | Data and purpose | Temporary? | Before change | Applied behavior | TTL? / reason |
| --- | --- | --- | --- | --- | --- |
| `messages` | Former one-document-per-guild embedded AI history. | Yes | No expiry; 3 documents with 301 turns. User turns had string timestamps; assistant/tool turns had BSON Int64 Unix seconds. | All 301 turns migrated using original timestamps; collection now has 0 documents. | No: TTL would expire the whole guild document and cannot enforce per-turn age. |
| `ai_turns` | One-document-per-turn AI history. | Yes | Collection absent. | 301 migrated turns at verification; each has BSON Date `createdAt` and `expiresAt` 30 days apart. New turns use the same fields. | Yes: single-field `expiresAt` index with `expireAfterSeconds: 0`, verified live. |
| `per_guild_settings` | Guild ID, prefix, AI approval, custom prompt. Needed for installed servers. | No | 11 documents, no dates or TTL. | Keep while installed; 12 documents at verification after guild creation initialized a missing default; delete on confirmed guild removal or verified server request. | No: expiration would break configuration. |
| `locked_channels` | Guild, channel, and lock-role IDs needed to unlock channels. | No | Collection absent in the running database; code creates it when a channel is locked. | Keep while installed and locked; delete on unlock or confirmed guild removal. | No: expiration would lose active lock state. |

No MongoDB command, debug, error, audit, warning, infraction, session, cache, or moderation-case collection exists in the audited source or the running database. Moderation commands act through Discord; the bot does not save a moderation case history. Temporary AI context is the same `ai_turns` history, with a 40-turn context window and 30-day storage limit; there is no separate seven-day cache to expire. Serenity's in-memory cache uses a 10-minute TTL.

The old bot wrote `log/app.log` and console logs without an age limit. The changed bot writes only to stdout and reduces identifier logging. Existing log files and Docker/Nginx logs remain subject to their host retention settings and need separate operator review; the bot does not claim to delete them after 30 days.

## Index and migration safety

`ensure_indexes` runs once at startup. It checks all existing `ai_turns.expiresAt` values are BSON Dates, rejects conflicting indexes, and refuses to attach a new TTL index to a nonempty collection. On a new empty collection it creates `ai_turns_expires_at_ttl` once and logs verification. It does not create TTL indexes on permanent or legacy collections.

The image includes `retention_audit` and `retention_migrate` binaries so they can run inside the Docker network using the bot container's existing `MONGO_URI` environment variable. `docker exec omnicore-discord-bot ./retention_audit` is read-only. `docker exec omnicore-discord-bot ./retention_migrate` is a dry run. Apply mode requires `--apply --reviewed-backup-and-counts EXPECTED_DOCUMENTS EXPECTED_RECENT_TURNS EXPECTED_EXPIRED_TURNS` and a verified TTL index. It scans the whole collection first and refuses to write if the counts differ from the reviewed dry run or any timestamp is unknown. It gives recent legacy turns expiry based on their original timestamp, never a fresh 30 days. For each fully parseable guild document, it upserts recent turns under deterministic IDs, then deletes the old document only if its message array has not changed. It does not insert already expired turns. If an apply step fails, stop and inspect before retrying; deterministic IDs make the upserts repeatable.

The production dry run and apply preflight both reported **3 legacy guild documents, 301 recent turns, 0 older turns, 0 unknown timestamps, and 0 skipped documents**. The migration moved all 301 turns and removed the three legacy documents. Post-migration checks found 301 BSON Date expiry fields, 301 exact 30-day offsets, 0 legacy documents, and one TTL index. The bot remained running with no startup errors or restarts.

Rollout completed in this order: backed up and verified the original database; stopped the old bot and started the new image; verified the new TTL index; backed up the pre-migration database; ran the dry run and exact-count apply; verified all 301 turns and settings; published privacy, terms, support, and command pages. Both public legal URLs returned HTTP 200 with the published content. The temporary database backups and stopped old container were removed after verification so they do not become extra long-lived copies of AI history.

## Deletion and authorization

`delete_guild_data(guild_id)` removes settings, lock mappings, old AI history, and new AI turns only for the supplied guild. `GuildDelete` invokes it only when Discord reports an actual removal, not temporary unavailability. Administrators can still use `/delete_memory` for their own guild's AI history. `/delete_my_data` uses only the invoking account's ID and removes new prompts and their linked AI/tool responses, plus old prompts and assistant replies linked by message ID. Legacy tool output and free-text mentions require manual review. Internal `delete_user_data` and `delete_guild_data` are available for verified support requests; no public command accepts an arbitrary target user or guild ID.

Existing records for guilds that left before this handler was deployed are not automatically discovered. Reconcile them separately against an authoritative installed-guild list before deleting anything. In-flight AI requests during a deletion are a residual race that should be checked during an operational deletion review.

## Privacy review

The bot's AI provider is the configured Ollama endpoint. Prompts, recent context, and associated Discord metadata are sent to it; Wikipedia tool queries go to Wikimedia. AI history includes message content, user/guild/channel IDs, display names, channel names, message IDs, and a repeated guild owner ID. The owner ID and names are candidates for further minimization after checking their effect on AI behavior. The code no longer requests unused presence and automod gateway intents. Local and infrastructure logs may still contain errors or identifiers and need a separate retention setting. Review MongoDB encryption at rest and service-provider controls before claiming full Discord policy compliance.
