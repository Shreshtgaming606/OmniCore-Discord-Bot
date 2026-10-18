//! Local, public image generation. Image bytes and prompts are never stored in MongoDB.

use crate::{CustomContext, Error, config, database};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use poise::{
    CreateReply,
    serenity_prelude::{Colour, CreateAllowedMentions, CreateAttachment, CreateEmbed},
};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Write},
    path::PathBuf,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

const MODEL: &str = "x/flux2-klein:4b";
const IMAGE_SIZE: u32 = 512;
const MAX_PROMPT_CHARS: usize = 500;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 12 * 1024 * 1024;
const COOLDOWN: Duration = Duration::from_secs(60);
const QUEUE_WAIT: Duration = Duration::from_secs(120);
const GENERATION_TIMEOUT: Duration = Duration::from_secs(240);
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(25);
const STOP_WAIT: Duration = Duration::from_secs(45);
const QUEUE_CAPACITY: usize = 4;
const MIN_HOST_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

static QUEUE_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(QUEUE_CAPACITY)));
static GENERATOR: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));
static ADMISSION: LazyLock<Mutex<Admission>> = LazyLock::new(|| Mutex::new(Admission::default()));
static RESTARTING: AtomicBool = AtomicBool::new(false);

pub fn generation_active() -> bool {
    ADMISSION
        .lock()
        .is_ok_and(|state| state.jobs.values().any(|job| job.phase != JobPhase::Queued))
}

#[derive(Default)]
struct Admission {
    active_users: HashSet<u64>,
    jobs: HashMap<u64, ActiveJob>,
    current_generator: Option<u64>,
    last_request: HashMap<u64, Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobPhase {
    Queued,
    Generating,
    Finalizing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopOutcome {
    Running,
    Finished,
    QueuedStopped,
    StoppedUnloaded,
    StoppedUnconfirmed,
}

struct ActiveJob {
    user: u64,
    phase: JobPhase,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<StopOutcome>,
}

struct Reservation {
    user: u64,
    guild: u64,
    cancel: watch::Receiver<bool>,
    done: watch::Sender<StopOutcome>,
    _queue_slot: OwnedSemaphorePermit,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if *self.done.borrow() == StopOutcome::Running {
            self.done.send_replace(StopOutcome::Finished);
        }
        if let Ok(mut state) = ADMISSION.lock() {
            state.active_users.remove(&self.user);
            state.jobs.remove(&self.guild);
            if state.current_generator == Some(self.guild) {
                state.current_generator = None;
            }
        }
    }
}

impl Reservation {
    fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    async fn cancelled(&mut self) {
        while !self.is_cancelled() && self.cancel.changed().await.is_ok() {}
    }

    fn complete(&self, outcome: StopOutcome) {
        self.done.send_replace(outcome);
    }

    fn set_phase(&self, phase: JobPhase) {
        if let Ok(mut state) = ADMISSION.lock() {
            if let Some(job) = state.jobs.get_mut(&self.guild) {
                job.phase = phase;
            }
            if phase == JobPhase::Generating {
                state.current_generator = Some(self.guild);
            } else if state.current_generator == Some(self.guild) {
                state.current_generator = None;
            }
        }
    }
}

fn admit(user: u64, guild: u64) -> Result<Reservation, &'static str> {
    if RESTARTING.load(Ordering::SeqCst) {
        return Err("OmniCore is restarting. Try again in a moment.");
    }
    let slot = QUEUE_SLOTS
        .clone()
        .try_acquire_owned()
        .map_err(|_| "The image generation queue is full. Try again shortly.")?;
    let mut state = ADMISSION
        .lock()
        .map_err(|_| "Image generation is temporarily unavailable.")?;
    if RESTARTING.load(Ordering::SeqCst) {
        return Err("OmniCore is restarting. Try again in a moment.");
    }
    let now = Instant::now();
    state
        .last_request
        .retain(|_, previous| now.duration_since(*previous) < COOLDOWN);
    if state.active_users.contains(&user) {
        return Err("You already have an image request in progress.");
    }
    if state.jobs.contains_key(&guild) {
        return Err("This server already has an image request in progress. Try again shortly.");
    }
    if let Some(previous) = state.last_request.get(&user) {
        let remaining = COOLDOWN.saturating_sub(now.duration_since(*previous));
        return Err(if remaining > Duration::from_secs(30) {
            "Please wait about a minute between image requests."
        } else {
            "Please wait a few more seconds before another image request."
        });
    }
    state.active_users.insert(user);
    state.last_request.insert(user, now);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (done_tx, done_rx) = watch::channel(StopOutcome::Running);
    state.jobs.insert(
        guild,
        ActiveJob {
            user,
            phase: JobPhase::Queued,
            cancel: cancel_tx,
            done: done_rx,
        },
    );
    Ok(Reservation {
        user,
        guild,
        cancel: cancel_rx,
        done: done_tx,
        _queue_slot: slot,
    })
}

