//! Notifications: when and where a runbook reports its runs, and email settings.


use serde::{Deserialize, Serialize};


use super::*;

/// When a notification is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyEvent {
    Started,
    Succeeded,
    Failed,
    /// Still running after `slow_after_seconds`; sent once per run.
    Slow,
}

impl NotifyEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyEvent::Started => "started",
            NotifyEvent::Succeeded => "succeeded",
            NotifyEvent::Failed => "failed",
            NotifyEvent::Slow => "slow",
        }
    }
}

/// Tell people about this runbook's runs, by email or with a webhook call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    #[serde(default)]
    pub id: String,
    pub on: Vec<NotifyEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_after_seconds: Option<u64>,
    /// Email recipients; needs an SMTP server in the server config.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub email: Vec<String>,
    /// Or a URL that gets a JSON POST (Slack and similar incoming webhooks work).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    /// Extra headers for the webhook call. Values may use `{{ secrets.NAME }}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<WebhookHeader>,
    /// Sign the webhook call in with a vault secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebhookAuth>,
    /// A JSON body to send instead of the default one. Strings in it may use
    /// placeholders like `{{ run.status }}` and `{{ secrets.NAME }}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookHeader {
    pub name: String,
    pub value: String,
}

/// `Authorization` for a webhook call, from a vault secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WebhookAuth {
    /// `Authorization: Bearer <secret>`
    Bearer { secret: String },
    /// `Authorization: Basic base64(<username>:<secret>)`
    Basic { username: String, secret: String },
}

impl WebhookAuth {
    pub fn secret(&self) -> &str {
        match self {
            WebhookAuth::Bearer { secret } | WebhookAuth::Basic { secret, .. } => secret,
        }
    }
}

/// Headers people may not set: the server sets them, or they'd break the request.
const RESERVED_HEADERS: &[&str] = &["host", "content-length", "content-type", "transfer-encoding", "connection", "x-opskills-event"];

/// A piece of a header value: text, or the value of a vault secret.
#[derive(Debug, PartialEq)]
pub enum HeaderPart {
    Text(String),
    Secret(String),
}

/// A piece of a template string: text, or a `{{ name }}` placeholder.
#[derive(Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Var(String),
}

/// Split a string into text and `{{ name }}` placeholders.
pub fn placeholders(value: &str) -> Result<Vec<Piece>, String> {
    let mut parts = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        if start > 0 {
            parts.push(Piece::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or("a {{ has no closing }}.")?;
        parts.push(Piece::Var(after[..end].trim().to_string()));
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        parts.push(Piece::Text(rest.to_string()));
    }
    Ok(parts)
}

/// Split a header value into text and `{{ secrets.NAME }}` references.
pub fn header_parts(value: &str) -> Result<Vec<HeaderPart>, String> {
    placeholders(value)?
        .into_iter()
        .map(|p| match p {
            Piece::Text(t) => Ok(HeaderPart::Text(t)),
            Piece::Var(v) => {
                let name = v.strip_prefix("secrets.").ok_or_else(|| format!("only {{{{ secrets.NAME }}}} can be used in a header, not {{{{ {v} }}}}."))?;
                validate_secret_ref(name)?;
                Ok(HeaderPart::Secret(name.to_string()))
            }
        })
        .collect()
}

/// Placeholders a custom webhook body can use: the fields of the default body,
/// plus vault secrets.
const BODY_VARS: &[&str] = &[
    "event",
    "text",
    "content",
    "runbook.id",
    "runbook.name",
    "run.id",
    "run.number",
    "run.status",
    "run.trigger",
    "run.started_by",
    "run.started_at",
    "run.finished_at",
    "run.duration_seconds",
    "run.url",
    "run.error",
    "run.failed_step.name",
    "run.failed_step.host",
    "run.failed_step.exit_code",
    "output_tail",
];

fn check_body_var(v: &str) -> Result<(), String> {
    if let Some(name) = v.strip_prefix("secrets.") {
        return validate_secret_ref(name);
    }
    if let Some(name) = v.strip_prefix("run.inputs.")
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Ok(());
    }
    if BODY_VARS.contains(&v) {
        return Ok(());
    }
    Err(format!("{{{{ {v} }}}} is not something the body can use. Use one of {}, run.inputs.NAME or secrets.NAME.", BODY_VARS.join(", ")))
}

