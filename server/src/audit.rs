//! Recording the audit log, and removing events older than `audit.retention_days`.

use std::sync::Arc;
use std::time::Duration;

use crate::api::CurrentUser;
use crate::model::{AuditEvent, AuditTarget, now_ms};
use crate::web::AppState;

/// Who did something: a user (maybe through an API token), or the server itself on
/// behalf of a schedule, webhook or agent.
#[derive(Clone, Debug)]
pub struct By {
    pub actor: String,
    pub via: Option<String>,
}

pub const SYSTEM: &str = "system";

impl By {
    pub fn user(u: &CurrentUser) -> By {
        By { actor: u.user.clone(), via: u.token.as_ref().map(|t| format!("token {t}")) }
    }
    /// A user by name, signed in directly (sign-in events, before a session exists).
    pub fn named(name: &str) -> By {
        By { actor: name.into(), via: None }
    }
    pub fn system(via: impl Into<String>) -> By {
        By { actor: SYSTEM.into(), via: Some(via.into()) }
    }
    /// How runs name who started them: "alice", "alice (token ci)", "schedule nightly".
    pub fn label(&self) -> String {
        match (&self.via, self.actor == SYSTEM) {
            (Some(via), true) => via.clone(),
            (Some(via), false) => format!("{} ({via})", self.actor),
            (None, _) => self.actor.clone(),
        }
    }
}

/// Record one change. A failure to write is logged, not returned: the change itself
/// already happened.
pub fn record(st: &AppState, by: &By, action: &str, target: Option<AuditTarget>, detail: Option<String>) {
    let event = AuditEvent {
        id: uuid::Uuid::now_v7().to_string(),
        at: now_ms(),
        actor: by.actor.clone(),
        via: by.via.clone(),
        action: action.into(),
        target,
        detail,
    };
    if let Err(e) = st.store.append_audit(&event) {
        tracing::error!(error = %format!("{e:#}"), action, "writing the audit log failed");
    }
}

/// Remove old events now and then every hour.
pub fn spawn_purge(st: Arc<AppState>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(3600));
        loop {
            every.tick().await;
            let st = st.clone();
            let _ = tokio::task::spawn_blocking(move || purge(&st)).await;
        }
    });
}

pub fn purge(st: &AppState) {
    let cutoff = now_ms().saturating_sub(st.cfg.audit.retention_days * 86_400_000);
    match st.store.purge_audit_before(cutoff) {
        Ok(0) => {}
        Ok(n) => tracing::info!(removed = n, days = st.cfg.audit.retention_days, "removed old audit events"),
        Err(e) => tracing::error!(error = %format!("{e:#}"), "removing old audit events failed"),
    }
}
