//! Persistent storage in a single redb file inside the data directory.
//!
//! Values are JSON documents. Run ids are UUID v7, so key order is creation order.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::model::{ApiToken, EnrollmentToken, Host, LogLine, Run, Runbook, Secret, Status, StoredApiToken, StoredHost, StoredSecret, StoredWebhook, Webhook, now_ms};

const RUNBOOKS: TableDefinition<&str, &str> = TableDefinition::new("runbooks");
const RUNS: TableDefinition<&str, &str> = TableDefinition::new("runs");
/// Key: "<run id>/<12-digit sequence number>".
const LOGS: TableDefinition<&str, &str> = TableDefinition::new("logs");
/// Key: secret name. Value: metadata plus the AES-256-GCM ciphertext, never plaintext.
const SECRETS: TableDefinition<&str, &str> = TableDefinition::new("secrets");
/// Key: host id. Value: host plus the hash of its agent secret.
const HOSTS: TableDefinition<&str, &str> = TableDefinition::new("hosts");
/// Key: SHA-256 of the enrollment token, hex.
const ENROLLMENT: TableDefinition<&str, &str> = TableDefinition::new("enrollment_tokens");
/// Key: SHA-256 of the session token, hex.
const SESSIONS: TableDefinition<&str, &str> = TableDefinition::new("sessions");
/// Key: API token id. Value: the token plus the SHA-256 of its secret.
const API_TOKENS: TableDefinition<&str, &str> = TableDefinition::new("api_tokens");
/// Key: webhook id. Value: the webhook, its token hash and sealed signing secret.
const WEBHOOKS: TableDefinition<&str, &str> = TableDefinition::new("webhooks");
/// Key: "<runbook id>/<schedule id>". Value: when it last fired, and how that went.
const SCHEDULE_STATE: TableDefinition<&str, &str> = TableDefinition::new("schedule_state");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub user: String,
    pub created_at: u64,
    pub expires_at: u64,
}

pub struct Store {
    db: Database,
}

