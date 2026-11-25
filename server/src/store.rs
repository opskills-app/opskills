//! Persistent storage in a single redb file inside the data directory.
//!
//! Values are JSON documents. Run ids are UUID v7, so key order is creation order.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::model::{LogLine, Run, Runbook, Secret, Status, StoredSecret, now_ms};

const RUNBOOKS: TableDefinition<&str, &str> = TableDefinition::new("runbooks");
const RUNS: TableDefinition<&str, &str> = TableDefinition::new("runs");
/// Key: "<run id>/<12-digit sequence number>".
const LOGS: TableDefinition<&str, &str> = TableDefinition::new("logs");
/// Key: secret name. Value: metadata plus the AES-256-GCM ciphertext, never plaintext.
const SECRETS: TableDefinition<&str, &str> = TableDefinition::new("secrets");
/// Key: SHA-256 of the session token, hex.
const SESSIONS: TableDefinition<&str, &str> = TableDefinition::new("sessions");

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
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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
            if let Ok(run) = serde_json::from_str::<Run>(v.value()) {
                if runbook_id.is_none_or(|id| run.runbook_id == id) {
                    out.push(run);
                    if out.len() >= limit {
                        break;
                    }
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

    // Sessions

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