fn request_stop(
    guild: u64,
    requester: u64,
    bot_owner: bool,
    guild_owner: bool,
) -> Result<watch::Receiver<StopOutcome>, &'static str> {
    let state = ADMISSION
        .lock()
        .map_err(|_| "Image generation is temporarily unavailable.")?;
    let target = if state
        .jobs
        .get(&guild)
        .is_some_and(|job| job.phase != JobPhase::Finalizing)
    {
        guild
    } else if bot_owner {
        state
            .current_generator
            .ok_or("No image is generating right now.")?
    } else {
        return Err("No image request is active in this server.");
    };
    let job = state
        .jobs
        .get(&target)
        .ok_or("No image is generating right now.")?;
    if requester != job.user && !bot_owner && !(guild_owner && target == guild) {
        return Err("Only the requester or a server or bot owner can stop this image request.");
    }
    if job.phase == JobPhase::Finalizing {
        return Err("The image has finished generating and is being uploaded.");
    }
    let done = job.done.clone();
    job.cancel.send_replace(true);
    Ok(done)
}

async fn wait_for_stop(done: &mut watch::Receiver<StopOutcome>) -> Option<StopOutcome> {
    tokio::time::timeout(STOP_WAIT, async {
        loop {
            let outcome = *done.borrow();
            if outcome != StopOutcome::Running {
                return outcome;
            }
            if done.changed().await.is_err() {
                return *done.borrow();
            }
        }
    })
    .await
    .ok()
}

pub async fn stop_all_for_restart() {
    RESTARTING.store(true, Ordering::SeqCst);
    let mut jobs = if let Ok(state) = ADMISSION.lock() {
        state
            .jobs
            .values()
            .map(|job| {
                if job.phase != JobPhase::Finalizing {
                    job.cancel.send_replace(true);
                }
                job.done.clone()
            })
            .collect::<Vec<_>>()
    } else {
        log::error!("Image generation state is unavailable during restart");
        Vec::new()
    };
    let _ = futures::future::join_all(jobs.iter_mut().map(wait_for_stop)).await;
    if let Some(base) = imagegen_ollama_base()
        && let Err(error) = unload_model(&base).await
    {
        log::warn!("Restart could not confirm image model unload: {error:?}");
    }
}

fn imagegen_ollama_base() -> Option<String> {
    std::env::var("OMNICORE_IMAGEGEN_OLLAMA_URL")
        .ok()
        .or_else(|| config::OLLAMA_BASE_URL.get().cloned())
}

fn validate_prompt(input: &str) -> Result<String, &'static str> {
    let prompt = input.trim();
    if prompt.is_empty() {
        return Err("Please enter a description of the image you want.");
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err("Prompts must be 500 characters or fewer.");
    }
    if prompt.chars().any(|ch| ch.is_control() && ch != '\n') {
        return Err("The prompt contains unsupported control characters.");
    }
    Ok(prompt.to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Received,
    Queued,
    Sending,
    Generating,
    InProgress,
    Finalizing,
    Ready,
    Failed,
    Stopped,
}

