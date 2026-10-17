# OmniCore BYOK operations

OmniCore remains a Rust bot. The optional provider integration uses the official OpenAI Responses, Anthropic Messages, and Gemini Generate Content APIs. The existing local Ollama paths for AutoMod and approved legacy assistant use are separate. Guild API keys are never submitted through Discord.

## Required private runtime settings

Add these names to the existing mode-`0600` `.env.runtime` on the bot host and pass that file to `docker run --env-file`:

```dotenv
OMNICORE_OWNER_1_ID=1109540816013234256
OMNICORE_OWNER_2_ID=1157083515486220429
OMNICORE_BYOK_MASTER_KEY_HEX=<one stable 64-character hex value, generated with openssl rand -hex 32>
OMNICORE_DISCORD_CLIENT_ID=1521673931134603304
OMNICORE_DISCORD_CLIENT_SECRET=<Discord Developer Portal OAuth2 client secret>
```

Do not add actual secrets to `.env.example`, source control, logs, or chat. Losing the master key makes stored provider credentials unreadable; keep a protected copy with the production runtime configuration. Rotating it requires a deliberate migration that decrypts with the old key and re-encrypts each credential with the new key. Records contain a key version field for that future migration.

Register the exact Discord OAuth2 redirect `https://discordbot.omnicoretech.org/connect/callback` for OmniCore's application. The bot starts its Rust portal on Docker-internal port 8098 only when both its master key and OAuth settings are present. The site Nginx container must share `omnicore-network` and reverse proxy `/connect/` to `http://omnicore-discord-bot:8098`, with access logging disabled for that path. The public Cloudflare endpoint must remain HTTPS.

## Pairing and use

The current guild owner runs `/ai setup` and receives an ephemeral link valid for ten minutes. The portal uses Discord OAuth2 authorization code flow, state, PKCE S256, secure cookies, CSRF tokens, and a fresh bot-API guild ownership check. The owner validates a provider key, chooses a model reported by that provider, and connects it. The key is encrypted with AES-256-GCM and random nonce before MongoDB storage. The session and unpaired key stay in process memory only and expire or disappear on restart.

An authorized member can use `/ask` or mention OmniCore in a configured channel. `/ai limits`, `/ai channel`, and `/ai role` control use. Each accepted request atomically reserves a daily per-guild and per-member slot before contacting the provider. A single DM warns the owner when the guild reaches at least 80% of its daily request limit. Request quotas are not a precise spending cap. `/ai usage` shows provider request and token metadata without prompts or estimated cost. `/ai disconnect` removes the stored key; owners should revoke it at the provider if needed.

## Moderation data

The AutoMod action path publishes an event only after a warning was actually delivered or Discord confirmed a timeout. Failed timeout attempts are separate. Events have no raw message body. One outbox record per incident/recipient deduplicates alerts to the two developers and current guild owner. Owner messages include the affected member and channel. Developer messages omit member details and raw content. `/automod diagnostics` disables operator diagnostic alerts per guild. Owner DM failure falls back only to a configured channel with an explicit `@everyone` view deny. Metadata and delivery records have 90-day TTL indexes. Raw AutoMod incident context remains capped at sixty seconds by the existing incident path.

## Rollback

Save the previous source, site, Nginx configuration, Compose override, and image tag before replacement. Revert the site and proxy first, then restore the previous container image with its original environment if the new process fails health checks. Do not delete the MongoDB records created during rollback; old code ignores the new collections and the TTL indexes expire their metadata. Reverting the bot does not undo Discord actions already applied.
