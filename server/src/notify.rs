//! Notifications about runs: email through the configured SMTP server, or a JSON POST
//! to a webhook URL. Sending never holds up a run, and each result is recorded on the
//! run so people can see whether a message went out.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{Config, Email};
use crate::executor::{human_duration, run_number};
use crate::model::{HeaderPart, Notification, NotifyEvent, Run, Status, WebhookAuth, header_parts, now_ms};
use crate::store::Store;
use crate::vault::{Sealed, Vault};

type Headers = Vec<(String, String)>;

/// Output lines included when a run fails.
const TAIL_LINES: usize = 20;
/// Waits between webhook attempts when the receiver is down or answers 5xx or 429.
#[cfg(not(test))]
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(10)];
#[cfg(test)]
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(20), Duration::from_millis(40)];

/// The outcome of one notification, kept with the run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notice {
    pub at: u64,
    pub event: NotifyEvent,
    pub target: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What gets sent: a subject and text for email, and a JSON body for webhooks.
pub struct Message {
    pub event: &'static str,
    pub subject: String,
    pub body: String,
    pub json: serde_json::Value,
}

pub struct Notifier {
    /// From the config file; wins over the Settings page when set.
    email: Email,
    public_url: Option<String>,
    store: Arc<Store>,
    vault: Arc<Vault>,
}

/// Where the SMTP settings come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EmailSource {
    Config,
    Settings,
    Off,
}

/// SMTP settings ready to use, with the password in hand.
struct Smtp {
    host: String,
    port: u16,
    security: String,
    credentials: Option<(String, String)>,
    from: String,
}

/// Slow-run timers for one run; dropping it (when the run ends) cancels them.
pub struct SlowTimers(Vec<tokio::task::JoinHandle<()>>);

impl Drop for SlowTimers {
    fn drop(&mut self) {
        for t in &self.0 {
            t.abort();
        }
    }
}

impl Notifier {
    pub fn new(cfg: &Config, store: Arc<Store>, vault: Arc<Vault>) -> Arc<Self> {
        let public_url = cfg.server.public_url.as_deref().map(|u| u.trim_end_matches('/').to_string());
        Arc::new(Notifier { email: cfg.email.clone(), public_url, store, vault })
    }

    pub fn email_source(&self) -> EmailSource {
        if self.email.enabled() {
            EmailSource::Config
        } else if matches!(self.store.email_settings(), Ok(Some(_))) {
            EmailSource::Settings
        } else {
            EmailSource::Off
        }
    }

    /// The config file's SMTP settings (never the password), when they're in use.
    pub fn config_email(&self) -> Option<serde_json::Value> {
        let e = &self.email;
        e.enabled().then(|| json!({ "smtp_host": e.smtp_host, "smtp_port": e.smtp_port, "security": e.security, "username": e.username, "from": e.from }))
    }

    pub fn email_enabled(&self) -> bool {
        self.email_source() != EmailSource::Off
    }

    pub fn email_from(&self) -> Option<String> {
        match self.email_source() {
            EmailSource::Config => self.email.from.clone(),
            EmailSource::Settings => self.store.email_settings().ok().flatten().map(|s| s.from),
            EmailSource::Off => None,
        }
    }

    /// The value of a vault secret.
    pub fn secret_value(&self, name: &str) -> Result<String, String> {
        let stored = self.store.secret(name).map_err(|e| e.to_string())?.ok_or_else(|| format!("the vault has no secret named {name}"))?;
        let value = self.vault.open(name, &Sealed { nonce: stored.nonce, ciphertext: stored.ciphertext }).map_err(|e| format!("opening {name}: {e}"))?;
        Ok(String::from_utf8_lossy(&value).into_owned())
    }

