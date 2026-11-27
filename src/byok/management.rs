use super::storage::{Limits, UsageSummary};
use crate::automod::config::{AutoModConfig, Sensitivity};
use serde::Deserialize;
use std::time::Duration;

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn checked(value: bool) -> &'static str {
    if value { "checked" } else { "" }
}

fn option(value: &str, current: &str) -> String {
    format!(
        "<option value=\"{value}\" {}>{value}</option>",
        if value == current { "selected" } else { "" }
    )
}

#[derive(Deserialize)]
pub(super) struct ManagementForm {
    pub csrf: String,
    automod_enabled: Option<String>,
    warnings_enabled: Option<String>,
    timeouts_enabled: Option<String>,
    notify_owner: Option<String>,
    sensitivity: String,
    moderate_minutes: String,
    serious_minutes: String,
    severe_minutes: String,
    extreme_minutes: String,
    user_daily: String,
    guild_daily: String,
    output_tokens: String,
    concurrency: String,
    vision_enabled: Option<String>,
    max_images: String,
    dm_vision_enabled: Option<String>,
}

fn number(value: &str, min: i32, max: i32, name: &'static str) -> Result<i32, &'static str> {
    let parsed = value.parse::<i32>().map_err(|_| name)?;
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(name)
    }
}

impl ManagementForm {
    pub(super) fn apply(
        &self,
        automod: &mut AutoModConfig,
        limits: &mut Limits,
    ) -> Result<(), &'static str> {
        let Some(sensitivity) = Sensitivity::parse(&self.sensitivity) else {
            return Err("Choose a valid AutoMod sensitivity.");
        };
        automod.enabled = self.automod_enabled.is_some();
        automod.warnings_enabled = self.warnings_enabled.is_some();
        automod.timeouts_enabled = self.timeouts_enabled.is_some();
        automod.notify_owner = self.notify_owner.is_some();
        automod.sensitivity = sensitivity;
        automod.moderate_timeout = Duration::from_secs(
            number(
                &self.moderate_minutes,
                1,
                2880,
                "Moderate timeout must be 1–2880 minutes.",
            )? as u64
                * 60,
        );
        automod.serious_timeout = Duration::from_secs(
            number(
                &self.serious_minutes,
                1,
                2880,
                "Serious timeout must be 1–2880 minutes.",
            )? as u64
                * 60,
        );
        automod.severe_timeout = Duration::from_secs(
            number(
                &self.severe_minutes,
                1,
                2880,
                "Severe timeout must be 1–2880 minutes.",
            )? as u64
                * 60,
        );
        automod.extreme_timeout = Duration::from_secs(
            number(
                &self.extreme_minutes,
                1,
                2880,
                "Extreme timeout must be 1–2880 minutes.",
            )? as u64
                * 60,
        );
        limits.user_daily = number(
            &self.user_daily,
            1,
            1000,
            "Per-user daily limit must be 1–1000.",
        )?;
        limits.guild_daily = number(
            &self.guild_daily,
            1,
            100_000,
            "Server daily limit must be 1–100000.",
        )?;
        limits.output_tokens = number(
            &self.output_tokens,
            32,
            4096,
            "Output token limit must be 32–4096.",
        )?;
        limits.concurrency = number(
            &self.concurrency,
            1,
            16,
            "Concurrent request limit must be 1–16.",
        )? as u32;
        limits.vision_enabled = Some(self.vision_enabled.is_some());
        limits.max_images = number(&self.max_images, 1, 3, "Image limit must be 1–3.")? as usize;
        limits.dm_vision_enabled = self.dm_vision_enabled.is_some();
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    guild_name: &str,
    guild: u64,
    csrf: &str,
    page: &str,
    automod: &AutoModConfig,
    limits: &Limits,
    usage: &UsageSummary,
    provider: &str,
    local_approved: bool,
) -> String {
    let name = esc(guild_name);
    let provider = esc(provider);
    let csrf = esc(csrf);
    let page = esc(page);
    let sensitivity = ["low", "normal", "high"]
        .iter()
        .map(|v| option(v, automod.sensitivity.name()))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Manage {name} · OmniCore</title><style>
*{{box-sizing:border-box}}body{{margin:0;background:#0b1222;color:#eef3ff;font:16px system-ui,sans-serif;line-height:1.5}}main{{width:min(900px,calc(100% - 32px));margin:32px auto 80px}}header{{border-bottom:1px solid #34435f;padding-bottom:20px}}h1{{font-size:clamp(28px,5vw,42px);line-height:1.1;margin:8px 0}}h2{{margin:0 0 12px;font-size:23px}}p{{color:#c1cee5}}.eyebrow{{color:#75d6e9;font-weight:700;letter-spacing:.12em;text-transform:uppercase;font-size:12px}}.panel{{background:#18243a;border:1px solid #324463;border-radius:14px;padding:24px;margin:20px 0}}.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:18px}}label{{display:block;color:#eaf0fc;font-weight:600}}label span{{display:block;color:#afbed6;font-size:13px;font-weight:400}}input[type=number],select{{width:100%;padding:10px;margin-top:6px;background:#0c1729;color:white;border:1px solid #657798;border-radius:7px;font:inherit}}.check{{display:flex;align-items:flex-start;gap:10px;margin:10px 0}}.check input{{width:19px;height:19px;flex:none;accent-color:#67ccf0}}.metric{{font-size:25px;font-weight:700;margin:0}}.muted{{color:#aabbd2;font-size:14px}}button{{background:#57c6e7;border:0;border-radius:8px;padding:13px 22px;color:#071321;font:inherit;font-weight:800;cursor:pointer}}button:hover{{background:#8cdeee}}a{{color:#8cdeee}}:focus-visible{{outline:3px solid #f5c95c;outline-offset:3px}}
</style></head><body><main><header><div class="eyebrow">OmniCore server management</div><h1>{name}</h1><p>Server ID: {guild}. Signed in with Discord. Your private link expires in 10 minutes and can be saved once.</p></header>
<section class="panel"><h2>Provider AI connection and usage</h2><p>Connected provider: {provider}. Local AI operator approval: {}.</p><div class="grid"><div><p class="metric">{}</p><span class="muted">Provider requests, past 30 days</span></div><div><p class="metric">{}</p><span class="muted">Successful</span></div><div><p class="metric">{}</p><span class="muted">Failed</span></div><div><p class="metric">{} / {}</p><span class="muted">Input / output tokens reported</span></div><div><p class="metric">{}</p><span class="muted">Provider vision requests</span></div></div><p class="muted">Local Ollama requests are not included. Token totals reflect provider reports and may be incomplete. Use /ai setup to connect or change a provider; credentials are never shown here.</p></section>
<form action="/connect/management/{page}" method="post"><input type="hidden" name="csrf" value="{csrf}">
<section class="panel"><h2>AutoMod</h2><label class="check"><input type="checkbox" name="automod_enabled" value="on" {}> Enable AutoMod</label><label class="check"><input type="checkbox" name="warnings_enabled" value="on" {}> Issue warnings</label><label class="check"><input type="checkbox" name="timeouts_enabled" value="on" {}> Apply automatic timeouts</label><label class="check"><input type="checkbox" name="notify_owner" value="on" {}> Notify server owner</label><div class="grid"><label>Sensitivity<select name="sensitivity">{sensitivity}</select></label><label>Moderate timeout (minutes)<input type="number" name="moderate_minutes" min="1" max="2880" required value="{}"></label><label>Serious timeout (minutes)<input type="number" name="serious_minutes" min="1" max="2880" required value="{}"></label><label>Severe timeout (minutes)<input type="number" name="severe_minutes" min="1" max="2880" required value="{}"></label><label>Extreme timeout (minutes)<input type="number" name="extreme_minutes" min="1" max="2880" required value="{}"></label></div></section>
<section class="panel"><h2>AI limits</h2><p class="muted">These limits apply to a connected provider. Operator-approved local AI follows its own route.</p><div class="grid"><label>Requests per user per day<input type="number" name="user_daily" min="1" max="1000" required value="{}"></label><label>Requests per server per day<input type="number" name="guild_daily" min="1" max="100000" required value="{}"></label><label>Maximum output tokens<input type="number" name="output_tokens" min="32" max="4096" required value="{}"></label><label>Concurrent requests<input type="number" name="concurrency" min="1" max="16" required value="{}"></label></div><label class="check"><input type="checkbox" name="vision_enabled" value="on" {}> Allow provider image recognition</label><div class="grid"><label>Maximum images per request<input type="number" name="max_images" min="1" max="3" required value="{}"></label></div><label class="check"><input type="checkbox" name="dm_vision_enabled" value="on" {}> Allow server-owner DM vision with this provider key</label><p class="muted">Provider usage may incur charges. Existing channel, role, and model choices are preserved. Use /ai commands for those settings.</p></section>
<button type="submit">Save changes and close link</button></form><p class="muted">After Save, this page and its link cannot be reused. Run /management again to make more changes.</p></main></body></html>"#,
        if local_approved {
            "Approved"
        } else {
            "Not approved"
        },
        usage.requests,
        usage.successful,
        usage.failures,
        usage.input_tokens,
        usage.output_tokens,
        usage.vision_requests,
        checked(automod.enabled),
        checked(automod.warnings_enabled),
        checked(automod.timeouts_enabled),
        checked(automod.notify_owner),
        automod.moderate_timeout.as_secs() / 60,
        automod.serious_timeout.as_secs() / 60,
        automod.severe_timeout.as_secs() / 60,
        automod.extreme_timeout.as_secs() / 60,
        limits.user_daily,
        limits.guild_daily,
        limits.output_tokens,
        limits.concurrency,
        checked(limits.vision_enabled != Some(false)),
        limits.max_images,
        checked(limits.dm_vision_enabled)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_limits_and_preserves_unedited_settings() {
        let mut a = AutoModConfig::default();
        let mut l = Limits {
            channels: vec![123],
            vision_model: Some("model".to_string()),
            ..Default::default()
        };
        let form = ManagementForm {
            csrf: "x".into(),
            automod_enabled: Some("on".into()),
            warnings_enabled: None,
            timeouts_enabled: Some("on".into()),
            notify_owner: None,
            sensitivity: "high".into(),
            moderate_minutes: "2".into(),
            serious_minutes: "10".into(),
            severe_minutes: "30".into(),
            extreme_minutes: "60".into(),
            user_daily: "50".into(),
            guild_daily: "500".into(),
            output_tokens: "1024".into(),
            concurrency: "3".into(),
            vision_enabled: Some("on".into()),
            max_images: "2".into(),
            dm_vision_enabled: None,
        };
        form.apply(&mut a, &mut l).unwrap();
        assert_eq!(a.sensitivity, Sensitivity::High);
        assert!(!a.warnings_enabled);
        assert_eq!(l.output_tokens, 1024);
        assert_eq!(l.channels, vec![123]);
        assert_eq!(l.vision_model.as_deref(), Some("model"));
        let bad = ManagementForm {
            output_tokens: "9000".into(),
            ..form
        };
        assert!(bad.apply(&mut a, &mut l).is_err());
    }

    #[test]
    fn escapes_server_and_model_names_in_management_page() {
        let html = render(
            "<script>alert(1)</script>",
            42,
            "csrf",
            "page",
            &AutoModConfig::default(),
            &Limits::default(),
            &UsageSummary::default(),
            "model\" onfocus=\"alert(1)",
            false,
        );
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("model&quot; onfocus=&quot;alert(1)"));
        assert!(html.contains("action=\"/connect/management/page\""));
    }
}