/// The strings of a JSON document, wherever they are (keys aren't filled in).
fn json_strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| json_strings(x, out)),
        serde_json::Value::Object(m) => m.values().for_each(|x| json_strings(x, out)),
        _ => {}
    }
}

/// Check a custom body and return the placeholders it uses.
pub fn body_vars(template: &str) -> Result<Vec<String>, String> {
    if template.len() > 16 * 1024 {
        return Err("the body can be at most 16 KB.".into());
    }
    let doc: serde_json::Value = serde_json::from_str(template).map_err(|e| format!("the body must be JSON: {e}."))?;
    if !(doc.is_object() || doc.is_array()) {
        return Err("the body must be a JSON object or array.".into());
    }
    let mut strings = Vec::new();
    json_strings(&doc, &mut strings);
    let mut vars = Vec::new();
    for s in &strings {
        for p in placeholders(s)? {
            if let Piece::Var(v) = p {
                check_body_var(&v)?;
                vars.push(v);
            }
        }
    }
    Ok(vars)
}

pub const MAX_NOTIFICATIONS: usize = 10;

/// SMTP settings made on the Settings page. The password is a vault secret, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailSettings {
    pub smtp_host: String,
    #[serde(default = "smtp_port")]
    pub smtp_port: u16,
    #[serde(default = "starttls")]
    pub security: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_secret: Option<String>,
    pub from: String,
}

fn smtp_port() -> u16 {
    587
}

fn starttls() -> String {
    "starttls".into()
}

impl EmailSettings {
    pub fn validate(&mut self) -> Result<(), String> {
        self.smtp_host = self.smtp_host.trim().to_string();
        if self.smtp_host.is_empty() || self.smtp_host.len() > 253 || self.smtp_host.contains(char::is_whitespace) {
            return Err("Give the SMTP server's host name, like smtp.example.com.".into());
        }
        if self.smtp_port == 0 {
            return Err("The SMTP port must be 1 to 65535.".into());
        }
        if !["starttls", "tls", "none"].contains(&self.security.as_str()) {
            return Err("Security must be starttls, tls or none.".into());
        }
        self.username = self.username.take().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        self.password_secret = self.password_secret.take().filter(|s| !s.is_empty());
        if self.username.is_some() != self.password_secret.is_some() {
            return Err("Give both a user name and a password secret, or neither.".into());
        }
        if let Some(s) = &self.password_secret {
            validate_secret_ref(s)?;
        }
        self.from = self.from.trim().to_string();
        self.from.parse::<lettre::message::Mailbox>().map_err(|_| format!("{:?} is not a sender address. Use one like OpSkills <opskills@example.com>.", self.from))?;
        Ok(())
    }
}

