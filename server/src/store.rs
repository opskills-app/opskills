//! Persistent storage in a single redb file inside the data directory.
//!
//! Values are JSON documents. Run ids are UUID v7, so key order is creation order.
//!
//! The `meta` table records the schema version. Opening an older database migrates it,
//! after copying the file to a backup next to it; a newer one is refused.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::model::{ApiToken, AuditEvent, DEFAULT_WORKSPACE, EnrollmentToken, FIRST_ADMIN, Host, LogLine, Role, Run, Runbook, Secret, Status, StoredApiToken, StoredHost, StoredSecret, StoredUser, StoredWebhook, User, Webhook, Workspace, now_ms};

const RUNBOOKS: TableDefinition<&str, &str> = TableDefinition::new("runbooks");
const RUNS: TableDefinition<&str, &str> = TableDefinition::new("runs");
/// Key: "<run id>/<12-digit sequence number>".
const LOGS: TableDefinition<&str, &str> = TableDefinition::new("logs");
/// Key: "<workspace>/<secret name>". Value: metadata plus the AES-256-GCM ciphertext,
/// never plaintext.
const SECRETS: TableDefinition<&str, &str> = TableDefinition::new("secrets");
/// Key: host id. Value: host plus the hash of its agent secret.
const HOSTS: TableDefinition<&str, &str> = TableDefinition::new("hosts");
/// Key: SHA-256 of the enrollment token, hex.
const ENROLLMENT: TableDefinition<&str, &str> = TableDefinition::new("enrollment_tokens");
/// Key: SHA-256 of the session token, hex.
const SESSIONS: TableDefinition<&str, &str> = TableDefinition::new("sessions");
/// Key: API token id. Value: the token plus the SHA-256 of its secret.
const API_TOKENS: TableDefinition<&str, &str> = TableDefinition::new("api_tokens");
/// Key: a setting's name, like "email". Value: its JSON.
const SETTINGS: TableDefinition<&str, &str> = TableDefinition::new("settings");
/// Key: run id. Value: the notifications sent about the run and how each went.
const NOTICES: TableDefinition<&str, &str> = TableDefinition::new("notices");
/// Key: webhook id. Value: the webhook, its token hash and sealed signing secret.
const WEBHOOKS: TableDefinition<&str, &str> = TableDefinition::new("webhooks");
/// Key: "<runbook id>/<schedule id>". Value: when it last fired, and how that went.
const SCHEDULE_STATE: TableDefinition<&str, &str> = TableDefinition::new("schedule_state");
/// Key: user name. Value: the user and their password hash.
const USERS: TableDefinition<&str, &str> = TableDefinition::new("users");
/// Key: event id (UUID v7, time-ordered). Value: the audit event.
const AUDIT: TableDefinition<&str, &str> = TableDefinition::new("audit");
/// Key: workspace slug. Value: the workspace.
const WORKSPACES: TableDefinition<&str, &str> = TableDefinition::new("workspaces");
/// Key: "schema_version". Value: the version number.
const META: TableDefinition<&str, &str> = TableDefinition::new("meta");

/// The schema this build writes. Each step in `MIGRATIONS` moves the database up by one.
pub const SCHEMA_VERSION: u64 = 2;

type Migration = fn(&redb::WriteTransaction, &Path) -> Result<()>;

/// `(version it produces, what it does, the change)`, in order.
const MIGRATIONS: &[(u64, &str, Migration)] = &[(1, "users and roles", migrate_users), (2, "workspaces", migrate_workspaces)];

/// Version 1: users. The single admin of earlier versions becomes the user `admin`, with
/// the password in `admin.hash`.
fn migrate_users(w: &redb::WriteTransaction, data_dir: &Path) -> Result<()> {
    let mut users = w.open_table(USERS)?;
    if users.iter()?.next().is_some() {
        return Ok(());
    }
    let Ok(hash) = std::fs::read_to_string(data_dir.join("admin.hash")) else { return Ok(()) };
    // The shape version 1 wrote; version 2 turns `role` into workspace roles.
    let admin = serde_json::json!({
        "name": FIRST_ADMIN, "role": "admin", "created_at": now_ms(), "must_change_password": false,
        "last_sign_in_at": null, "password_changed_at": null, "password_hash": hash.trim(),
    });
    users.insert(FIRST_ADMIN, admin.to_string().as_str())?;
    Ok(())
}