impl Stage {
    fn details(self) -> (&'static str, &'static str) {
        match self {
            Self::Received => ("Request received", "0%"),
            Self::Queued => ("Queued for the local generator", "10%"),
            Self::Sending => ("Sending to local Ollama", "20%"),
            Self::Generating => ("Model generation started", "40%"),
            Self::InProgress => ("Generating locally (time based milestone)", "60%"),
            Self::Finalizing => ("Validating and preparing the image", "80%"),
            Self::Ready => ("Image ready", "100%"),
            Self::Failed => ("Failed", "—"),
            Self::Stopped => ("Stopped", "—"),
        }
    }
}

fn status_reply(prompt: &str, stage: Stage, reason: Option<&str>) -> CreateReply {
    let (label, progress) = stage.details();
    let mut description = format!(
        "**Prompt**\n{}\n\n**Status** {label}\n**Progress** {progress}",
        prompt
    );
    if let Some(reason) = reason {
        description.push_str("\n**Reason** ");
        description.push_str(reason);
    }
    CreateReply::default()
        .embed(
            CreateEmbed::new()
                .title(if stage == Stage::Ready {
                    "🎨 OmniCore Image Generation Complete"
                } else {
                    "🎨 OmniCore Image Generation"
                })
                .description(description)
                .colour(if matches!(stage, Stage::Failed | Stage::Stopped) {
                    Colour::from_rgb(220, 70, 70)
                } else {
                    Colour::from_rgb(105, 92, 230)
                }),
        )
        .allowed_mentions(
            CreateAllowedMentions::new()
                .empty_users()
                .empty_roles()
                .everyone(false),
        )
        .ephemeral(false)
}

#[derive(Debug)]
enum GenerateError {
    Unavailable,
    HostDiskUnavailable,
    HostDiskLow,
    ModelMissing,
    UnsupportedRuntime,
    Timeout,
    Cancelled,
    Rejected,
    BadImage,
}

impl GenerateError {
    fn message(&self) -> &'static str {
        match self {
            Self::Unavailable => "The local Ollama server is unavailable. Please try later.",
            Self::HostDiskUnavailable => {
                "I could not check the Mac's free disk space, so image generation is paused."
            }
            Self::HostDiskLow => {
                "The Mac needs at least 10 GiB of free internal disk space for safe image generation."
            }
            Self::ModelMissing => "The FLUX.2 Klein 4B model is missing from local Ollama.",
            Self::UnsupportedRuntime => {
                "This Ollama version cannot generate images. Start the local image-capable Ollama service."
            }
            Self::Timeout => "Local generation took too long. Please try again later.",
            Self::Cancelled => "Image generation was stopped.",
            Self::Rejected => "Local Ollama could not complete this image request.",
            Self::BadImage => "Ollama returned an invalid or oversized image.",
        }
    }
}

#[cfg(unix)]
fn available_disk_bytes(path: &std::path::Path) -> std::io::Result<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs writes the structure on success; the configured path is local and read-only.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    Ok((stats.f_bavail as u128)
        .saturating_mul(stats.f_frsize as u128)
        .min(u64::MAX as u128) as u64)
}

fn ensure_host_space(free: u64) -> Result<(), GenerateError> {
    if free < MIN_HOST_FREE_BYTES {
        Err(GenerateError::HostDiskLow)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn check_host_space() -> Result<(), GenerateError> {
    let Some(path) = std::env::var_os("OMNICORE_IMAGEGEN_HOST_DISK_PATH") else {
        return Ok(());
    };
    let free = available_disk_bytes(std::path::Path::new(&path))
        .map_err(|_| GenerateError::HostDiskUnavailable)?;
    ensure_host_space(free)
}

#[cfg(not(unix))]
fn check_host_space() -> Result<(), GenerateError> {
    if std::env::var_os("OMNICORE_IMAGEGEN_HOST_DISK_PATH").is_some() {
        Err(GenerateError::HostDiskUnavailable)
    } else {
        Ok(())
    }
}

fn decode_image(value: &serde_json::Value) -> Result<Vec<u8>, GenerateError> {
    let encoded = value
        .get("image")
        .and_then(serde_json::Value::as_str)
        .ok_or(GenerateError::BadImage)?;
    if encoded.len() > MAX_RESPONSE_BYTES {
        return Err(GenerateError::BadImage);
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| GenerateError::BadImage)?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(GenerateError::BadImage);
    }
    let mut reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| GenerateError::BadImage)?;
    let format = reader.format().ok_or(GenerateError::BadImage)?;
    if format != image::ImageFormat::Png {
        return Err(GenerateError::BadImage);
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|_| GenerateError::BadImage)?;
    Ok(bytes)
}