    fn smtp(&self) -> Result<Smtp, String> {
        let e = &self.email;
        if e.enabled() {
            return Ok(Smtp {
                host: e.smtp_host.clone().unwrap_or_default(),
                port: e.smtp_port,
                security: e.security.clone(),
                credentials: e.username.clone().zip(e.password.clone()),
                from: e.from.clone().unwrap_or_default(),
            });
        }
        let s = self.store.email_settings().map_err(|e| e.to_string())?.ok_or("Email isn't set up on this server. Set it up under Settings > Email.")?;
        let credentials = match (&s.username, &s.password_secret) {
            (Some(u), Some(secret)) => Some((u.clone(), self.secret_value(secret).map_err(|e| format!("SMTP password: {e}"))?)),
            _ => None,
        };
        Ok(Smtp { host: s.smtp_host, port: s.smtp_port, security: s.security, credentials, from: s.from })
    }

    /// The extra headers of a webhook notification, with secrets filled in.
    /// Returns the headers, and the secret values in them (to keep out of errors).
    fn webhook_headers(&self, n: &Notification) -> Result<(Headers, Vec<String>), String> {
        let mut out = Vec::new();
        let mut secrets = Vec::new();
        for h in &n.headers {
            let mut value = String::new();
            for part in header_parts(&h.value)? {
                match part {
                    HeaderPart::Text(t) => value.push_str(&t),
                    HeaderPart::Secret(name) => {
                        let v = self.secret_value(&name).map_err(|e| format!("header {}: {e}", h.name))?;
                        value.push_str(&v);
                        secrets.push(v);
                    }
                }
            }
            out.push((h.name.clone(), value));
        }
        if let Some(auth) = &n.auth {
            let secret = self.secret_value(auth.secret()).map_err(|e| format!("sign-in: {e}"))?;
            let value = match auth {
                WebhookAuth::Bearer { .. } => format!("Bearer {secret}"),
                WebhookAuth::Basic { username, .. } => format!("Basic {}", base64(format!("{username}:{secret}").as_bytes())),
            };
            secrets.push(secret);
            out.push(("Authorization".into(), value));
        }
        Ok((out, secrets))
    }

    /// The runbook's notification rules, read once when a run starts. Runs started by
    /// a call step don't notify; the run that called them does.
    pub fn rules_for(&self, run: &Run) -> Vec<Notification> {
        if run.parent.is_some() {
            return vec![];
        }
        match self.store.runbook(&run.runbook_id) {
            Ok(Some(rb)) => rb.notifications.into_iter().filter(|n| n.enabled).collect(),
            _ => vec![],
        }
    }

    /// Send `event` for `run` to every rule that wants it, in the background.
    pub fn emit(self: &Arc<Self>, rules: &[Notification], run: &Run, event: NotifyEvent) {
        let wanted: Vec<Notification> = rules.iter().filter(|n| n.on.contains(&event)).cloned().collect();
        if wanted.is_empty() {
            return;
        }
        let (me, run) = (self.clone(), run.clone());
        tokio::task::spawn_blocking(move || {
            let msg = me.compose(&run, event, None);
            for n in &wanted {
                let result = me.deliver(n, &msg);
                me.record(&run.id, n, event, result);
            }
        });
    }

