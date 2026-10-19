//! The only entry point for provider API keys. No key is accepted in Discord.
//! Pairing, OAuth state and sessions are short-lived and process local; the
//! connected credential is encrypted before MongoDB persistence.
use super::{provider::AiProvider, storage::ByokService};
use axum::{
    Form, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use once_cell::sync::OnceCell;
use rand::RngCore;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const COOKIE: &str = "omnicore_byok_session";
const OAUTH_COOKIE: &str = "omnicore_byok_oauth";
const LINK_LIFE: Duration = Duration::from_secs(600);
const SESSION_LIFE: Duration = Duration::from_secs(1800);

static PORTAL: OnceCell<Arc<Portal>> = OnceCell::new();

struct Link {
    guild: u64,
    until: Instant,
}
struct OAuthState {
    link: String,
    verifier: Zeroizing<String>,
    until: Instant,
}
struct Pending {
    provider: AiProvider,
    key: Zeroizing<String>,
    models: Vec<String>,
    until: Instant,
}
struct Session {
    guild: u64,
    user: u64,
    csrf: String,
    until: Instant,
    attempts: u8,
    pending: Option<Pending>,
}

struct Portal {
    site: String,
    callback: String,
    client_id: String,
    client_secret: Zeroizing<String>,
    bot_token: Zeroizing<String>,
    service: Arc<ByokService>,
    http: reqwest::Client,
    links: Mutex<HashMap<String, Link>>,
    oauth: Mutex<HashMap<String, OAuthState>>,
    sessions: Mutex<HashMap<String, Session>>,
}

fn random() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(body: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>OmniCore secure AI setup</title><style>body{{background:#0d1120;color:#eef1ff;font:16px system-ui;margin:0;padding:3rem 1rem}}main{{max-width:640px;margin:auto}}section{{background:#1b2338;padding:1.5rem;border-radius:12px;margin:1rem 0}}label{{display:block;margin-top:1rem}}input,select,button{{box-sizing:border-box;width:100%;font:inherit;padding:.7rem;margin-top:.4rem;border-radius:7px;border:1px solid #7784aa}}button{{background:#7b72f4;color:white;cursor:pointer}}a{{color:#aaa5ff}}small{{color:#c6cee5}}</style></head><body><main><h1>OmniCore AI setup</h1>{body}<small>API usage may incur charges billed directly by your AI provider. Keys are encrypted before storage. Never enter an API key in Discord. Read our <a href=\"/privacy/\">Privacy Policy</a> and <a href=\"/terms/\">Terms of Service</a>.</small></main></body></html>"
    );
    let mut response = Html(html).into_response();
    secure_headers(&mut response);
    response
}

fn secure_headers(response: &mut Response) {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
}

fn error_page(status: StatusCode, message: &str) -> Response {
    let mut response = page(&format!("<section><p>{}</p></section>", esc(message)));
    *response.status_mut() = status;
    response
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|entry| {
            let (key, value) = entry.trim().split_once('=')?;
            (key == name
                && value.len() == 43
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            .then(|| value.to_string())
        })
}

fn origin_ok(headers: &HeaderMap, site: &str) -> bool {
    headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(site)
}

fn public_site(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then(|| url.origin().ascii_serialization())
}

fn set_cookie(response: &mut Response, name: &str, value: &str, age: u32) {
    if let Ok(value) = HeaderValue::from_str(&format!(
        "{name}={value}; Max-Age={age}; Path=/connect; HttpOnly; Secure; SameSite=Lax"
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn redirect(location: &str) -> Response {
    let mut response = Redirect::to(location).into_response();
    secure_headers(&mut response);
    response
}

pub async fn create_setup_link(guild: u64) -> Result<Option<String>, &'static str> {
    let Some(portal) = PORTAL.get() else {
        return Ok(None);
    };
    let token = random();
    let mut links = portal.links.lock().await;
    links.retain(|_, link| link.until > Instant::now());
    if links.values().filter(|link| link.guild == guild).count() >= 5 {
        return Err("Too many setup links for this server; wait ten minutes");
    }
    links.insert(
        token.clone(),
        Link {
            guild,
            until: Instant::now() + LINK_LIFE,
        },
    );
    Ok(Some(format!("{}/connect/?token={token}", portal.site)))
}

pub fn maybe_start(service: Option<Arc<ByokService>>, bot_token: &str) {
    let Some(service) = service else { return };
    let (Ok(client_id), Ok(client_secret)) = (
        std::env::var("OMNICORE_DISCORD_CLIENT_ID"),
        std::env::var("OMNICORE_DISCORD_CLIENT_SECRET"),
    ) else {
        log::warn!("Discord OAuth configuration is missing; secure pairing portal is disabled");
        return;
    };
    if !client_id.bytes().all(|b| b.is_ascii_digit())
        || client_id.is_empty()
        || client_secret.len() < 16
    {
        log::error!("Invalid Discord OAuth configuration; secure pairing portal is disabled");
        return;
    }
    let Some(site) = std::env::var("OMNICORE_PUBLIC_BASE_URL")
        .ok()
        .as_deref()
        .and_then(public_site)
    else {
        log::error!(
            "Set OMNICORE_PUBLIC_BASE_URL to a public HTTPS origin to enable secure pairing"
        );
        return;
    };
    let portal = Arc::new(Portal {
        callback: format!("{site}/connect/callback"),
        site,
        client_id,
        client_secret: Zeroizing::new(client_secret),
        bot_token: Zeroizing::new(bot_token.to_owned()),
        service,
        http: super::provider::client(),
        links: Mutex::new(HashMap::new()),
        oauth: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
    });
    if PORTAL.set(portal.clone()).is_err() {
        return;
    }
    let cleanup = portal.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(60));
        loop {
            every.tick().await;
            let now = Instant::now();
            cleanup
                .links
                .lock()
                .await
                .retain(|_, link| link.until > now);
            cleanup
                .oauth
                .lock()
                .await
                .retain(|_, state| state.until > now);
            let mut sessions = cleanup.sessions.lock().await;
            sessions.retain(|_, session| session.until > now);
            for session in sessions.values_mut() {
                if session
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.until <= now)
                {
                    session.pending = None;
                }
            }
        }
    });
    tokio::spawn(async move {
        let app = Router::new()
            .route("/connect/", get(start))
            .route("/connect/callback", get(callback))
            .route("/connect/manage", get(manage))
            .route("/connect/validate", post(validate))
            .route("/connect/pair", post(pair))
            .route("/connect/disconnect", post(disconnect))
            .layer(axum::extract::DefaultBodyLimit::max(8_192))
            .with_state(portal);
        let listener = match tokio::net::TcpListener::bind("0.0.0.0:8098").await {
            Ok(listener) => listener,
            Err(_) => {
                log::error!("Secure pairing portal could not bind its internal port");
                return;
            }
        };
        log::info!("Secure pairing portal listening on Docker-internal port 8098");
        if axum::serve(listener, app).await.is_err() {
            log::error!("Secure pairing portal stopped");
        }
    });
}

