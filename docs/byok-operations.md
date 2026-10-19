# Secure server-owned AI setup

OmniCore's optional provider path uses OpenAI Responses, Anthropic Messages, and Gemini Generate Content APIs. Each guild owner supplies their own API key through the Rust HTTPS pairing portal; keys are never accepted in Discord chat. Provider use may incur charges on that owner's account.

## Private runtime settings

Set these values in an ignored `.env` or secret manager:

```dotenv
OMNICORE_OWNER_IDS=<numeric Discord operator ID, or comma-separated IDs>
OMNICORE_BYOK_MASTER_KEY_HEX=<stable 64-character hex value>
OMNICORE_DISCORD_CLIENT_ID=<your Discord application ID>
OMNICORE_DISCORD_CLIENT_SECRET=<your Discord OAuth client secret>
OMNICORE_PUBLIC_BASE_URL=https://your-domain.example
```

Generate the master key privately with `openssl rand -hex 32` and keep an encrypted backup. Losing it makes stored provider keys unreadable. Rotating it requires a deliberate re-encryption migration; changing the variable alone will break existing connections. Existing installations may use `OMNICORE_OWNER_1_ID` and `OMNICORE_OWNER_2_ID` instead of `OMNICORE_OWNER_IDS`.

Register exactly `https://your-domain.example/connect/callback` in your application's Discord OAuth2 redirects. Proxy `/connect/` to bot port 8098 on a private network; terminate TLS at the proxy. The portal starts only when the master key, client ID, client secret, and valid `OMNICORE_PUBLIC_BASE_URL` are configured. Do not expose plain HTTP port 8098 publicly. The bundled `deploy/nginx/` files describe the official site's deployment and need adaptation for another domain/network.

## Pairing and permissions

The current guild owner runs `/ai setup` and receives a short-lived link. The portal uses Discord OAuth2 authorization code flow, PKCE S256, state, secure session cookies, CSRF protection, and a fresh guild ownership check. The owner validates a provider key and chooses a model returned by that provider. AES-256-GCM encrypts the key before MongoDB storage. Unpaired keys and sessions remain only in process memory and expire or disappear on restart.

Owners can manage access with `/ai status`, `/ai model`, `/ai limits`, `/ai channel`, `/ai role`, `/ai vision`, `/ai thinking`, and `/ai disconnect`. Members use `/ask` where allowed. Request quotas are usage controls, not precise spending caps. `/ai usage` records counts and provider token metadata without prompts or image data. Disconnecting removes the stored key; revoke it at the provider too if compromise is suspected.

AutoMod uses the configured local Ollama classifier, not a guild's cloud key. Local bot-approved chat and vision are separate from server-owned provider billing. The owner can enable voice provider use within their existing AI limits.

## Operations

Back up the private runtime configuration and MongoDB securely. Do not put keys in Git, issues, logs, or chat. Retain the master key across restarts. Review `/ai usage` and provider-side billing. On rollback, restore a compatible image and runtime variables; do not delete MongoDB collections blindly. [Retention details](RETENTION.md) describe safe legacy migration and deletion.