fn log_key(run_id: &str, seq: u64) -> String {
    format!("{run_id}/{seq:012}")
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).with_context(|| format!("opening {}", path.display()))?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        let w = db.begin_write()?;
        {
            w.open_table(RUNBOOKS)?;
            w.open_table(RUNS)?;
            w.open_table(LOGS)?;
            w.open_table(SESSIONS)?;
            w.open_table(SECRETS)?;
            w.open_table(HOSTS)?;
            w.open_table(ENROLLMENT)?;
            w.open_table(API_TOKENS)?;
            w.open_table(SCHEDULE_STATE)?;
            w.open_table(WEBHOOKS)?;
        }
        w.commit()?;
        Ok(Store { db })
    }

    fn put(&self, def: TableDefinition<&'static str, &'static str>, key: &str, value: &str) -> Result<()> {
        let w = self.db.begin_write()?;
        w.open_table(def)?.insert(key, value)?;
        w.commit()?;
        Ok(())
    }

    fn get(&self, def: TableDefinition<&'static str, &'static str>, key: &str) -> Result<Option<String>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(def)?;
        Ok(t.get(key)?.map(|v| v.value().to_string()))
    }

    fn remove(&self, def: TableDefinition<&'static str, &'static str>, key: &str) -> Result<bool> {
        let w = self.db.begin_write()?;
        let existed = w.open_table(def)?.remove(key)?.is_some();
        w.commit()?;
        Ok(existed)
    }

    fn values(&self, def: TableDefinition<&'static str, &'static str>) -> Result<Vec<String>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(def)?;
        let mut out = Vec::new();
        for item in t.iter()? {
            let (_, v) = item?;
            out.push(v.value().to_string());
        }
        Ok(out)
    }

    // Runbooks

    pub fn put_runbook(&self, rb: &Runbook) -> Result<()> {
        self.put(RUNBOOKS, &rb.id, &serde_json::to_string(rb)?)
    }

    pub fn runbook(&self, id: &str) -> Result<Option<Runbook>> {
        Ok(match self.get(RUNBOOKS, id)? {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    pub fn runbooks(&self) -> Result<Vec<Runbook>> {
        let mut list: Vec<Runbook> = self.values(RUNBOOKS)?.iter().filter_map(|s| serde_json::from_str(s).ok()).collect();
        list.sort_by_key(|a| a.name.to_lowercase());
        Ok(list)
    }

    pub fn delete_runbook(&self, id: &str) -> Result<bool> {
        self.remove(RUNBOOKS, id)
    }

    // Runs

    pub fn put_run(&self, run: &Run) -> Result<()> {
        self.put(RUNS, &run.id, &serde_json::to_string(run)?)
    }

    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        Ok(match self.get(RUNS, id)? {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    /// Newest first, optionally for one runbook.
    pub fn runs(&self, runbook_id: Option<&str>, limit: usize) -> Result<Vec<Run>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(RUNS)?;
        let mut out = Vec::new();
        for item in t.iter()?.rev() {
            let (_, v) = item?;
            if let Ok(run) = serde_json::from_str::<Run>(v.value())
                && runbook_id.is_none_or(|id| run.runbook_id == id) {
                    out.push(run);
                    if out.len() >= limit {
                        break;
                    }
                }
        }
        Ok(out)
    }

    /// Runs that were queued or running when the server stopped can never finish.
    pub fn fail_interrupted_runs(&self) -> Result<usize> {
        let mut fixed = 0;
        for s in self.values(RUNS)? {
            let Ok(mut run) = serde_json::from_str::<Run>(&s) else { continue };
            if run.status.is_finished() {
                continue;
            }
            let now = now_ms();
            for step in &mut run.steps {
                if !step.status.is_finished() {
                    step.status = if step.status == Status::Running { Status::Failed } else { Status::Skipped };
                    step.finished_at = step.started_at.map(|_| now);
                }
            }
            run.status = Status::Failed;
            run.finished_at = Some(now);
            run.error = Some("The server stopped while this run was in progress.".into());
            self.put_run(&run)?;
            fixed += 1;
        }
        Ok(fixed)
    }

    // Logs

    pub fn append_logs(&self, run_id: &str, lines: &[LogLine]) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(LOGS)?;
            for l in lines {
                t.insert(log_key(run_id, l.seq).as_str(), serde_json::to_string(l)?.as_str())?;
            }
        }
        w.commit()?;
        Ok(())
    }

    /// Lines with a sequence number greater than `after`, up to `limit`.
    pub fn logs(&self, run_id: &str, after: u64, limit: usize) -> Result<Vec<LogLine>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(LOGS)?;
        let start = log_key(run_id, after.saturating_add(1));
        let end = format!("{run_id}/~");
        let mut out = Vec::new();
        for item in t.range(start.as_str()..end.as_str())? {
            let (_, v) = item?;
            if let Ok(line) = serde_json::from_str::<LogLine>(v.value()) {
                out.push(line);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    // Secrets

    pub fn put_secret(&self, s: &StoredSecret) -> Result<()> {
        self.put(SECRETS, &s.meta.name, &serde_json::to_string(s)?)
    }

    pub fn secret(&self, name: &str) -> Result<Option<StoredSecret>> {
        Ok(match self.get(SECRETS, name)? {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    /// Metadata of every secret, by name.
    pub fn secrets(&self) -> Result<Vec<Secret>> {
        Ok(self.values(SECRETS)?.iter().filter_map(|s| serde_json::from_str::<StoredSecret>(s).ok()).map(|s| s.meta).collect())
    }

    pub fn delete_secret(&self, name: &str) -> Result<bool> {
        self.remove(SECRETS, name)
    }

    // Hosts

    pub fn put_host(&self, h: &StoredHost) -> Result<()> {
        self.put(HOSTS, &h.host.id, &serde_json::to_string(h)?)
    }

    pub fn stored_host(&self, id: &str) -> Result<Option<StoredHost>> {
        Ok(match self.get(HOSTS, id)? {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    /// Every host, by name.
    pub fn hosts(&self) -> Result<Vec<Host>> {
        let mut list: Vec<Host> = self.values(HOSTS)?.iter().filter_map(|s| serde_json::from_str::<StoredHost>(s).ok()).map(|h| h.host).collect();
        list.sort_by_key(|a| a.name.to_lowercase());
        Ok(list)
    }

    pub fn delete_host(&self, id: &str) -> Result<bool> {
        self.remove(HOSTS, id)
    }

    pub fn put_enrollment_token(&self, token_hash: &str, t: &EnrollmentToken) -> Result<()> {
        self.put(ENROLLMENT, token_hash, &serde_json::to_string(t)?)
    }

    /// Use up an enrollment token: true if it existed and had not expired. Either way it's gone.
    pub fn take_enrollment_token(&self, token_hash: &str) -> Result<bool> {
        let w = self.db.begin_write()?;
        let valid = {
            let mut t = w.open_table(ENROLLMENT)?;
            let removed = t.remove(token_hash)?.map(|v| v.value().to_string());
            match removed {
                Some(s) => serde_json::from_str::<EnrollmentToken>(&s).map(|e| e.expires_at > now_ms()).unwrap_or(false),
                None => false,
            }
        };
        w.commit()?;
        Ok(valid)
    }

    /// Drop expired enrollment tokens.
    pub fn purge_expired_enrollment_tokens(&self) -> Result<()> {
        let now = now_ms();
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(ENROLLMENT)?;
            t.retain(|_, v| serde_json::from_str::<EnrollmentToken>(v).map(|e| e.expires_at > now).unwrap_or(false))?;
        }
        w.commit()?;
        Ok(())
    }

    // Sessions

    // Webhooks

    pub fn put_webhook(&self, h: &StoredWebhook) -> Result<()> {
        self.put(WEBHOOKS, &h.hook.id, &serde_json::to_string(h)?)
    }
    pub fn webhook(&self, id: &str) -> Result<Option<StoredWebhook>> {
        self.get(WEBHOOKS, id)?.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    /// The webhooks of one runbook, oldest first.
    pub fn webhooks_of(&self, runbook_id: &str) -> Result<Vec<Webhook>> {
        let mut out: Vec<Webhook> = self
            .values(WEBHOOKS)?
            .iter()
            .map(|s| serde_json::from_str::<StoredWebhook>(s).map(|h| h.hook))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|h| h.runbook_id == runbook_id)
            .collect();
        out.sort_by_key(|h| h.created_at);
        Ok(out)
    }
    /// Change a webhook in one write transaction, so a delivery being recorded can't
    /// undo an edit or a new URL made at the same moment (or the other way round).
    pub fn update_webhook<T>(&self, id: &str, f: impl FnOnce(&mut StoredWebhook) -> T) -> Result<Option<T>> {
        let w = self.db.begin_write()?;
        let out = {
            let mut t = w.open_table(WEBHOOKS)?;
            let Some(cur) = t.get(id)?.map(|v| v.value().to_string()) else { return Ok(None) };
            let mut h: StoredWebhook = serde_json::from_str(&cur)?;
            let out = f(&mut h);
            t.insert(id, serde_json::to_string(&h)?.as_str())?;
            out
        };
        w.commit()?;
        Ok(Some(out))
    }
    pub fn delete_webhook(&self, id: &str) -> Result<bool> {
        self.remove(WEBHOOKS, id)
    }

    // Schedules

    pub fn schedule_state(&self, key: &str) -> Result<Option<crate::schedule::ScheduleState>> {
        self.get(SCHEDULE_STATE, key)?.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    pub fn put_schedule_state(&self, key: &str, s: &crate::schedule::ScheduleState) -> Result<()> {
        self.put(SCHEDULE_STATE, key, &serde_json::to_string(s)?)
    }

    // API tokens

    pub fn put_api_token(&self, t: &StoredApiToken) -> Result<()> {
        self.put(API_TOKENS, &t.token.id, &serde_json::to_string(t)?)
    }
    pub fn api_token(&self, id: &str) -> Result<Option<StoredApiToken>> {
        self.get(API_TOKENS, id)?.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    /// Every token, newest first.
    pub fn api_tokens(&self) -> Result<Vec<ApiToken>> {
        let mut out: Vec<ApiToken> = self.values(API_TOKENS)?.iter().map(|s| serde_json::from_str::<StoredApiToken>(s).map(|t| t.token)).collect::<Result<_, _>>()?;
        out.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        Ok(out)
    }
    pub fn delete_api_token(&self, id: &str) -> Result<bool> {
        self.remove(API_TOKENS, id)
    }

    pub fn put_session(&self, token_hash: &str, s: &Session) -> Result<()> {
        self.put(SESSIONS, token_hash, &serde_json::to_string(s)?)
    }

    pub fn session(&self, token_hash: &str) -> Result<Option<Session>> {
        let Some(s) = self.get(SESSIONS, token_hash)? else { return Ok(None) };
        let session: Session = serde_json::from_str(&s)?;
        if session.expires_at <= now_ms() {
            self.remove(SESSIONS, token_hash)?;
            return Ok(None);
        }
        Ok(Some(session))
    }

    pub fn delete_session(&self, token_hash: &str) -> Result<()> {
        self.remove(SESSIONS, token_hash).map(|_| ())
    }

    pub fn clear_sessions(&self) -> Result<()> {
        let w = self.db.begin_write()?;
        w.open_table(SESSIONS)?.retain(|_, _| false)?;
        w.commit()?;
        Ok(())
    }

    pub fn purge_expired_sessions(&self) -> Result<()> {
        let now = now_ms();
        let w = self.db.begin_write()?;
        w.open_table(SESSIONS)?
            .retain(|_, v| serde_json::from_str::<Session>(v).map(|s| s.expires_at > now).unwrap_or(false))?;
        w.commit()?;
        Ok(())
    }
}
