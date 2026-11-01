# OmniCore AutoMod

AutoMod is enabled by default in every guild. It runs independently of the optional mention-based AI chat approval. `/automod settings` and `/automod status` show the active policy. Only the live Discord guild owner can run the mutation subcommands: `enable`, `disable`, `sensitivity`, `timeout`, `warnings`, `timeouts`, `notifications`, and `log-channel`.

## Pipeline

1. A local candidate screen checks targeted abuse phrases, severe threats, excessive mentions, and repeated messages. Safe traffic never reaches the moderation model. A second conservative local guard is required before an automatic timeout.
2. Up to 12 concurrent requests go to the configured Ollama `/api/chat` endpoint with a 30-second request timeout, allowing requests to wait while normal AI chat uses the same local model. Input contains only the selected message (at most 1,500 characters), repeat count, and mention count. No channel history or normal AI memory is sent. The model returns a strictly validated typed assessment. Invalid or unavailable responses cause no action and log a specific failure stage without logging message content. There are no retries. Rate limits are 30 candidate requests per guild and four per user per minute.
3. The guild policy uses sensitivity presets and confidence thresholds. The model cannot call Discord tools. The conversational AI's old unrestricted timeout tool was removed. Warnings and temporary timeouts are applied by the Rust action layer only. The first confirmed violation produces a warning; another confirmed violation by that member in the same guild within 24 hours produces a timeout. Two warning-level violations use the configured moderate duration. More severe second violations use the corresponding configured duration. If timeouts are disabled, further violations continue to warn. If warnings are disabled, AutoMod cannot start a new timeout escalation sequence.

Normal sensitivity requires confidence at least 0.90 for warnings, 0.97 for timeouts, and 0.95 for severe timeouts. Low is stricter (0.95, 0.99, 0.985); high allows warnings at 0.84 with timeout thresholds of 0.96 and 0.93. Severe classifications must have an appropriate category and the local timeout guard. Uncertain assessments cause no action.

Default durations are 10 minutes for moderate, one hour for serious, six hours for severe, and 24 hours for extreme violations. Every automatic duration is capped by `MAX_AUTO_TIMEOUT` in `src/automod/config.rs` at 48 hours, including database reads and action execution. Invalid owner durations are rejected. The bot never automatically bans or kicks users.

## Safety and permissions

Bots are skipped. The guild owner and members with administrative or moderation permissions can receive a public warning and a private notice when their message is confirmed as a violation, but they are not automatically timed out and do not accumulate timeout strikes. The owner receives a separate incident notice by default. Before a timeout on an ordinary member, AutoMod fetches current guild and member data, checks `MODERATE_MEMBERS`, and compares the bot's highest role to the target's. Failed or impossible timeout attempts notify the owner by default. Notifications first try the owner's DM and then use a configured channel only if that channel explicitly denies View Channel to @everyone. For every delivered warning or successful timeout, the bot replies to the flagged message in its channel, explicitly mentions only the affected member, and states the reason. It also tries to DM the member. If the original message was deleted, the bot tries an ordinary channel message. If the timeout fails, it replies with a warning that does not claim a timeout happened.

The `notify_owner` setting defaults to true and can be changed by the guild owner. If both owner DMs and a private fallback channel are unavailable, notification delivery is not guaranteed. Discord role or permission changes between the preflight and API call can still cause a timeout to fail safely.

## Owner-requested mute

The server owner can mention OmniCore and ask it to mute one explicitly mentioned member for a duration up to two days, for example `@OmniCore mute @member for 10m`. Replying to that member's message with `@OmniCore mute for 10m` also works. The bot resolves the target from the current message or reply, fetches the current guild owner from Discord, validates the duration, checks the target and its own permissions and role hierarchy, and applies a Discord timeout directly. It does not use model-generated tool arguments or infer a target from old chat history. If the target or duration is missing, it asks for them. The existing `/time` command remains available to members with Moderate Members permission.

## Privacy and retention

Candidate message content is held in a bounded in-memory incident cache during classification; periodic cleanup evicts it after approximately 60 seconds. For a confirmed violation, a sanitized excerpt of up to 800 characters is kept in pending notification deliveries so the server owner and enabled bot operators can read the alert. The excerpt is removed after successful delivery or final failure. An undelivered record expires after at most 90 days. Event analytics and warning state contain no incident text, and the message does not enter MongoDB `ai_turns` history. Warning records expire after 24 hours. Discord may retain delivered owner/operator alerts and public moderation replies under its own policies.


The existing mention-based AI chat continues to store ordinary mentions in `ai_turns` under its separate 30-day retention policy. A message selected as an AutoMod candidate is withheld from that path even if the classifier ultimately decides it is safe. Owner mute requests are handled before normal AI chat and are not saved as AI turns.

## Deployment

The Discord application must have **Message Content Intent** enabled in the Developer Portal, and the bot must have **Moderate Members** permission with a role above members it may time out. The source already requests `GatewayIntents::MESSAGE_CONTENT` and `GUILD_MEMBERS`; no new privileged intents were added. Deploy the updated bot and privacy policy together. There is no dedicated test guild in this source package, so live warning, timeout, DM, and fallback-channel delivery need a test account in a test guild before broader rollout.
