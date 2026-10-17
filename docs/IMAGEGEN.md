# Local image generation

`/imagegen prompt:<description>` runs FLUX.2 Klein 4B through a local Ollama `/api/generate` request. It is available in guilds approved for local AI by an OmniCore bot owner. The command's interaction response, progress embed, and final PNG attachment are public. No cloud image API or provider key is used.

The exact model tag is `x/flux2-klein:4b`. The bot uses `OMNICORE_IMAGEGEN_OLLAMA_URL` if set, otherwise `OLLAMA_BASE_URL`. Only local HTTP hosts (`localhost`, `127.0.0.1`, and Docker Desktop's `host.docker.internal`) are accepted. The existing chat and AutoMod Ollama URL and model are unchanged.

Ollama 0.40.5 lists the FLUX.2 model but returns HTTP 400 for generation because Ollama removed its experimental image engine after 0.32.5. On this Mac, a dedicated official Ollama 0.32.5 process serves only image generation on `127.0.0.1:11435`. Its model store is on the external drive. The production bot container points `OMNICORE_IMAGEGEN_OLLAMA_URL` to `http://host.docker.internal:11435`; existing AI continues to use port 11434. The older image service is bound to loopback and has cloud access disabled. If it is unavailable, `/imagegen` reports failure publicly without changing other bot features.

Each prompt is trimmed and limited to 500 Unicode characters. A user may have one active request and must wait 60 seconds between accepted requests. Each guild may have one active or queued request. The global queue admits at most four requests, and one request generates at a time. Queue waiting has a two-minute deadline, and generation has a four-minute deadline. The status message progresses through received, queued, sending, generating, in progress, finalizing, and ready or failed. The 60% milestone is time based; Ollama does not report a true percentage here.

The bot validates Ollama's base64 response as a PNG, limits encoded and decoded size, limits image dimensions and decoder allocation, and writes the output to a random name in `OMNICORE_IMAGEGEN_TEMP_DIR` (or the system temp directory). It reads that file into a Discord attachment and removes the file after upload or failure. It does not store the prompt or image in MongoDB. Discord keeps the posted message and attachment under its own policies. Logs contain IDs, prompt length, duration, output size, and failure category; they do not contain prompt text or image bytes.

Each request generates a 512×512 PNG and asks Ollama to unload the image model when generation finishes (`keep_alive: 0`) to reduce memory pressure on this 16 GB Mac. Keep several gigabytes of internal disk free for macOS swap. A kernel panic on October 17, 2026 reported a watchdog timeout, a full compressor segment limit, and low swap space while the internal disk had less than 500 MB available; image generation should not be run while the disk is that full.

On this Mac, the bot container has an empty internal-drive directory mounted read-only at `/host-disk-check`, and `OMNICORE_IMAGEGEN_HOST_DISK_PATH=/host-disk-check`. Before sending a job to Ollama, the Rust command uses `statvfs` on that mount and publicly pauses the request if the Mac has less than 8 GiB available or the check fails. The mount exposes no host files. Other deployments can configure their own check path; without it, this optional guard is disabled.

Local setup checks:

1. `curl http://127.0.0.1:11435/api/version` should report the dedicated image-capable Ollama version.
2. The dedicated model store must include `x/flux2-klein:4b`.
3. The bot container must reach `http://host.docker.internal:11435/api/version`.
4. In an approved test guild, invoke `/imagegen prompt:a gray cat` and verify the public progress edit and PNG attachment.

The host-side launch agent is `~/Library/LaunchAgents/org.omnicore.imagegen-ollama.plist`. Its executable is under `~/Library/Application Support/OmniCore/imagegen-ollama/`, while the model store and runtime assets are under `/Volumes/Mac Mini External Storage 1TB/Services/omnicore-ollama-imagegen/`. Keep the external drive mounted before launching the service. The image service is independent of the primary Ollama service on port 11434.