#[derive(Deserialize)]
struct StartQuery {
    token: String,
}
async fn start(State(portal): State<Arc<Portal>>, Query(query): Query<StartQuery>) -> Response {
    if query.token.len() != 43 {
        return error_page(StatusCode::BAD_REQUEST, "This setup link is invalid.");
    }
    let links = portal.links.lock().await;
    if !links
        .get(&query.token)
        .is_some_and(|link| link.until > Instant::now())
    {
        return error_page(
            StatusCode::BAD_REQUEST,
            "This setup link has expired. Run /ai setup again.",
        );
    }
    drop(links);
    let state = random();
    let verifier = Zeroizing::new(random());
    let challenge = pkce_challenge(&verifier);
    let mut states = portal.oauth.lock().await;
    states.retain(|_, auth| auth.until > Instant::now() && auth.link != query.token);
    states.insert(
        state.clone(),
        OAuthState {
            link: query.token,
            verifier,
            until: Instant::now() + LINK_LIFE,
        },
    );
    drop(states);
    let url = format!(
        "https://discord.com/oauth2/authorize?response_type=code&client_id={}&scope=identify&redirect_uri={}&state={}&code_challenge={challenge}&code_challenge_method=S256",
        portal.client_id,
        urlencoding::encode(&portal.callback),
        state
    );
    let mut response = redirect(&url);
    set_cookie(&mut response, OAUTH_COOKIE, &state, 600);
    response
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
}
async fn callback(
    State(portal): State<Arc<Portal>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let (Some(code), Some(state)) = (query.code, query.state) else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Discord sign-in was not completed.",
        );
    };
    if query.iss.as_deref() != Some("https://discord.com")
        || cookie(&headers, OAUTH_COOKIE).as_deref() != Some(&state)
    {
        return error_page(
            StatusCode::FORBIDDEN,
            "Discord sign-in state did not match.",
        );
    }
    let Some(auth) = portal.oauth.lock().await.remove(&state) else {
        return error_page(StatusCode::FORBIDDEN, "Discord sign-in expired.");
    };
    if auth.until <= Instant::now() {
        return error_page(StatusCode::FORBIDDEN, "Discord sign-in expired.");
    }
    let guild = {
        let mut links = portal.links.lock().await;
        let Some(link) = links.remove(&auth.link) else {
            return error_page(StatusCode::FORBIDDEN, "This setup link was already used.");
        };
        if link.until <= Instant::now() {
            return error_page(StatusCode::FORBIDDEN, "This setup link expired.");
        }
        link.guild
    };
    let token_result = portal
        .http
        .post("https://discord.com/api/v10/oauth2/token")
        .basic_auth(&portal.client_id, Some(portal.client_secret.as_str()))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", portal.callback.as_str()),
            ("code_verifier", auth.verifier.as_str()),
        ])
        .send()
        .await;
    let Ok(token_response) = token_result else {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "Discord sign-in is temporarily unavailable.",
        );
    };
    if !token_response.status().is_success() {
        return error_page(
            StatusCode::FORBIDDEN,
            "Discord did not accept this sign-in.",
        );
    }
    let Ok(token_body) = token_response.json::<Value>().await else {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "Discord returned an invalid sign-in response.",
        );
    };
    let Some(access_token) = token_body.get("access_token").and_then(Value::as_str) else {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "Discord did not return a sign-in token.",
        );
    };
    let user_response = portal
        .http
        .get("https://discord.com/api/v10/users/@me")
        .bearer_auth(access_token)
        .send()
        .await;
    let Ok(user_response) = user_response else {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "Could not verify your Discord identity.",
        );
    };
    let Ok(user_body) = user_response.json::<Value>().await else {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "Could not verify your Discord identity.",
        );
    };
    let Some(user) = user_body
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<u64>().ok())
    else {
        return error_page(
            StatusCode::FORBIDDEN,
            "Could not verify your Discord identity.",
        );
    };
    if !portal.is_owner(guild, user).await {
        return error_page(
            StatusCode::FORBIDDEN,
            "Only the current Discord server owner can configure this server's AI provider.",
        );
    }
    let id = random();
    portal.sessions.lock().await.insert(
        id.clone(),
        Session {
            guild,
            user,
            csrf: random(),
            until: Instant::now() + SESSION_LIFE,
            attempts: 0,
            pending: None,
        },
    );
    let mut response = redirect("/connect/manage");
    set_cookie(&mut response, COOKIE, &id, 1800);
    set_cookie(&mut response, OAUTH_COOKIE, "", 0);
    response
}