fn local_endpoint(base: &str, endpoint: &str) -> Result<String, GenerateError> {
    let parsed = reqwest::Url::parse(base).map_err(|_| GenerateError::Unavailable)?;
    if parsed.scheme() != "http"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(
            parsed.host_str(),
            Some("localhost" | "127.0.0.1" | "host.docker.internal")
        )
    {
        return Err(GenerateError::Unavailable);
    }
    Ok(format!("{}/{}", base.trim_end_matches('/'), endpoint))
}

fn image_model_loaded(value: &serde_json::Value) -> Result<bool, GenerateError> {
    let models = value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or(GenerateError::Unavailable)?;
    Ok(models.iter().any(|model| {
        model.get("name").and_then(serde_json::Value::as_str) == Some(MODEL)
            || model.get("model").and_then(serde_json::Value::as_str) == Some(MODEL)
    }))
}

async fn unload_model(base: &str) -> Result<(), GenerateError> {
    let generate_url = local_endpoint(base, "api/generate")?;
    let ps_url = local_endpoint(base, "api/ps")?;
    tokio::time::timeout(UNLOAD_TIMEOUT, async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| GenerateError::Unavailable)?;
        let response = client
            .post(generate_url)
            .json(&serde_json::json!({"model": MODEL, "prompt": "", "stream": false, "keep_alive": 0}))
            .send()
            .await
            .map_err(|_| GenerateError::Unavailable)?;
        if !response.status().is_success() {
            return Err(GenerateError::Rejected);
        }
        for _ in 0..8 {
            let response = client
                .get(&ps_url)
                .send()
                .await
                .map_err(|_| GenerateError::Unavailable)?;
            if !response.status().is_success() {
                return Err(GenerateError::Unavailable);
            }
            let value: serde_json::Value = response
                .json()
                .await
                .map_err(|_| GenerateError::Unavailable)?;
            if !image_model_loaded(&value)? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(GenerateError::Timeout)
    })
    .await
    .map_err(|_| GenerateError::Timeout)?
}

async fn generate(base: &str, prompt: &str) -> Result<Vec<u8>, GenerateError> {
    let url = local_endpoint(base, "api/generate")?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(GENERATION_TIMEOUT)
        .build()
        .map_err(|_| GenerateError::Unavailable)?;
    let mut response = client
        .post(url)
        .json(&serde_json::json!({"model": MODEL, "prompt": prompt, "stream": false, "keep_alive": 0, "width": IMAGE_SIZE, "height": IMAGE_SIZE}))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GenerateError::Timeout
            } else {
                GenerateError::Unavailable
            }
        })?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(GenerateError::ModelMissing);
    }
    if response.status() == reqwest::StatusCode::BAD_REQUEST {
        let mut error_body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| GenerateError::Rejected)?
        {
            if error_body.len() + chunk.len() > 4096 {
                break;
            }
            error_body.extend_from_slice(&chunk);
        }
        if String::from_utf8_lossy(&error_body)
            .contains("image generation models are not currently supported")
        {
            return Err(GenerateError::UnsupportedRuntime);
        }
        return Err(GenerateError::Rejected);
    }
    if !response.status().is_success() {
        return Err(GenerateError::Rejected);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| GenerateError::Unavailable)?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(GenerateError::BadImage);
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| GenerateError::BadImage)?;
    decode_image(&value)
}

struct TempImage(PathBuf);

