# Local image generation

`/imagegen prompt:<description>` and `/imagine` request FLUX.2 Klein 4B from a **local**, image-capable Ollama-compatible `/api/generate` endpoint. They are available only in guilds approved for local AI by a bot operator. Status updates and the final PNG are public. No cloud image API or guild provider key is used.

The expected model tag is `x/flux2-klein:4b`. Set `OMNICORE_IMAGEGEN_OLLAMA_URL` to a dedicated local image service, or the bot falls back to `OLLAMA_BASE_URL`. The endpoint guard accepts only HTTP on `localhost`, `127.0.0.1`, or Docker Desktop's `host.docker.internal`. Do not loosen this guard to arbitrary URLs; it protects against server-side request forgery. Image support depends on your installed Ollama version and model. Check that your service actually accepts image generation before enabling the feature. Model weights and the image service are not included in this repository.

Each prompt is trimmed and limited to 500 Unicode characters. One request per user and guild runs at a time; a user has a 60-second cooldown. The global queue holds at most four requests, generation runs one at a time, queue wait is limited to two minutes, and generation to four minutes. Progress milestones are elapsed-time estimates because the service does not report a reliable completion percentage.

The bot validates the base64 response as PNG, bounds encoded/decoded size and dimensions, writes a random temporary output file in `OMNICORE_IMAGEGEN_TEMP_DIR` or the OS temp directory, uploads it to Discord, and removes the local file. Prompt and image bytes are not stored in MongoDB. Discord retains the posted message and attachment under its own policies. Logs include metadata such as IDs, length, duration, size, and failure category, not prompt text or image bytes. `/imagegenstop` cancels active generation and requests model unload. Normal completion also asks Ollama to unload the model (`keep_alive: 0`) to reduce RAM use.

On a memory-constrained host, leave enough free disk for OS swap. You may set `OMNICORE_IMAGEGEN_HOST_DISK_PATH` to a safe, read-only mount on the host filesystem; generation pauses when the check fails or available space is below 10 GiB. Without this variable, the optional host disk guard is disabled.

To test, verify the image service and model independently, then run `/imagegen prompt:a gray cat` in an approved test guild. Check the public status edit and attached PNG. A model that appears in the Ollama list but rejects image generation is not usable for this feature.
