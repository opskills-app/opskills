//! Hosts with the OpSkills agent, and enrollment tokens.


use serde::{Deserialize, Serialize};

use super::{default_workspace, default_workspaces};



/// A machine with the OpSkills agent, enrolled with this server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub name: String,
    /// The workspaces whose runbooks can run on it: the one it enrolled into, and any an
    /// Admin added it to. Never empty.
    #[serde(default = "default_workspaces")]
    pub workspaces: Vec<String>,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub agent_version: String,
    pub tags: Vec<String>,
    pub enrolled_at: u64,
    pub last_seen: u64,
}

/// What the store keeps: the host plus the SHA-256 of its agent secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredHost {
    #[serde(flatten)]
    pub host: Host,
    pub secret_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrollmentToken {
    /// The workspace the host joins.
    #[serde(default = "default_workspace")]
    pub workspace: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub created_by: String,
}