    /// Start a timer for each rule that wants to hear about slow runs.
    pub fn slow_timers(self: &Arc<Self>, rules: &[Notification], run: &Run) -> SlowTimers {
        let timers = rules
            .iter()
            .filter(|n| n.on.contains(&NotifyEvent::Slow))
            .filter_map(|n| n.slow_after_seconds.map(|s| (n.clone(), s)))
            .map(|(n, secs)| {
                let (me, run_id) = (self.clone(), run.id.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    let _ = tokio::task::spawn_blocking(move || {
                        // Only if it's really still running.
                        let Ok(Some(run)) = me.store.run(&run_id) else { return };
                        if run.status != Status::Running {
                            return;
                        }
                        let msg = me.compose(&run, NotifyEvent::Slow, Some(secs));
                        let result = me.deliver(&n, &msg);
                        me.record(&run.id, &n, NotifyEvent::Slow, result);
                    })
                    .await;
                })
            })
            .collect();
        SlowTimers(timers)
    }

    fn record(&self, run_id: &str, n: &Notification, event: NotifyEvent, result: Result<(), String>) {
        let target = n.target_label();
        match &result {
            Ok(()) => tracing::info!(run_id, event = event.as_str(), %target, "notification sent"),
            Err(e) => tracing::warn!(run_id, event = event.as_str(), %target, error = %e, "notification failed"),
        }
        let notice = Notice { at: now_ms(), event, target, ok: result.is_ok(), error: result.err() };
        if let Err(e) = self.store.add_notice(run_id, &notice) {
            tracing::error!(run_id, error = %e, "saving notification result failed");
        }
    }

    /// A test message to check a target before saving the runbook.
    pub fn send_test(&self, n: &Notification) -> Result<(), String> {
        let text = "This is a test notification from OpSkills. If you can read this, notifications for this runbook will reach you.".to_string();
        let msg = Message {
            event: "test",
            subject: "[OpSkills] Test notification".into(),
            body: format!("{text}\n{}", self.public_url.as_deref().map(|u| format!("\n{u}")).unwrap_or_default()),
            json: json!({ "event": "test", "text": text, "content": text }),
        };
        self.deliver(n, &msg)
    }

    pub fn deliver(&self, n: &Notification, msg: &Message) -> Result<(), String> {
        match &n.webhook {
            Some(url) => {
                let (headers, secrets) = self.webhook_headers(n)?;
                // Keep secret values out of anything recorded.
                post_webhook(url, msg, &headers).map_err(|e| secrets.iter().filter(|s| !s.is_empty()).fold(e, |e, s| e.replace(s.as_str(), "***")))
            }
            None => self.send_email(&n.email, msg),
        }
    }

    fn send_email(&self, to: &[String], msg: &Message) -> Result<(), String> {
        use lettre::message::{Mailbox, header::ContentType};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{SmtpTransport, Transport};
        let e = self.smtp()?;
        let host = e.host.as_str();
        let from: Mailbox = e.from.parse().map_err(|err| format!("sender {:?}: {err}", e.from))?;
        let builder = match e.security.as_str() {
            "tls" => SmtpTransport::relay(host),
            "starttls" => SmtpTransport::starttls_relay(host),
            _ => Ok(SmtpTransport::builder_dangerous(host)),
        }
        .map_err(|err| format!("SMTP: {err}"))?;
        let mut builder = builder.port(e.port).timeout(Some(Duration::from_secs(20)));
        if let Some((user, pass)) = e.credentials.clone() {
            builder = builder.credentials(Credentials::new(user, pass));
        }
        let mut mail = lettre::Message::builder().from(from).subject(msg.subject.as_str());
        for addr in to {
            mail = mail.to(addr.parse::<Mailbox>().map_err(|err| format!("{addr}: {err}"))?);
        }
        let mail = mail.header(ContentType::TEXT_PLAIN).body(msg.body.clone()).map_err(|err| format!("building the email: {err}"))?;
        builder.build().send(&mail).map(|_| ()).map_err(|err| format!("SMTP {host}:{}: {err}", e.port))
    }

    /// The message for one event of a run.
    pub fn compose(&self, run: &Run, event: NotifyEvent, slow_after: Option<u64>) -> Message {
        let n = run_number(&run.id);
        let name = &run.runbook_name;
        let url = self.public_url.as_ref().map(|u| format!("{u}/#/runs/{}", run.id));
        let took = match (run.started_at, run.finished_at) {
            (Some(s), Some(f)) if f.saturating_sub(s) < 1000 => Some("less than a second".to_string()),
            (Some(s), Some(f)) => Some(human_duration(f.saturating_sub(s) / 1000)),
            _ => None,
        };
        let failed_step = run.steps.iter().find(|s| s.status == Status::Failed);
        let place = if run.hosts.is_empty() {
            "on the OpSkills server".to_string()
        } else if run.hosts.len() == 1 {
            format!("on {}", run.hosts[0].name)
        } else {
            format!("on {} hosts", run.hosts.len())
        };
        let mut text = match event {
            NotifyEvent::Started => format!("{name} started: run #{n}, started by {}, {place}.", run.started_by),
            NotifyEvent::Succeeded => format!("{name} succeeded: run #{n} took {}.", took.as_deref().unwrap_or("a moment")),
            NotifyEvent::Failed => match failed_step {
                Some(s) => format!("{name} failed: run #{n}, step \"{}\" on {}, after {}.", s.name, s.host_name, took.as_deref().unwrap_or("a moment")),
                None => format!("{name} failed: run #{n}{}.", run.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()),
            },
            NotifyEvent::Slow => format!(
                "{name} is still running after {}: run #{n}, started by {}.",
                human_duration(slow_after.unwrap_or(0)),
                run.started_by
            ),
        };
        if let Some(u) = &url {
            text.push(' ');
            text.push_str(u);
        }
        let tail: Vec<String> = if event == NotifyEvent::Failed {
            self.store
                .logs(&run.id, run.log_lines.saturating_sub(TAIL_LINES as u64), TAIL_LINES)
                .unwrap_or_default()
                .into_iter()
                // Name the host only when there are several.
                .map(|l| if run.hosts.len() > 1 { format!("[{}] {}", l.host, l.line) } else { l.line })
                .collect()
        } else {
            vec![]
        };
        let status_word = match event {
            NotifyEvent::Slow => "still running",
            e => e.as_str(),
        };
        let subject = format!("[OpSkills] {name} {status_word} (run #{n})");
        let mut body = format!("{text}\n\nRunbook:    {name}\nRun:        #{n}\nStarted by: {}\nTrigger:    {}\n", run.started_by, run.trigger);
        if let Some(t) = &took {
            body.push_str(&format!("Took:       {t}\n"));
        }
        if !run.inputs.is_empty() {
            body.push_str("\nInputs:\n");
            for (k, v) in &run.inputs {
                body.push_str(&format!("  {k} = {v}\n"));
            }
        }
        if !tail.is_empty() {
            body.push_str("\nLast output:\n");
            for l in &tail {
                body.push_str(&format!("  {l}\n"));
            }
        }
        let json = json!({
            "event": event.as_str(),
            "text": text,
            "content": text,
            "runbook": { "id": run.runbook_id, "name": name },
            "run": {
                "id": run.id,
                "number": n,
                "status": run.status,
                "trigger": run.trigger,
                "started_by": run.started_by,
                "started_at": run.started_at,
                "finished_at": run.finished_at,
                "url": url,
                "failed_step": failed_step.map(|s| json!({ "name": s.name, "host": s.host_name, "exit_code": s.exit_code })),
                "error": run.error,
            },
            "output_tail": tail,
        });
        Message { event: event.as_str(), subject, body, json }
    }
}

