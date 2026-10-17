# OmniCore image recognition

Image recognition is part of the Rust BYOK path. A guild owner connects an OpenAI, Anthropic, or Gemini API key with `/ai setup`. The existing chat model is used for images only when it supports image input; the owner may select a separate account-available vision model with `/ai vision model:<model-id>`. OmniCore never changes the chat model automatically.

## Invoking vision

- Attach a PNG, JPEG, WebP, or GIF to the same message as a direct `@OmniCore` image question.
- Or reply directly to the message containing the image and mention OmniCore in that reply. OmniCore fetches only the exact referenced message ID from the same channel and guild. It never searches earlier channel messages or follows reply chains.
- A message that merely says “describe the picture above” without an attachment or direct reply receives instructions to attach or reply to the image.
- A normal image post without a direct invocation does not trigger vision.
- Explicit image questions in DMs need no mention. To bill a guild key in DMs, the guild owner must opt in with `/ai vision dm_enabled:true`. Only that guild's current owner can use the key in DMs. If the owner has multiple opted-in guilds, the DM must include `server:<guild-id>` to select one.

The hard maximum is three images per request. Owners can lower it to one or two with `/ai vision max_images:<count>`. Each image must be at most 4 MiB, 6,000 pixels on either side, and 20 megapixels. Animated GIFs use their first frame, converted to PNG in memory. If the format, file data, size, or direct reply cannot be validated, OmniCore does not call the AI provider.

## Access, cost, and privacy

Vision inherits the guild's BYOK channel and role restrictions, concurrency cap, output token cap, and atomic daily user and guild request quotas. Owner-only DM vision requires its own explicit opt-in and still uses that guild's daily quotas. Image requests can cost more than text requests; the provider bills the connected account. `/ai usage` shows image request and submitted-image counts alongside reported token use. OmniCore does not estimate exact image charges.

Downloads accept only HTTPS Discord attachment CDN URLs, with redirects disabled and byte/time limits. The image's real format and dimensions are decoded before it is sent as base64 to the selected provider. Image bytes live only in the current request and are dropped when it completes or times out after 55 seconds. No image files, base64 data, image descriptions, or chat history are written to MongoDB by the vision path. Usage records contain only guild, provider, model, status, image count, token counts when reported, and time; they expire after up to 90 days. Provider-side processing and retention follow the provider's terms.

The bot needs View Channel, Send Messages, and Read Message History in the channel to fetch an exact replied-to message. Its Message Content intent is needed for message-based AI requests; Guild Members access supports role checks. Expired Discord attachment links may need to be reattached.