impl TempImage {
    fn create(bytes: &[u8]) -> std::io::Result<Self> {
        let directory = std::env::var_os("OMNICORE_IMAGEGEN_TEMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("omnicore-imagegen"));
        std::fs::create_dir_all(&directory)?;
        let metadata = std::fs::symlink_metadata(&directory)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "image output directory must be a real directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        for _ in 0..3 {
            let path = directory.join(format!("imagegen-{:032x}.png", rand::random::<u128>()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let image = Self(path);
                    file.write_all(bytes)?;
                    return Ok(image);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "temporary filename collision",
        ))
    }
}

impl Drop for TempImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Generate an image locally with FLUX.2 Klein 4B.
#[poise::command(
    slash_command,
    guild_only,
    category = "AI",
    description_localized("en-US", "Generate an image locally with FLUX.2 Klein 4B")
)]
pub async fn imagegen(
    ctx: CustomContext<'_>,
    #[description = "Describe the image to generate"] prompt: String,
) -> Result<(), Error> {
    run_imagegen(ctx, prompt).await
}

/// Generate an image locally with FLUX.2 Klein 4B.
#[poise::command(
    slash_command,
    guild_only,
    category = "AI",
    description_localized("en-US", "Generate an image locally with FLUX.2 Klein 4B")
)]
pub async fn imagine(
    ctx: CustomContext<'_>,
    #[description = "Describe the image to generate"] prompt: String,
) -> Result<(), Error> {
    run_imagegen(ctx, prompt).await
}