impl Portal {
    async fn is_owner(&self, guild: u64, user: u64) -> bool {
        let result = self
            .http
            .get(format!("https://discord.com/api/v10/guilds/{guild}"))
            .header("Authorization", format!("Bot {}", self.bot_token.as_str()))
            .send()
            .await;
        let Ok(response) = result else { return false };
        if !response.status().is_success() {
            return false;
        }
        response.json::<Value>().await.ok().and_then(|body| {
            body.get("owner_id")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<u64>().ok())
        }) == Some(user)
    }
    async fn session(&self, headers: &HeaderMap) -> Option<(String, u64, u64, String)> {
        let id = cookie(headers, COOKIE)?;
        let sessions = self.sessions.lock().await;
        let session = sessions.get(&id)?;
        if session.until <= Instant::now() {
            return None;
        }
        Some((id, session.guild, session.user, session.csrf.clone()))
    }
}

fn form_shell(guild: u64, csrf: &str, current: &str, extra: &str) -> Response {
    page(&format!(
        "<section><p>Discord server ID: <strong>{guild}</strong></p><p>{}</p></section><section><h2>Connect or replace provider</h2><form method=\"post\" action=\"/connect/validate\" autocomplete=\"off\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><label>Provider<select name=\"provider\"><option value=\"openai\">OpenAI API</option><option value=\"anthropic\">Anthropic Claude API</option><option value=\"gemini\">Google Gemini API</option></select></label><label>API key<input type=\"password\" name=\"key\" required maxlength=\"1024\" autocomplete=\"off\"></label><button type=\"submit\">Validate key and choose model</button></form></section>{}<section><form method=\"post\" action=\"/connect/disconnect\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">Disconnect provider and delete stored key</button></form></section>",
        esc(current),
        esc(csrf),
        extra,
        esc(csrf)
    ))
}

