//! Notifications about runs: email through the configured SMTP server, or a JSON POST
//! to a webhook URL. Sending never holds up a run, and each result is recorded on the
//! run so people can see whether a message went out.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{Config, Email};
use crate::executor::{human_duration, run_number};
use crate::model::{Notification, NotifyEvent, Run, Status, now_ms};
use crate::store::Store;

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
    email: Email,
    public_url: Option<String>,
    store: Arc<Store>,
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
    pub fn new(cfg: &Config, store: Arc<Store>) -> Arc<Self> {
        let public_url = cfg.server.public_url.as_deref().map(|u| u.trim_end_matches('/').to_string());
        Arc::new(Notifier { email: cfg.email.clone(), public_url, store })
    }

    pub fn email_enabled(&self) -> bool {
        self.email.enabled()
    }

    pub fn email_from(&self) -> Option<String> {
        self.email_enabled().then(|| self.email.from.clone()).flatten()
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
            Some(url) => post_webhook(url, msg),
            None => self.send_email(&n.email, msg),
        }
    }

    fn send_email(&self, to: &[String], msg: &Message) -> Result<(), String> {
        use lettre::message::{Mailbox, header::ContentType};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{SmtpTransport, Transport};
        let e = &self.email;
        let host = e.smtp_host.as_deref().filter(|h| !h.is_empty()).ok_or("Email isn't set up on this server. Set [email] in the server config.")?;
        let from: Mailbox = e.from.as_deref().unwrap_or_default().parse().map_err(|err| format!("email.from: {err}"))?;
        let builder = match e.security.as_str() {
            "tls" => SmtpTransport::relay(host),
            "starttls" => SmtpTransport::starttls_relay(host),
            _ => Ok(SmtpTransport::builder_dangerous(host)),
        }
        .map_err(|err| format!("SMTP: {err}"))?;
        let mut builder = builder.port(e.smtp_port).timeout(Some(Duration::from_secs(20)));
        if let (Some(user), Some(pass)) = (&e.username, &e.password) {
            builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
        }
        let mut mail = lettre::Message::builder().from(from).subject(msg.subject.as_str());
        for addr in to {
            mail = mail.to(addr.parse::<Mailbox>().map_err(|err| format!("{addr}: {err}"))?);
        }
        let mail = mail.header(ContentType::TEXT_PLAIN).body(msg.body.clone()).map_err(|err| format!("building the email: {err}"))?;
        builder.build().send(&mail).map(|_| ()).map_err(|err| format!("SMTP {host}:{}: {err}", e.smtp_port))
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

/// POST the JSON body, retrying when the receiver is down or busy.
fn post_webhook(url: &str, msg: &Message) -> Result<(), String> {
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
        match agent.post(url).header("Content-Type", "application/json").header("X-OpSkills-Event", msg.event).send(body.as_str()) {
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
