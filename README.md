# OmniCore Discord Bot

Secure server-owner AI pairing, per-guild provider settings, and moderation alert operations are documented in [docs/byok-operations.md](docs/byok-operations.md).

OmniCore is a Discord bot written in Rust. Local Ollama powers the existing conversational AI and AutoMod paths. Server owners can separately connect their own OpenAI, Anthropic, or Gemini API account for guild-scoped `/ask` requests.

## Build and run

Install a current Rust toolchain, then run `cargo test` and `cargo build --release`. The binary is `target/release/omnicore_bot`; the Dockerfile builds the same Rust application.

Copy `.env.example` to a private runtime configuration file and replace its example values. Never commit Discord tokens, OAuth client secrets, provider API keys, MongoDB credentials, or the BYOK encryption master key. The master key must remain stable across restarts so existing encrypted provider keys remain readable. See [the operations guide](docs/byok-operations.md) for the production setup and rollback steps.

The public static site is in [`website/`](website/). The dedicated site Nginx configuration and Compose override are in [`deploy/nginx/`](deploy/nginx/); the `/connect/` route proxies to the Rust pairing portal on the private Docker network.

## Data retention

New AI turns are stored in `ai_turns` with a 30-day MongoDB TTL index. Guild settings and active channel-lock mappings do not expire by age. See `docs/RETENTION.md` for the production audit, safe legacy migration, and deletion behavior. Do not run the migration's apply mode before reviewing a backup and its dry-run counts.

## Automatic moderation

AutoMod is enabled by default for new and existing guilds. The server owner can view and change it with `/automod` subcommands. It uses a local candidate screen and the configured Ollama model, with strict confidence policy and a code-level 48-hour automatic timeout limit. Selected incident message text is kept in memory for no more than approximately 60 seconds and is excluded from normal AI chat history. See [docs/AUTOMOD.md](docs/AUTOMOD.md) for commands, privacy behavior, permissions, and rollout guidance.

The first confirmed violation warns the member in a public reply to the flagged message. Another confirmed violation by the same member in the same server within 24 hours triggers a temporary timeout when enabled. Confirmed warnings and successful timeouts notify the guild owner and configured bot operators. The moderation analytics and delivery records contain metadata only, with a 90-day expiry; incident message text is not saved there.

The server owner can also request a direct mute in chat: `@OmniCore mute @member for 10m`. OmniCore checks current ownership, target, duration, bot permissions, and role hierarchy before applying a Discord timeout. A reply to the member's message can supply the target. This direct owner action is capped at two days.

## Server-owned AI

The guild owner starts with `/ai setup`, opens the short-lived HTTPS link, signs in with Discord, and enters a provider key in the authenticated portal. Keys are encrypted per guild. `/ai status`, `/ai model`, `/ai usage`, `/ai limits`, `/ai channel`, `/ai role`, and `/ai disconnect` manage access and usage. Members can use `/ask` where the owner permits it. The bot's local AutoMod model does not charge the guild's provider account.

Image questions addressed to OmniCore can use images attached to the invoking message or to its exact direct reply target. See [image recognition and privacy rules](docs/VISION.md), including `/ai vision` settings and owner-only DM opt-in.

## AI usage in development

AI tools have assisted with parts of the code and documentation. The project policy is that a human reviews code and documentation.