async fn manage(State(portal): State<Arc<Portal>>, headers: HeaderMap) -> Response {
    let Some((_, guild, user, csrf)) = portal.session(&headers).await else {
        return error_page(
            StatusCode::UNAUTHORIZED,
            "Session expired. Run /ai setup in Discord again.",
        );
    };
    if !portal.is_owner(guild, user).await {
        return error_page(
            StatusCode::FORBIDDEN,
            "Server ownership could not be verified.",
        );
    }
    let current = match portal.service.connection(guild).await {
        Ok(Some(connection)) => format!(
            "Connected: {} / {}. Key stored securely; never displayed.",
            connection.provider.name(),
            connection.model
        ),
        Ok(None) => "No provider connected yet.".to_string(),
        Err(_) => {
            return error_page(
                StatusCode::SERVICE_UNAVAILABLE,
                "AI settings are temporarily unavailable.",
            );
        }
    };
    form_shell(guild, &csrf, &current, "")
}

#[derive(Deserialize)]
struct ValidateForm {
    csrf: String,
    provider: String,
    key: String,
}
async fn validate(
    State(portal): State<Arc<Portal>>,
    headers: HeaderMap,
    Form(form): Form<ValidateForm>,
) -> Response {
    if !origin_ok(&headers, &portal.site) {
        return error_page(StatusCode::FORBIDDEN, "Invalid request origin.");
    }
    let Some((id, guild, user, csrf)) = portal.session(&headers).await else {
        return error_page(StatusCode::UNAUTHORIZED, "Session expired.");
    };
    if form.csrf != csrf || !portal.is_owner(guild, user).await {
        return error_page(StatusCode::FORBIDDEN, "This request is not authorized.");
    }
    let Some(provider) = AiProvider::parse(&form.provider) else {
        return error_page(StatusCode::BAD_REQUEST, "Choose a supported provider.");
    };
    if !(10..=1024).contains(&form.key.len()) {
        return error_page(StatusCode::BAD_REQUEST, "The API key length is invalid.");
    }
    {
        let mut sessions = portal.sessions.lock().await;
        let Some(session) = sessions.get_mut(&id) else {
            return error_page(StatusCode::UNAUTHORIZED, "Session expired.");
        };
        if session.attempts >= 5 {
            return error_page(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many validation attempts. Run /ai setup later.",
            );
        }
        session.attempts += 1;
    }
    let key = Zeroizing::new(form.key);
    let models = match portal.service.list_models(provider, &key).await {
        Ok(models) if !models.is_empty() => models,
        _ => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "The provider did not accept this key or return usable models. Check the key and provider account.",
            );
        }
    };
    let options = models
        .iter()
        .take(100)
        .map(|m| format!("<option value=\"{}\">{}</option>", esc(m), esc(m)))
        .collect::<String>();
    let mut sessions = portal.sessions.lock().await;
    let Some(session) = sessions.get_mut(&id) else {
        return error_page(StatusCode::UNAUTHORIZED, "Session expired.");
    };
    session.pending = Some(Pending {
        provider,
        key,
        models,
        until: Instant::now() + LINK_LIFE,
    });
    page(&format!(
        "<section><h2>Select a model</h2><p>Key validated for {}. Choose a model to save the encrypted connection.</p><form method=\"post\" action=\"/connect/pair\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><label>Model<select name=\"model\">{}</select></label><button type=\"submit\">Connect provider</button></form></section>",
        provider.name(),
        esc(&csrf),
        options
    ))
}

