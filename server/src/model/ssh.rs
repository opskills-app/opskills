//! Reaching hosts without the agent over SSH: what a source says about it, what a run
//! records, and the host keys pinned for each host.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How a source's hosts sign in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuth {
    /// A private key from the vault.
    #[default]
    Key,
    /// A password from the vault.
    Password,
}

/// A source's SSH settings: its hosts that no agent enrolled are reached over SSH. Set by
/// the source's Admin; the credential never comes from the document a source returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSsh {
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub auth: SshAuth,
    /// The vault secret with the key or password, like `ssh/OPS_KEY` or `shared/ssh/KEY`.
    pub secret: String,
    /// Run steps as root with `sudo -n` when the user isn't root.
    #[serde(default = "yes")]
    pub sudo: bool,
}

fn default_user() -> String {
    "root".into()
}
fn default_port() -> u16 {
    22
}
fn yes() -> bool {
    true
}

impl SourceSsh {
    pub fn validate(&mut self) -> Result<(), String> {
        self.user = self.user.trim().to_string();
        validate_user(&self.user)?;
        if self.port == 0 {
            return Err("The SSH port must be 1 to 65535.".into());
        }
        crate::model::SecretRef::parse(&self.secret).map_err(|e| format!("SSH secret: {e}"))?;
        Ok(())
    }
}

/// What a source's document may say about one host's SSH: never a credential. Other
/// fields are refused, so a document can't think it set a password.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSsh {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The host's public keys (`ssh-ed25519 AAAA...`) or their fingerprints
    /// (`SHA256:...`). When given, the first connection must match one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_keys: Vec<String>,
}

impl HostSsh {
    pub fn validate(&mut self) -> Result<(), String> {
        if let Some(u) = &mut self.user {
            *u = u.trim().to_string();
            validate_user(u)?;
        }
        if self.port == Some(0) {
            return Err("the SSH port must be 1 to 65535".into());
        }
        if self.host_keys.len() > 10 {
            return Err("at most 10 host keys".into());
        }
        for k in &mut self.host_keys {
            *k = k.trim().to_string();
            if !(k.starts_with("SHA256:") && k.len() == 50 || public_key_parts(k).is_some()) {
                return Err(format!("{k:?} is not a public key like \"ssh-ed25519 AAAA...\" or a fingerprint like \"SHA256:...\""));
            }
        }
        Ok(())
    }
}

/// A user name SSH and sudo accept, that can't be read as an option.
pub fn validate_user(user: &str) -> Result<(), String> {
    let ok = !user.is_empty()
        && user.len() <= 64
        && !user.starts_with('-')
        && user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok { Ok(()) } else { Err(format!("{user:?} is not an SSH user name: letters, digits, dots, dashes and underscores.")) }
}

/// A hostname or IP address to connect to, that can't be read as an option.
pub fn validate_address(addr: &str) -> Result<(), String> {
    let ok = !addr.is_empty()
        && addr.len() <= 255
        && !addr.starts_with('-')
        && addr.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'));
    if ok { Ok(()) } else { Err(format!("{addr:?} is not a hostname or IP address SSH can connect to.")) }
}

/// How a run reaches a host over SSH. No secret values: the credential is read from the
/// vault when a step starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SshTarget {
    pub address: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// The vault secret with the key or password.
    pub secret: String,
    pub sudo: bool,
    /// Keys or fingerprints the source expects the host to have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_keys: Vec<String>,
}

impl SshTarget {
    /// `user@address:port`, for messages.
    pub fn label(&self) -> String {
        if self.port == 22 { format!("{}@{}", self.user, self.address) } else { format!("{}@{}:{}", self.user, self.address, self.port) }
    }
}

/// A host's pinned SSH keys and how its last connection went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SshHostState {
    /// Pinned public keys, `type base64`.
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// When a connection found a different key than the pinned one; cleared when an
    /// Admin accepts a new key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_changed_at: Option<u64>,
}

impl SshHostState {
    pub fn fingerprints(&self) -> Vec<String> {
        self.keys.iter().filter_map(|k| fingerprint(k)).collect()
    }
}

/// `(type, blob)` of a public key written `type base64 [comment]`.
fn public_key_parts(line: &str) -> Option<(&str, Vec<u8>)> {
    let mut parts = line.split_whitespace();
    let kind = parts.next()?;
    let blob = base64::engine::general_purpose::STANDARD.decode(parts.next()?).ok()?;
    let known = kind.starts_with("ssh-") || kind.starts_with("ecdsa-sha2-") || kind.starts_with("sk-");
    (known && blob.len() >= 16).then_some((kind, blob))
}

/// `type base64` without a comment, or `None` if it isn't a public key.
pub fn normalize_key(line: &str) -> Option<String> {
    let (kind, blob) = public_key_parts(line)?;
    Some(format!("{kind} {}", base64::engine::general_purpose::STANDARD.encode(blob)))
}

/// The key's SHA-256 fingerprint as OpenSSH prints it: `SHA256:` and unpadded base64.
pub fn fingerprint(line: &str) -> Option<String> {
    let (_, blob) = public_key_parts(line)?;
    Some(format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&blob))))
}

/// Whether `key` is one the source expects: its own line, or its fingerprint.
pub fn expected(key: &str, expect: &[String]) -> bool {
    let fp = fingerprint(key);
    let norm = normalize_key(key);
    expect.iter().any(|e| if e.starts_with("SHA256:") { fp.as_deref() == Some(e.as_str()) } else { normalize_key(e) == norm })
}

#[cfg(test)]
mod tests {
    use super::*;

    // An ed25519 public key and its fingerprint, as ssh-keygen -l prints it.
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl test";

    #[test]
    fn fingerprints_match_openssh() {
        // ssh-keygen -l prints SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU for it.
        let fp = fingerprint(KEY).unwrap();
        assert_eq!(fp, "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU");
        assert_eq!(normalize_key(KEY).unwrap(), "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl");
        assert!(expected(KEY, std::slice::from_ref(&fp)));
        assert!(expected(KEY, &["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl".into()]));
        assert!(!expected(KEY, &["SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()]));
        assert!(fingerprint("ssh-ed25519 not-base64!").is_none());
        assert!(fingerprint("rsa AAAA").is_none());
    }

    #[test]
    fn names_that_could_be_options_are_refused() {
        assert!(validate_user("ops").is_ok() && validate_user("deploy.bot").is_ok());
        for bad in ["", "-oProxyCommand=x", "a b", "ops;id", "o\nps"] {
            assert!(validate_user(bad).is_err(), "{bad:?}");
        }
        assert!(validate_address("web-1.internal").is_ok() && validate_address("10.0.0.5").is_ok() && validate_address("fe80::1%eth0").is_ok());
        for bad in ["", "-oProxyCommand=x", "a b", "host;id", "[::1]"] {
            assert!(validate_address(bad).is_err(), "{bad:?}");
        }
    }
}
