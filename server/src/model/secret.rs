//! Vault secrets: metadata and validation. Values never appear in these types.


use serde::{Deserialize, Serialize};

use super::{DEFAULT_WORKSPACE, default_workspace};

/// What a secret's encryption is bound to, so a sealed value can't be moved to another
/// name or workspace. Names without folders keep the binding they had before folders:
/// the bare name in `default` (as before workspaces) and `<workspace>/<name>` elsewhere.
/// Names with folders bind to `<workspace>:<path>`; `:` can't appear in slugs or names,
/// so no two secrets share a binding. (`<workspace>/<path>` would: `default`'s
/// `ops/DB_PASSWORD` and the `ops` workspace's `DB_PASSWORD`.)
pub fn secret_binding(workspace: &str, path: &str) -> String {
    if path.contains('/') {
        format!("{workspace}:{path}")
    } else if workspace == DEFAULT_WORKSPACE {
        path.to_string()
    } else {
        format!("{workspace}/{path}")
    }
}

/// The binding of a shared secret. `@` can't start a workspace slug.
pub fn shared_secret_binding(path: &str) -> String {
    format!("@shared:{path}")
}

/// How runbooks, notifications and the email settings name a shared secret:
/// `shared/<path>`. A workspace's vault can't have a top folder called `shared`.
pub const SHARED_PREFIX: &str = "shared/";
const SHARED_FOLDER: &str = "shared";
/// Folders a secret can be in, at most.
pub const MAX_FOLDERS: usize = 5;
const MAX_PATH_LEN: usize = 200;

/// A secret as a runbook, notification or the email settings name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretRef {
    /// A path in the vault of the workspace that uses it.
    Workspace(String),
    /// A shared secret's path, without `shared/`.
    Shared(String),
}

impl SecretRef {
    pub fn parse(reference: &str) -> Result<SecretRef, String> {
        match reference.strip_prefix(SHARED_PREFIX) {
            Some(path) => {
                validate_secret_path(path)?;
                Ok(SecretRef::Shared(path.to_string()))
            }
            None => {
                validate_secret_path(reference)?;
                Ok(SecretRef::Workspace(reference.to_string()))
            }
        }
    }
}

/// The environment variable a secret becomes: the last part of its name.
pub fn secret_env_name(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

fn validate_folder(folder: &str) -> Result<(), String> {
    let ok = !folder.is_empty()
        && folder.len() <= 40
        && folder.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
        && folder.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if ok { Ok(()) } else { Err(format!("Folder {folder:?} must be 1 to 40 lowercase letters, digits, dots, dashes or underscores, starting with a letter or digit, like prod or db-1.")) }
}

/// A secret's full name: up to five folders, then the name, like `prod/db/DB_PASSWORD`.
pub fn validate_secret_path(path: &str) -> Result<(), String> {
    if path.len() > MAX_PATH_LEN {
        return Err(format!("A secret's full name can be at most {MAX_PATH_LEN} characters."));
    }
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    if parts.len() > MAX_FOLDERS {
        return Err(format!("A secret can be in at most {MAX_FOLDERS} folders, like prod/db/DB_PASSWORD."));
    }
    for folder in parts {
        validate_folder(folder)?;
    }
    validate_secret_name(name)
}

/// The full name of a secret in a workspace's vault: a path whose top folder isn't
/// `shared`, which names shared secrets.
pub fn validate_workspace_secret_path(path: &str) -> Result<(), String> {
    validate_secret_path(path)?;
    if path.split('/').next() == Some(SHARED_FOLDER) && path.contains('/') {
        return Err("A top folder can't be called shared: that name is for shared secrets, which server admins add under Vault > Shared.".into());
    }
    Ok(())
}

/// Check how something names a secret: a workspace path or `shared/<path>`.
pub fn validate_secret_ref(reference: &str) -> Result<(), String> {
    SecretRef::parse(reference).map(|_| ())
}



/// Metadata of a vault secret. The value is never part of this type.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Secret {
    pub name: String,
    /// Names are unique within a workspace.
    #[serde(default = "default_workspace")]
    pub workspace: String,
    pub kind: String,
    pub description: String,
    pub created_at: u64,
    pub updated_at: u64,
    /// Who last added, replaced or changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

/// Metadata of a shared secret: server admins manage it, and the workspaces it's
/// shared with can use it as `shared/<name>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedSecret {
    pub name: String,
    pub kind: String,
    pub description: String,
    /// Shared with every workspace, now and later.
    #[serde(default)]
    pub all_workspaces: bool,
    /// Or with these.
    #[serde(default)]
    pub workspaces: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

impl SharedSecret {
    pub fn available_to(&self, workspace: &str) -> bool {
        self.all_workspaces || self.workspaces.iter().any(|w| w == workspace)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredSharedSecret {
    #[serde(flatten)]
    pub meta: SharedSecret,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSecretInput {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: String,
    pub value: String,
    #[serde(default)]
    pub all_workspaces: bool,
    #[serde(default)]
    pub workspaces: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSecretUpdate {
    /// A new full name: moves the secret.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub all_workspaces: Option<bool>,
    #[serde(default)]
    pub workspaces: Option<Vec<String>>,
}

/// What the store keeps: metadata plus the encrypted value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredSecret {
    #[serde(flatten)]
    pub meta: Secret,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretInput {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretUpdate {
    /// A new full name: moves the secret.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

/// Shown in place of a secret value.
pub const MASK: &str = "••••••";

pub const SECRET_KINDS: &[&str] = &["password", "token", "ssh_key", "certificate", "other"];
const RESERVED_NAMES: &[&str] = &["PATH", "HOME", "LANG", "PWD", "OLDPWD", "SHELL", "USER", "LOGNAME", "IFS", "TERM", "TMPDIR", "LD_PRELOAD", "LD_LIBRARY_PATH"];

/// Secret names become environment variable names.
pub fn validate_secret_name(name: &str) -> Result<(), String> {
    let ok_chars = name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    let starts_ok = name.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '_');
    if name.is_empty() || name.len() > 64 || !ok_chars || !starts_ok {
        return Err(format!("Secret name {name:?} must be 1 to 64 capital letters, digits or underscores, and not start with a digit, like DB_PASSWORD."));
    }
    if name.starts_with("OPSKILLS_") || RESERVED_NAMES.contains(&name) {
        return Err(format!("{name} is reserved. Choose another name."));
    }
    Ok(())
}

pub fn validate_secret_kind(kind: &str) -> Result<(), String> {
    if SECRET_KINDS.contains(&kind) { Ok(()) } else { Err(format!("Kind must be one of: {}.", SECRET_KINDS.join(", "))) }
}

pub fn validate_secret_value(value: &str) -> Result<(), String> {
    if value.chars().count() < 4 || value.len() > 65_536 {
        return Err("A secret value must be 4 characters to 64 KB.".into());
    }
    Ok(())
}

pub fn validate_secret_description(description: &str) -> Result<(), String> {
    if description.chars().count() > 500 { Err("Description must be at most 500 characters.".into()) } else { Ok(()) }
}