#[derive(Deserialize)]
struct PairForm {
    csrf: String,
    model: String,
}
async fn pair(
    State(portal): State<Arc<Portal>>,
    headers: HeaderMap,
    Form(form): Form<PairForm>,
) -> Response {
    if !origin_ok(&headers, &portal.site) {
        return error_page(StatusCode::FORBIDDEN, "Invalid request origin.");
    }
    let Some((id, guild, user, csrf)) = portal.session(&headers).await else {
        return error_page(StatusCode::UNAUTHORIZED, "Session expired.");
    };
    if form.csrf != csrf || !portal.is_owner(guild, user).await {
        return error_page(StatusCode::FORBIDDEN, "This request is not authorized.");
    }
    let pending = portal
        .sessions
        .lock()
        .await
        .get_mut(&id)
        .and_then(|session| session.pending.take());
    let Some(pending) = pending else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Key validation expired. Start again.",
        );
    };
    if pending.until <= Instant::now() || !pending.models.iter().any(|model| model == &form.model) {
        return error_page(
            StatusCode::BAD_REQUEST,
            "The selected model is invalid or validation expired.",
        );
    }
    if portal
        .service
        .connect(guild, pending.provider, &pending.key, &form.model)
        .await
        .is_err()
    {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "The provider could not be connected. Validate the key again.",
        );
    }
    page(&format!(
        "<section><h2>Connected</h2><p>{} is connected for this server using model {}.</p><p><a href=\"/connect/manage\">Back to settings</a></p></section>",
        pending.provider.name(),
        esc(&form.model)
    ))
}

#[derive(Deserialize)]
struct CsrfForm {
    csrf: String,
}
async fn disconnect(
    State(portal): State<Arc<Portal>>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    if !origin_ok(&headers, &portal.site) {
        return error_page(StatusCode::FORBIDDEN, "Invalid request origin.");
    }
    let Some((id, guild, user, csrf)) = portal.session(&headers).await else {
        return error_page(StatusCode::UNAUTHORIZED, "Session expired.");
    };
    if form.csrf != csrf || !portal.is_owner(guild, user).await {
        return error_page(StatusCode::FORBIDDEN, "This request is not authorized.");
    }
    if portal.service.disconnect(guild).await.is_err() {
        return error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Could not disconnect this provider.",
        );
    }
    if let Some(session) = portal.sessions.lock().await.get_mut(&id) {
        session.pending = None;
    }
    page(
        "<section><h2>Disconnected</h2><p>The stored API key was deleted. Revoke it with your provider as well if you want to make it unusable outside OmniCore.</p><p><a href=\"/connect/manage\">Back to settings</a></p></section>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookie_parser_requires_exact_name_and_token_shape() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "other=abc; omnicore_byok_session=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNO12",
            ),
        );
        assert!(cookie(&headers, COOKIE).is_some());
        assert!(cookie(&headers, OAUTH_COOKIE).is_none());
    }

    #[test]
    fn pkce_uses_sha256_url_safe_encoding() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn custom_portal_origin_requires_clean_https_origin() {
        assert_eq!(
            public_site("https://example.org/"),
            Some("https://example.org".into())
        );
        for bad in [
            "http://example.org",
            "https://example.org/connect",
            "https://user@example.org",
            "https://example.org/?token=abc",
            "https://example.org/#fragment",
        ] {
            assert!(public_site(bad).is_none(), "accepted {bad}");
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.org"),
        );
        assert!(origin_ok(&headers, "https://example.org"));
        assert!(!origin_ok(&headers, "https://different.example"));
    }
}
