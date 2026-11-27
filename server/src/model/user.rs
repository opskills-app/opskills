//! Users, server admins, and each user's role in the workspaces they belong to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a user may do in a workspace. Each role includes everything the ones before it may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Sees runbooks, runs, output and hosts.
    Viewer,
    /// Also starts, retries and cancels runs.
    Operator,
    /// Also edits runbooks (with their schedules, webhooks and notifications) and the vault.
    /// Anyone who can edit a runbook can run any command as root on the hosts it targets.
    Editor,
    /// Also manages the workspace's hosts and members.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

/// The user the first start creates, and the one a password reset restores.
pub const FIRST_ADMIN: &str = "admin";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    /// The sign-in name; also how runs and API tokens refer to the user.
    pub name: String,
    /// Server admins manage accounts, workspaces, email settings and everyone's API
    /// tokens, and act as Admin in every workspace.
    #[serde(default)]
    pub admin: bool,
    /// The user's role in each workspace they belong to, by workspace id.
    #[serde(default)]
    pub workspaces: BTreeMap<String, Role>,
    pub created_at: u64,
    /// Set for a generated password: the user must choose their own before doing anything else.
    #[serde(default)]
    pub must_change_password: bool,
    #[serde(default)]
    pub last_sign_in_at: Option<u64>,
    #[serde(default)]
    pub password_changed_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredUser {
    #[serde(flatten)]
    pub user: User,
    /// `argon2id$v=19$m=19456,t=2,p=1$<salt hex>$<hash hex>`
    pub password_hash: String,
}

impl User {
    /// What the user may do in a workspace: Admin for server admins, otherwise their
    /// role there, or nothing if they don't belong to it.
    pub fn role_in(&self, workspace: &str) -> Option<Role> {
        if self.admin { Some(Role::Admin) } else { self.workspaces.get(workspace).copied() }
    }
}

pub const MIN_PASSWORD: usize = 12;
const MAX_PASSWORD: usize = 256;

/// Names are 1 to 64 characters: lowercase letters, digits, `.`, `_` and `-`, starting with
/// a letter or digit. Returns the name trimmed and lowercased.
pub fn validate_username(name: &str) -> Result<String, String> {
    let name = name.trim().to_lowercase();
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(name)
    } else {
        Err("A user name is 1 to 64 lowercase letters, digits, dots, dashes or underscores, starting with a letter or digit.".into())
    }
}

pub fn validate_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_PASSWORD {
        return Err(format!("A password needs at least {MIN_PASSWORD} characters."));
    }
    if n > MAX_PASSWORD {
        return Err(format!("A password can have at most {MAX_PASSWORD} characters."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Viewer < Role::Operator && Role::Operator < Role::Editor && Role::Editor < Role::Admin);
        assert_eq!(serde_json::to_string(&Role::Operator).unwrap(), "\"operator\"");
    }

    #[test]
    fn roles_are_per_workspace_and_server_admins_have_all() {
        let mut u = User { name: "a".into(), admin: false, workspaces: BTreeMap::new(), created_at: 0, must_change_password: false, last_sign_in_at: None, password_changed_at: None };
        u.workspaces.insert("ops".into(), Role::Operator);
        assert_eq!(u.role_in("ops"), Some(Role::Operator));
        assert_eq!(u.role_in("payments"), None);
        u.admin = true;
        assert_eq!(u.role_in("payments"), Some(Role::Admin));
    }

    #[test]
    fn user_names_are_checked() {
        assert_eq!(validate_username(" Alice.B ").unwrap(), "alice.b");
        for bad in ["", "-x", ".x", "a b", "a/b", "é", &"a".repeat(65)] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
        assert!(validate_password("short").is_err());
        assert!(validate_password("twelve chars").is_ok());
    }
}