/// Version 2: workspaces. Everything so far belongs to the workspace `default`: runbooks,
/// runs, hosts and enrollment tokens read their missing workspace as `default`; vault
/// secrets move to keys `default/<name>`. Admins become server admins, and every user
/// keeps their role in `default`.
fn migrate_workspaces(w: &redb::WriteTransaction, _data_dir: &Path) -> Result<()> {
    let mut spaces = w.open_table(WORKSPACES)?;
    if spaces.get(DEFAULT_WORKSPACE)?.is_none() {
        let default = Workspace { slug: DEFAULT_WORKSPACE.into(), name: "Default".into(), description: String::new(), created_at: now_ms() };
        spaces.insert(DEFAULT_WORKSPACE, serde_json::to_string(&default)?.as_str())?;
    }
    let mut users = w.open_table(USERS)?;
    let old: Vec<(String, String)> = users.iter()?.map(|r| r.map(|(k, v)| (k.value().to_string(), v.value().to_string()))).collect::<Result<_, _>>()?;
    for (name, json) in old {
        let mut v: serde_json::Value = serde_json::from_str(&json)?;
        let Some(role) = v.as_object_mut().and_then(|o| o.remove("role")) else { continue };
        v["admin"] = serde_json::Value::Bool(role == "admin");
        v["workspaces"] = serde_json::json!({ DEFAULT_WORKSPACE: role });
        users.insert(name.as_str(), v.to_string().as_str())?;
    }
    let mut secrets = w.open_table(SECRETS)?;
    let old: Vec<(String, String)> = secrets.iter()?.map(|r| r.map(|(k, v)| (k.value().to_string(), v.value().to_string()))).collect::<Result<_, _>>()?;
    for (name, json) in old {
        if name.contains('/') {
            continue;
        }
        let mut v: serde_json::Value = serde_json::from_str(&json)?;
        v["workspace"] = serde_json::Value::String(DEFAULT_WORKSPACE.into());
        secrets.remove(name.as_str())?;
        secrets.insert(secret_key(DEFAULT_WORKSPACE, &name).as_str(), v.to_string().as_str())?;
    }
    Ok(())
}

fn secret_key(workspace: &str, name: &str) -> String {
    format!("{workspace}/{name}")
}

