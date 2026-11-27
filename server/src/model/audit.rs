//! The audit log: who changed what, and when.

use serde::{Deserialize, Serialize};

/// One change. Reads aren't recorded, and secret values never appear here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// UUID v7, so ids sort by time.
    pub id: String,
    pub at: u64,
    /// The user who did it, or "system" for schedules, webhooks and agents.
    pub actor: String,
    /// How: "token ci", "schedule nightly", "webhook deploy", "agent web-1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// What, as `<area>.<verb>`: "runbook.update", "auth.sign_in_failed".
    pub action: String,
    /// The workspace it happened in; none for server-wide changes like users and sign-ins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AuditTarget>,
    /// A short note, like "role operator to editor". Never a secret value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditTarget {
    /// "runbook", "run", "user", "token", "secret", "host", "webhook", "settings".
    pub kind: String,
    pub id: String,
    pub name: String,
}

impl AuditTarget {
    pub fn new(kind: &str, id: impl Into<String>, name: impl Into<String>) -> Self {
        AuditTarget { kind: kind.into(), id: id.into(), name: name.into() }
    }
}