/// Standard base64 with padding, for basic sign-in.
fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// POST the JSON body, retrying when the receiver is down or busy.
fn post_webhook(url: &str, msg: &Message, headers: &[(String, String)]) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(format!("OpSkills/{}", crate::VERSION))
        .build()
        .into();
    let body = msg.json.to_string();
    let mut last = String::new();
    for attempt in 0..=RETRY_DELAYS.len() {
        if attempt > 0 {
            std::thread::sleep(RETRY_DELAYS[attempt - 1]);
        }
        let mut req = agent.post(url).header("Content-Type", "application/json").header("X-OpSkills-Event", msg.event);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        match req.send(body.as_str()) {
            Ok(res) if res.status().is_success() => return Ok(()),
            Ok(res) => {
                let code = res.status();
                last = format!("the webhook answered {code}");
                if !(code.is_server_error() || code.as_u16() == 429) {
                    break;
                }
            }
            // The URL itself can be the secret (as with Slack), so keep it out of errors.
            Err(e) => last = format!("calling the webhook failed: {}", e.to_string().replace(url, "<url>")),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard() {
        for (input, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("ops:s3cret!", "b3BzOnMzY3JldCE=")] {
            assert_eq!(super::base64(input.as_bytes()), want, "{input}");
        }
    }
}