async fn run_imagegen(ctx: CustomContext<'_>, prompt: String) -> Result<(), Error> {
    let started = Instant::now();
    let guild = ctx.guild_id().ok_or("Server required")?.get();
    let user = ctx.author().id.get();
    let channel = ctx.channel_id().get();
    ctx.defer().await?; // Poise's public defer; never defer_ephemeral.
    let preview = if prompt.chars().count() > MAX_PROMPT_CHARS {
        "(prompt is too long)".to_owned()
    } else {
        prompt.trim().to_owned()
    };
    let status = ctx
        .send(status_reply(&preview, Stage::Received, None))
        .await?;
    let prompt = match validate_prompt(&prompt) {
        Ok(prompt) => prompt,
        Err(message) => {
            status
                .edit(ctx, status_reply(&preview, Stage::Failed, Some(message)))
                .await?;
            return Ok(());
        }
    };
    match database::is_ai_approved(guild).await {
        Ok(true) => {}
        Ok(false) => {
            status.edit(ctx, status_reply(&prompt, Stage::Failed, Some("Local AI is not approved for this server. Ask an OmniCore bot owner to approve it."))).await?;
            return Ok(());
        }
        Err(error) => {
            log::warn!("imagegen approval lookup failed guild={guild}: {error}");
            status
                .edit(
                    ctx,
                    status_reply(
                        &prompt,
                        Stage::Failed,
                        Some("AI approval status is temporarily unavailable."),
                    ),
                )
                .await?;
            return Ok(());
        }
    }
    if ctx.data().voice.active.lock().await.is_some() {
        status.edit(ctx, status_reply(&prompt, Stage::Failed, Some("A voice session is active. Try image generation after it disconnects so the local models have enough memory."))).await?;
        return Ok(());
    }
    let mut reservation = match admit(user, guild) {
        Ok(reservation) => reservation,
        Err(message) => {
            status
                .edit(ctx, status_reply(&prompt, Stage::Failed, Some(message)))
                .await?;
            return Ok(());
        }
    };
    log::info!(
        "imagegen accepted user={user} guild={guild} channel={channel} prompt_chars={}",
        prompt.chars().count()
    );
    status
        .edit(ctx, status_reply(&prompt, Stage::Queued, None))
        .await?;
    let _generator = tokio::select! {
        biased;
        _ = reservation.cancelled() => {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                status.edit(ctx, status_reply(&prompt, Stage::Stopped, Some("Stopped before generation started."))),
            ).await;
            reservation.complete(StopOutcome::QueuedStopped);
            return Ok(());
        }
        result = tokio::time::timeout(QUEUE_WAIT, GENERATOR.clone().acquire_owned()) => {
            match result {
                Ok(Ok(permit)) => permit,
                _ => {
                    status
                        .edit(
                            ctx,
                            status_reply(
                                &prompt,
                                Stage::Failed,
                                Some("The local generator remained busy for too long. Try again later."),
                            ),
                        )
                        .await?;
                    return Ok(());
                }
            }
        }
    };
    if reservation.is_cancelled() {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            status.edit(
                ctx,
                status_reply(
                    &prompt,
                    Stage::Stopped,
                    Some("Stopped before generation started."),
                ),
            ),
        )
        .await;
        reservation.complete(StopOutcome::QueuedStopped);
        return Ok(());
    }
    if ctx.data().voice.active.lock().await.is_some() {
        status.edit(ctx, status_reply(&prompt, Stage::Failed, Some("A voice session started while this request was queued. Try again after it disconnects."))).await?;
        return Ok(());
    }
    reservation.set_phase(JobPhase::Generating);
    status
        .edit(ctx, status_reply(&prompt, Stage::Sending, None))
        .await?;
    if let Err(error) = check_host_space() {
        log::warn!("imagegen host disk preflight failed guild={guild} reason={error:?}");
        status
            .edit(
                ctx,
                status_reply(&prompt, Stage::Failed, Some(error.message())),
            )
            .await?;
        return Ok(());
    }
    // A dedicated local Ollama build can serve images without changing chat AI.
    let base = imagegen_ollama_base().ok_or("Ollama URL not configured")?;
    let mut request = Box::pin(generate(&base, &prompt));
    status
        .edit(ctx, status_reply(&prompt, Stage::Generating, None))
        .await?;
    let bytes = tokio::select! {
        biased;
        _ = reservation.cancelled() => Err(GenerateError::Cancelled),
        result = &mut request => result,
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                status.edit(ctx, status_reply(&prompt, Stage::InProgress, None)),
            ).await;
            tokio::select! {
                biased;
                _ = reservation.cancelled() => Err(GenerateError::Cancelled),
                result = &mut request => result,
            }
        }
    };
    drop(request); // Disconnect the active generation request before asking Ollama to unload.
    if matches!(&bytes, Err(GenerateError::Cancelled)) || reservation.is_cancelled() {
        let unloaded = unload_model(&base).await.is_ok();
        let reason = if unloaded {
            "Stopped; the image model was unloaded from Ollama."
        } else {
            "Stopped; I could not confirm that Ollama unloaded the model."
        };
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            status.edit(ctx, status_reply(&prompt, Stage::Stopped, Some(reason))),
        )
        .await;
        reservation.complete(if unloaded {
            StopOutcome::StoppedUnloaded
        } else {
            StopOutcome::StoppedUnconfirmed
        });
        log::info!("imagegen stopped user={user} guild={guild} model_unloaded={unloaded}");
        return Ok(());
    }
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            if let Err(unload_error) = unload_model(&base).await {
                log::warn!("imagegen cleanup could not confirm model unload: {unload_error:?}");
            }
            log::warn!(
                "imagegen failed user={user} guild={guild} channel={channel} duration_ms={} reason={error:?}",
                started.elapsed().as_millis()
            );
            status
                .edit(
                    ctx,
                    status_reply(&prompt, Stage::Failed, Some(error.message())),
                )
                .await?;
            return Ok(());
        }
    };
    reservation.set_phase(JobPhase::Finalizing);
    status
        .edit(ctx, status_reply(&prompt, Stage::Finalizing, None))
        .await?;
    let image = match TempImage::create(&bytes) {
        Ok(image) => image,
        Err(error) => {
            log::error!("imagegen temporary file failed: {error}");
            status
                .edit(
                    ctx,
                    status_reply(
                        &prompt,
                        Stage::Failed,
                        Some("Could not prepare the image for Discord upload."),
                    ),
                )
                .await?;
            return Ok(());
        }
    };
    let attachment = match CreateAttachment::path(&image.0).await {
        Ok(attachment) => attachment,
        Err(error) => {
            log::error!("imagegen attachment preparation failed: {error}");
            status
                .edit(
                    ctx,
                    status_reply(
                        &prompt,
                        Stage::Failed,
                        Some("Could not prepare the Discord attachment."),
                    ),
                )
                .await?;
            return Ok(());
        }
    };
    if let Err(error) = status
        .edit(
            ctx,
            status_reply(&prompt, Stage::Ready, None).attachment(attachment),
        )
        .await
    {
        log::error!(
            "imagegen Discord upload failed user={user} guild={guild} channel={channel}: {error}"
        );
        let _ = status
            .edit(
                ctx,
                status_reply(
                    &prompt,
                    Stage::Failed,
                    Some("Discord could not upload the generated image."),
                ),
            )
            .await;
        return Ok(());
    }
    log::info!(
        "imagegen completed user={user} guild={guild} channel={channel} duration_ms={} image_bytes={}",
        started.elapsed().as_millis(),
        bytes.len()
    );
    Ok(())
}