fn schema_version(db: &Database) -> Result<u64> {
    let r = db.begin_read()?;
    let t = match r.open_table(META) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    Ok(match t.get("schema_version")? {
        Some(v) => v.value().parse().context("reading the schema version")?,
        None => 0,
    })
}

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
    /// Open (or create) the database, migrating an older one. `data_dir` holds the files a
    /// migration may read, like `admin.hash`.
    pub fn open(path: &Path, data_dir: &Path) -> Result<Self> {
        let existed = path.exists();
        let mut db = Database::create(path).with_context(|| format!("opening {}", path.display()))?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        let version = schema_version(&db)?;
        anyhow::ensure!(
            version <= SCHEMA_VERSION,
            "the database was written by a newer OpSkills (schema {version}, this version knows up to {SCHEMA_VERSION}). Install that version or newer, or restore a backup."
        );
        if version < SCHEMA_VERSION && existed {
            // Close it, copy it, open it again: the copy is the state before any change.
            drop(db);
            let backup = path.with_extension(format!("redb.schema-{version}-backup-{}", now_ms() / 1000));
            std::fs::copy(path, &backup).with_context(|| format!("backing up the database to {}", backup.display()))?;
            let _ = std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600));
            tracing::info!(from = version, to = SCHEMA_VERSION, backup = %backup.display(), "migrating the database; the backup is the state before");
            db = Database::create(path).with_context(|| format!("opening {}", path.display()))?;
        }
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
            w.open_table(NOTICES)?;
            w.open_table(SETTINGS)?;
            w.open_table(USERS)?;
            w.open_table(AUDIT)?;
            w.open_table(WORKSPACES)?;
        }
        // Every migration and the new version commit together, or not at all.
        for (to, what, migrate) in MIGRATIONS {
            if *to > version {
                migrate(&w, data_dir).with_context(|| format!("migrating the database to schema {to} ({what})"))?;
            }
        }
        w.open_table(META)?.insert("schema_version", SCHEMA_VERSION.to_string().as_str())?;
        w.commit()?;
        Ok(Store { db })
    }

    pub fn schema_version(&self) -> Result<u64> {
        schema_version(&self.db)
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

    /// Newest first, optionally only in one workspace and for one runbook.
    pub fn runs(&self, workspace: Option<&str>, runbook_id: Option<&str>, limit: usize) -> Result<Vec<Run>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(RUNS)?;
        let mut out = Vec::new();
        for item in t.iter()?.rev() {
            let (_, v) = item?;
            if let Ok(run) = serde_json::from_str::<Run>(v.value())
                && runbook_id.is_none_or(|id| run.runbook_id == id)
                && workspace.is_none_or(|w| run.workspace == w) {
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
        self.put(SECRETS, &secret_key(&s.meta.workspace, &s.meta.name), &serde_json::to_string(s)?)
    }

    pub fn secret(&self, workspace: &str, name: &str) -> Result<Option<StoredSecret>> {
        Ok(match self.get(SECRETS, &secret_key(workspace, name))? {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    /// Metadata of every secret in a workspace, by name.
    pub fn secrets(&self, workspace: &str) -> Result<Vec<Secret>> {
        let mut list: Vec<Secret> = self.values(SECRETS)?.iter().filter_map(|s| serde_json::from_str::<StoredSecret>(s).ok()).map(|s| s.meta).filter(|m| m.workspace == workspace).collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    pub fn delete_secret(&self, workspace: &str, name: &str) -> Result<bool> {
        self.remove(SECRETS, &secret_key(workspace, name))
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

    /// Use up an enrollment token: the token if it existed and had not expired. Either way
    /// it's gone.
    pub fn take_enrollment_token(&self, token_hash: &str) -> Result<Option<EnrollmentToken>> {
        let w = self.db.begin_write()?;
        let valid = {
            let mut t = w.open_table(ENROLLMENT)?;
            let removed = t.remove(token_hash)?.map(|v| v.value().to_string());
            removed.and_then(|s| serde_json::from_str::<EnrollmentToken>(&s).ok()).filter(|e| e.expires_at > now_ms())
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

    // Settings

    pub fn email_settings(&self) -> Result<Option<crate::model::EmailSettings>> {
        Ok(self.get(SETTINGS, "email")?.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    pub fn put_email_settings(&self, s: Option<&crate::model::EmailSettings>) -> Result<()> {
        match s {
            Some(s) => self.put(SETTINGS, "email", &serde_json::to_string(s)?),
            None => self.remove(SETTINGS, "email").map(|_| ()),
        }
    }

    // Notifications

    /// Add a notification result to a run's list, in one transaction.
    pub fn add_notice(&self, run_id: &str, n: &crate::notify::Notice) -> Result<()> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(NOTICES)?;
            let cur = t.get(run_id)?.map(|v| v.value().to_string());
            let mut list: Vec<crate::notify::Notice> = cur.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
            list.push(n.clone());
            t.insert(run_id, serde_json::to_string(&list)?.as_str())?;
        }
        w.commit()?;
        Ok(())
    }
    pub fn notices(&self, run_id: &str) -> Result<Vec<crate::notify::Notice>> {
        Ok(self.get(NOTICES, run_id)?.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default())
    }

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

    // Users

    pub fn user(&self, name: &str) -> Result<Option<StoredUser>> {
        self.get(USERS, name)?.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    pub fn put_user(&self, u: &StoredUser) -> Result<()> {
        self.put(USERS, &u.user.name, &serde_json::to_string(u)?)
    }
    /// Every user, by name.
    pub fn users(&self) -> Result<Vec<User>> {
        self.values(USERS)?.iter().map(|s| serde_json::from_str::<StoredUser>(s).map(|u| u.user).map_err(Into::into)).collect()
    }
    /// Remove a user with their sessions and API tokens.
    pub fn delete_user(&self, name: &str) -> Result<bool> {
        let w = self.db.begin_write()?;
        let existed = w.open_table(USERS)?.remove(name)?.is_some();
        w.open_table(SESSIONS)?.retain(|_, v| serde_json::from_str::<Session>(v).map(|s| s.user != name).unwrap_or(false))?;
        w.open_table(API_TOKENS)?.retain(|_, v| serde_json::from_str::<StoredApiToken>(v).map(|t| t.token.user != name).unwrap_or(false))?;
        w.commit()?;
        Ok(existed)
    }
    /// After a new password from `admin.hash`: the `admin` user exists, is a server admin, has that
    /// password and must choose a new one at sign-in. Everyone is signed out.
    pub fn reset_first_admin(&self, password_hash: &str) -> Result<()> {
        let mut u = self.user(FIRST_ADMIN)?.unwrap_or_else(|| StoredUser {
            user: User { name: FIRST_ADMIN.into(), admin: true, workspaces: Default::default(), created_at: now_ms(), must_change_password: true, last_sign_in_at: None, password_changed_at: None },
            password_hash: String::new(),
        });
        u.user.admin = true;
        u.user.workspaces.insert(DEFAULT_WORKSPACE.into(), Role::Admin);
        u.user.must_change_password = true;
        u.password_hash = password_hash.trim().to_string();
        self.put_user(&u)?;
        self.clear_sessions()
    }
    /// Sign a user out everywhere, except the session with this token hash.
    pub fn delete_sessions_of(&self, name: &str, except: Option<&str>) -> Result<()> {
        let w = self.db.begin_write()?;
        w.open_table(SESSIONS)?
            .retain(|k, v| except == Some(k) || serde_json::from_str::<Session>(v).map(|s| s.user != name).unwrap_or(false))?;
        w.commit()?;
        Ok(())
    }


    // Audit log

    pub fn append_audit(&self, e: &AuditEvent) -> Result<()> {
        self.put(AUDIT, &e.id, &serde_json::to_string(e)?)
    }
    /// Events before the one with id `before` (or the newest), newest first, that pass
    /// `keep`, up to `limit`. At most `scan` events are looked at, so a search can't run
    /// through a very long log in one request; `more` says whether there are older ones.
    pub fn audit(&self, before: Option<&str>, limit: usize, scan: usize, keep: impl Fn(&AuditEvent) -> bool) -> Result<(Vec<AuditEvent>, Option<String>)> {
        let r = self.db.begin_read()?;
        let t = r.open_table(AUDIT)?;
        let iter = match before {
            Some(b) => t.range(..b)?,
            None => t.iter()?,
        };
        let mut out = Vec::new();
        let mut last = None;
        for (seen, item) in iter.rev().enumerate() {
            let (k, v) = item?;
            if seen >= scan || out.len() >= limit {
                // There is more; continue after the last one looked at.
                return Ok((out, last));
            }
            last = Some(k.value().to_string());
            if let Ok(e) = serde_json::from_str::<AuditEvent>(v.value())
                && keep(&e)
            {
                out.push(e);
            }
        }
        Ok((out, None))
    }
    /// Drop events older than `cutoff_ms`. Returns how many.
    pub fn purge_audit_before(&self, cutoff_ms: u64) -> Result<usize> {
        let w = self.db.begin_write()?;
        let mut removed = 0;
        {
            let mut t = w.open_table(AUDIT)?;
            t.retain(|_, v| {
                let keep = serde_json::from_str::<AuditEvent>(v).map(|e| e.at >= cutoff_ms).unwrap_or(false);
                if !keep {
                    removed += 1;
                }
                keep
            })?;
        }
        w.commit()?;
        Ok(removed)
    }


    // Workspaces

    pub fn workspace(&self, slug: &str) -> Result<Option<Workspace>> {
        self.get(WORKSPACES, slug)?.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    pub fn put_workspace(&self, ws: &Workspace) -> Result<()> {
        self.put(WORKSPACES, &ws.slug, &serde_json::to_string(ws)?)
    }
    /// Every workspace, `default` first, then by name.
    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let mut list: Vec<Workspace> = self.values(WORKSPACES)?.iter().map(|s| serde_json::from_str(s)).collect::<Result<_, _>>()?;
        list.sort_by_key(|w| (w.slug != DEFAULT_WORKSPACE, w.name.to_lowercase()));
        Ok(list)
    }
    /// Remove a workspace in one transaction: its run history (runs, output, notices),
    /// everyone's membership, and its place on shared hosts. The caller checks first that
    /// it has no runbooks, secrets or hosts of its own. Returns the number of runs removed.
    pub fn delete_workspace(&self, slug: &str) -> Result<usize> {
        let w = self.db.begin_write()?;
        let mut removed = 0;
        {
            w.open_table(WORKSPACES)?.remove(slug)?;
            let mut runs = w.open_table(RUNS)?;
            let ids: Vec<String> = runs
                .iter()?
                .filter_map(|r| r.ok())
                .filter_map(|(k, v)| serde_json::from_str::<Run>(v.value()).ok().filter(|run| run.workspace == slug).map(|_| k.value().to_string()))
                .collect();
            let mut logs = w.open_table(LOGS)?;
            let mut notices = w.open_table(NOTICES)?;
            for id in &ids {
                runs.remove(id.as_str())?;
                notices.remove(id.as_str())?;
                let (start, end) = (format!("{id}/"), format!("{id}/~"));
                logs.retain_in(start.as_str()..end.as_str(), |_, _| false)?;
                removed += 1;
            }
            let mut users = w.open_table(USERS)?;
            let all: Vec<(String, String)> = users.iter()?.map(|r| r.map(|(k, v)| (k.value().to_string(), v.value().to_string()))).collect::<Result<_, _>>()?;
            for (name, json) in all {
                let mut u: StoredUser = serde_json::from_str(&json)?;
                if u.user.workspaces.remove(slug).is_some() {
                    users.insert(name.as_str(), serde_json::to_string(&u)?.as_str())?;
                }
            }
            let mut hosts = w.open_table(HOSTS)?;
            let all: Vec<(String, String)> = hosts.iter()?.map(|r| r.map(|(k, v)| (k.value().to_string(), v.value().to_string()))).collect::<Result<_, _>>()?;
            for (id, json) in all {
                let mut h: StoredHost = serde_json::from_str(&json)?;
                if h.host.workspaces.iter().any(|x| x == slug) {
                    h.host.workspaces.retain(|x| x != slug);
                    hosts.insert(id.as_str(), serde_json::to_string(&h)?.as_str())?;
                }
            }
        }
        w.commit()?;
        Ok(removed)
    }

}
