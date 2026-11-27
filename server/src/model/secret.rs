//! Vault secrets: metadata and validation. Values never appear in these types.


use serde::{Deserialize, Serialize};



/// Metadata of a vault secret. The value is never part of this type.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Secret {
    pub name: String,
    pub kind: String,
    pub description: String,
    pub created_at: u64,
    pub updated_at: u64,
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