/// Stop your image request; server and bot owners can also stop requests they manage.
#[poise::command(slash_command, guild_only, category = "AI")]
pub async fn imagegenstop(ctx: CustomContext<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild = ctx.guild_id().ok_or("Server required")?.get();
    let user = ctx.author().id.get();
    let bot_owner = ctx.framework().options().owners.contains(&ctx.author().id);
    let guild_owner = ctx
        .guild()
        .is_some_and(|current| current.owner_id == ctx.author().id);
    let message = match request_stop(guild, user, bot_owner, guild_owner) {
        Ok(mut done) => match wait_for_stop(&mut done).await {
            Some(StopOutcome::QueuedStopped) => {
                "Your image request was removed from the queue; it had not loaded the model."
            }
            Some(StopOutcome::StoppedUnloaded) => {
                "Image generation stopped and the FLUX model was unloaded from Ollama. macOS may take a moment to reclaim the RAM."
            }
            Some(StopOutcome::StoppedUnconfirmed) => {
                "Image generation stopped, but I could not confirm that Ollama unloaded the model."
            }
            Some(StopOutcome::Finished) => "The image request finished before it could be stopped.",
            _ => "Stop requested, but model unloading has not been confirmed yet.",
        },
        Err(message) => message,
    };
    ctx.send(CreateReply::default().content(message).ephemeral(true))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    static ADMISSION_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn prompt_validation() {
        assert_eq!(validate_prompt("  a gray cat  ").unwrap(), "a gray cat");
        assert!(validate_prompt(" \n ").is_err());
        assert!(validate_prompt(&"a".repeat(MAX_PROMPT_CHARS + 1)).is_err());
        assert!(validate_prompt("cat\0").is_err());
    }

    #[test]
    fn public_stages_and_registration() {
        let command = imagegen();
        assert_eq!(command.name, "imagegen");
        assert_eq!(
            command.description.as_deref(),
            Some("Generate an image locally with FLUX.2 Klein 4B.")
        );
        assert!(command.slash_action.is_some());
        let alias = imagine();
        assert_eq!(alias.name, "imagine");
        assert_eq!(alias.description, command.description);
        assert!(alias.slash_action.is_some());
        for stage in [
            Stage::Received,
            Stage::Queued,
            Stage::Sending,
            Stage::Generating,
            Stage::InProgress,
            Stage::Finalizing,
            Stage::Ready,
            Stage::Failed,
            Stage::Stopped,
        ] {
            let reply = status_reply("a gray cat", stage, None);
            assert_eq!(reply.ephemeral, Some(false));
            assert_eq!(reply.embeds.len(), 1);
        }
    }

    #[test]
    fn queue_enforces_user_guild_and_cooldown() {
        let _lock = ADMISSION_TEST_LOCK.lock().unwrap();
        let first = admit(991_000_001, 991_000_002).ok().unwrap();
        assert!(admit(991_000_001, 991_000_003).is_err());
        assert!(admit(991_000_004, 991_000_002).is_err());
        drop(first);
        assert!(admit(991_000_001, 991_000_003).is_err());
        assert!(admit(991_000_004, 991_000_002).is_ok());
        let reservations: Vec<_> = (0..QUEUE_CAPACITY)
            .map(|index| {
                admit(992_000_000 + index as u64, 993_000_000 + index as u64)
                    .ok()
                    .unwrap()
            })
            .collect();
        assert!(matches!(
            admit(994_000_000, 995_000_000),
            Err("The image generation queue is full. Try again shortly.")
        ));
        drop(reservations);
    }

    #[test]
    fn stop_respects_requester_and_owner_scopes() {
        let _lock = ADMISSION_TEST_LOCK.lock().unwrap();
        let own_guild = 996_000_001;
        let other_guild = 996_000_002;
        let job = admit(996_000_003, own_guild).unwrap();
        assert!(request_stop(own_guild, 996_000_004, false, false).is_err());
        assert!(request_stop(other_guild, 996_000_003, false, false).is_err());
        let done = request_stop(own_guild, 996_000_003, false, false).unwrap();
        assert!(job.is_cancelled());
        job.complete(StopOutcome::QueuedStopped);
        drop(job);
        assert_eq!(*done.borrow(), StopOutcome::QueuedStopped);

        let generating = admit(996_000_005, own_guild).unwrap();
        generating.set_phase(JobPhase::Generating);
        let done = request_stop(other_guild, 996_000_006, true, false).unwrap();
        assert!(generating.is_cancelled());
        drop(generating);
        assert_eq!(*done.borrow(), StopOutcome::Finished);

        let server_job = admit(996_000_007, own_guild).unwrap();
        assert!(request_stop(own_guild, 996_000_008, false, true).is_ok());
        assert!(server_job.is_cancelled());
        drop(server_job);
    }

    #[test]
    fn malformed_and_missing_images_are_rejected() {
        assert!(decode_image(&serde_json::json!({})).is_err());
        assert!(decode_image(&serde_json::json!({"image":"not base64"})).is_err());
    }

    #[test]
    fn host_disk_guard_requires_ten_gibibytes() {
        assert!(ensure_host_space(MIN_HOST_FREE_BYTES - 1).is_err());
        assert!(ensure_host_space(MIN_HOST_FREE_BYTES).is_ok());
        #[cfg(unix)]
        assert!(available_disk_bytes(&std::env::temp_dir()).unwrap() > 0);
    }

    #[test]
    fn temporary_image_is_removed() {
        let path = {
            let image = TempImage::create(b"test").unwrap();
            assert!(image.0.exists());
            image.0.clone()
        };
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn ollama_adapter_accepts_real_png_response() {
        use axum::{Json, Router, routing::post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/generate",
            post(|Json(request): Json<serde_json::Value>| async move {
                assert_eq!(request["model"], MODEL);
                assert_eq!(request["prompt"], "a gray cat");
                assert_eq!(request["stream"], false);
                assert_eq!(request["width"], IMAGE_SIZE);
                assert_eq!(request["height"], IMAGE_SIZE);
                assert_eq!(request["keep_alive"], 0);
                let mut png = Cursor::new(Vec::new());
                image::DynamicImage::new_rgb8(1, 1)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .unwrap();
                Json(serde_json::json!({"image": STANDARD.encode(png.into_inner())}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bytes = generate(&format!("http://{address}"), "a gray cat")
            .await
            .unwrap();
        assert_eq!(
            image::guess_format(&bytes).unwrap(),
            image::ImageFormat::Png
        );
        server.abort();
    }

    #[tokio::test]
    async fn ollama_adapter_handles_missing_service_and_model() {
        use axum::{Router, http::StatusCode, routing::post};
        assert!(matches!(
            generate("https://example.com", "cat").await,
            Err(GenerateError::Unavailable)
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/api/generate", post(|| async { StatusCode::NOT_FOUND }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(matches!(
            generate(&format!("http://{address}"), "cat").await,
            Err(GenerateError::ModelMissing)
        ));
        server.abort();
    }

    #[tokio::test]
    async fn stop_unloads_only_the_image_model_and_confirms_it_is_gone() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/api/generate",
                post(|Json(request): Json<serde_json::Value>| async move {
                    assert_eq!(request["model"], MODEL);
                    assert_eq!(request["prompt"], "");
                    assert_eq!(request["keep_alive"], 0);
                    Json(serde_json::json!({"done": true, "done_reason": "unload"}))
                }),
            )
            .route(
                "/api/ps",
                get(|| async { Json(serde_json::json!({"models": []})) }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unload_model(&format!("http://{address}")).await.unwrap();
        assert!(matches!(
            unload_model("http://example.com").await,
            Err(GenerateError::Unavailable)
        ));
        server.abort();
    }
}