impl Notification {
    pub fn validate(&mut self) -> Result<(), String> {
        if self.id.is_empty() {
            self.id = hex::encode(crate::firststart::random_bytes::<6>());
        }
        self.on.sort_unstable();
        self.on.dedup();
        if self.on.is_empty() {
            return Err("pick when to notify: started, succeeded, failed or slow.".into());
        }
        if self.on.contains(&NotifyEvent::Slow) {
            match self.slow_after_seconds {
                Some(s) if (1..=7 * 86_400).contains(&s) => {}
                _ => return Err("\"slow\" needs slow_after_seconds, from 1 second to 7 days.".into()),
            }
        } else {
            self.slow_after_seconds = None;
        }
        self.email = self.email.iter().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
        self.email.dedup();
        self.webhook = self.webhook.take().map(|w| w.trim().to_string()).filter(|w| !w.is_empty());
        match (self.email.is_empty(), &self.webhook) {
            (false, None) => {
                if self.email.len() > 20 {
                    return Err("send to at most 20 email addresses.".into());
                }
                for e in &self.email {
                    e.parse::<lettre::Address>().map_err(|_| format!("{e:?} is not an email address."))?;
                }
            }
            (true, Some(url)) => check_webhook_url(url)?,
            _ => return Err("send either to email addresses or to a webhook URL.".into()),
        }
        self.body = self.body.take().filter(|b| !b.trim().is_empty());
        if self.webhook.is_none() && (!self.headers.is_empty() || self.auth.is_some() || self.body.is_some()) {
            return Err("headers, sign-in and a custom body are only for webhooks.".into());
        }
        if let Some(b) = &self.body {
            body_vars(b)?;
        }
        if self.headers.len() > 10 {
            return Err("a webhook can have at most 10 headers.".into());
        }
        let mut seen: Vec<String> = Vec::new();
        for h in &mut self.headers {
            h.name = h.name.trim().to_string();
            let lower = h.name.to_ascii_lowercase();
            if h.name.is_empty() || h.name.len() > 64 || !h.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(format!("{:?} is not a header name. Use letters, digits and dashes, like X-Api-Key.", h.name));
            }
            if RESERVED_HEADERS.contains(&lower.as_str()) {
                return Err(format!("the {} header is set by OpSkills.", h.name));
            }
            if lower == "authorization" && self.auth.is_some() {
                return Err("set Authorization either as a header or with sign-in, not both.".into());
            }
            if seen.contains(&lower) {
                return Err(format!("the {} header is listed twice.", h.name));
            }
            seen.push(lower);
            if h.value.len() > 1000 || h.value.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
                return Err(format!("the {} header's value must be one line of at most 1000 characters.", h.name));
            }
            header_parts(&h.value).map_err(|e| format!("{}: {e}", h.name))?;
        }
        if let Some(auth) = &mut self.auth {
            validate_secret_ref(auth.secret())?;
            if let WebhookAuth::Basic { username, .. } = auth {
                *username = username.trim().to_string();
                if username.is_empty() || username.len() > 200 || username.contains(':') || username.chars().any(char::is_control) {
                    return Err("basic sign-in needs a user name without a colon.".into());
                }
            }
        }
        Ok(())
    }

    /// The vault secrets this notification uses.
    pub fn secret_refs(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .headers
            .iter()
            .flat_map(|h| header_parts(&h.value).unwrap_or_default())
            .filter_map(|p| match p {
                HeaderPart::Secret(s) => Some(s),
                HeaderPart::Text(_) => None,
            })
            .chain(self.auth.as_ref().map(|a| a.secret().to_string()))
            .chain(self.body.as_deref().map(|b| body_vars(b).unwrap_or_default()).unwrap_or_default().into_iter().filter_map(|v| v.strip_prefix("secrets.").map(str::to_string)))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Who gets it, for logs and the run page. A webhook shows only its host, since
    /// the rest of the URL is often the secret (as with Slack).
    pub fn target_label(&self) -> String {
        match &self.webhook {
            Some(url) => format!("webhook {}", webhook_host(url)),
            None => self.email.join(", "),
        }
    }
}

pub fn check_webhook_url(url: &str) -> Result<(), String> {
    let uri: axum::http::Uri = url.parse().map_err(|_| format!("{url:?} is not a URL."))?;
    let ok_scheme = matches!(uri.scheme_str(), Some("https" | "http"));
    if !ok_scheme || uri.host().is_none_or(str::is_empty) || url.len() > 2000 {
        return Err("a webhook URL starts with https:// (or http://) and has a host.".into());
    }
    Ok(())
}

pub fn webhook_host(url: &str) -> String {
    url.parse::<axum::http::Uri>().ok().and_then(|u| u.host().map(str::to_string)).unwrap_or_else(|| "?".into())
}
