//! Domain types shared by the API, the store and the executor.

use serde::{Deserialize, Serialize};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
}

impl Status {
    pub fn is_finished(self) -> bool {
        matches!(self, Status::Succeeded | Status::Failed | Status::Cancelled | Status::Skipped)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub run: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runbook {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub steps: Vec<Step>,
    /// Names of vault secrets exposed to every step as environment variables.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Where the steps run. Empty means the OpSkills server itself.
    #[serde(default)]
    pub targets: Targets,
    /// Maximum time for each step, in seconds.
    pub timeout_seconds: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

/// What clients send to create or replace a runbook.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunbookInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub targets: Targets,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Hosts a runbook runs on: specific hosts by id, plus every host with one of the tags.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Targets {
    pub fn is_server(&self) -> bool {
        self.hosts.is_empty() && self.tags.is_empty()
    }
}

pub const LOCAL_HOST_ID: &str = "local";
pub const LOCAL_HOST_NAME: &str = "OpSkills server";

/// Normalize a tag, or explain why it's not valid.
pub fn normalize_tag(tag: &str) -> Result<Option<String>, String> {
    let t = tag.trim().to_lowercase();
    if t.is_empty() {
        return Ok(None);
    }
    if t.len() > 40 || !t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Err(format!("Tag {t:?} must be up to 40 letters, digits, dashes, dots or underscores."));
    }
    Ok(Some(t))
}

pub const DEFAULT_STEP_TIMEOUT: u64 = 600;

impl RunbookInput {
    /// Check every field and return a normalized copy, or a message for the user.
    pub fn validate(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_string();
        self.description = self.description.trim().to_string();
        if self.name.is_empty() || self.name.chars().count() > 100 {
            return Err("Name must be 1 to 100 characters.".into());
        }
        if self.description.chars().count() > 1000 {
            return Err("Description must be at most 1000 characters.".into());
        }
        let mut tags = Vec::new();
        for tag in &self.tags {
            if let Some(t) = normalize_tag(tag)?
                && !tags.contains(&t) {
                    tags.push(t);
                }
        }
        let mut target_tags = Vec::new();
        for tag in &self.targets.tags {
            if let Some(t) = normalize_tag(tag)?
                && !target_tags.contains(&t) {
                    target_tags.push(t);
                }
        }
        self.targets.tags = target_tags;
        self.targets.hosts.dedup();
        if self.targets.hosts.len() > 200 || self.targets.tags.len() > 20 {
            return Err("A runbook can target at most 200 hosts and 20 tags.".into());
        }
        if tags.len() > 20 {
            return Err("A runbook can have at most 20 tags.".into());
        }
        self.tags = tags;
        if self.steps.is_empty() || self.steps.len() > 50 {
            return Err("A runbook needs 1 to 50 steps.".into());
        }
        for (i, step) in self.steps.iter_mut().enumerate() {
            step.name = step.name.trim().to_string();
            if step.name.is_empty() {
                step.name = format!("Step {}", i + 1);
            }
            if step.name.chars().count() > 100 {
                return Err(format!("Step {} name must be at most 100 characters.", i + 1));
            }
            if step.run.trim().is_empty() || step.run.len() > 20_000 {
                return Err(format!("Step {} command must be 1 to 20000 characters.", i + 1));
            }
        }
        let mut secrets: Vec<String> = Vec::new();
        for name in &self.secrets {
            validate_secret_name(name)?;
            if !secrets.contains(name) {
                secrets.push(name.clone());
            }
        }
        if secrets.len() > 20 {
            return Err("A runbook can use at most 20 secrets.".into());
        }
        self.secrets = secrets;
        match self.timeout_seconds {
            Some(t) if !(1..=86_400).contains(&t) => return Err("Step timeout must be 1 to 86400 seconds.".into()),
            _ => {}
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepResult {
    pub name: String,
    pub run: String,
    #[serde(default = "local_id")]
    pub host_id: String,
    #[serde(default = "local_name")]
    pub host_name: String,
    pub status: Status,
    pub exit_code: Option<i32>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub runbook_id: String,
    pub runbook_name: String,
    pub status: Status,
    pub trigger: String,
    pub started_by: String,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Hosts in the order they run.
    #[serde(default)]
    pub hosts: Vec<RunHost>,
    pub steps: Vec<StepResult>,
    pub error: Option<String>,
    pub log_lines: u64,
}

fn local_id() -> String {
    LOCAL_HOST_ID.into()
}

fn local_name() -> String {
    LOCAL_HOST_NAME.into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunHost {
    pub id: String,
    pub name: String,
}

impl Run {
    /// A queued run of `runbook` on `hosts`, one after another; each host runs every step.
    pub fn new(runbook: &Runbook, started_by: &str, hosts: Vec<RunHost>) -> Self {
        Run {
            id: uuid::Uuid::now_v7().to_string(),
            runbook_id: runbook.id.clone(),
            runbook_name: runbook.name.clone(),
            status: Status::Queued,
            trigger: "manual".into(),
            started_by: started_by.into(),
            created_at: now_ms(),
            started_at: None,
            finished_at: None,
            timeout_seconds: runbook.timeout_seconds,
            secrets: runbook.secrets.clone(),
            steps: hosts
                .iter()
                .flat_map(|h| {
                    runbook.steps.iter().map(move |s| StepResult {
                        name: s.name.clone(),
                        run: s.run.clone(),
                        host_id: h.id.clone(),
                        host_name: h.name.clone(),
                        status: Status::Pending,
                        exit_code: None,
                        started_at: None,
                        finished_at: None,
                    })
                })
                .collect(),
            hosts,
            error: None,
            log_lines: 0,
        }
    }
}

/// A machine with the OpSkills agent, enrolled with this server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub name: String,
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
    pub created_at: u64,
    pub expires_at: u64,
    pub created_by: String,
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub ts: u64,
    /// Index of the step this line belongs to.
    pub step: usize,
    /// Name of the host the line came from.
    #[serde(default)]
    pub host: String,
    /// "stdout", "stderr" or "system".
    pub stream: String,
    pub line: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, steps: Vec<(&str, &str)>) -> RunbookInput {
        RunbookInput {
            name: name.into(),
            description: String::new(),
            tags: vec![" Web ".into(), "web".into(), "".into()],
            steps: steps.into_iter().map(|(n, r)| Step { name: n.into(), run: r.into() }).collect(),
            secrets: vec![],
            targets: Targets::default(),
            timeout_seconds: None,
        }
    }

    #[test]
    fn validation_normalizes_and_rejects() {
        let ok = input("  Check disk  ", vec![("", "df -h")]).validate().unwrap();
        assert_eq!(ok.name, "Check disk");
        assert_eq!(ok.tags, vec!["web"]);
        assert_eq!(ok.steps[0].name, "Step 1");
        assert!(input("", vec![("a", "true")]).validate().is_err());
        assert!(input("x", vec![]).validate().is_err());
        assert!(input("x", vec![("a", "   ")]).validate().is_err());
        let mut bad_tag = input("x", vec![("a", "true")]);
        bad_tag.tags = vec!["no spaces".into()];
        assert!(bad_tag.validate().is_err());
        let mut bad_secret = input("x", vec![("a", "true")]);
        bad_secret.secrets = vec!["db_password".into()];
        assert!(bad_secret.validate().is_err());
        for name in ["PATH", "OPSKILLS_X", "1ABC", ""] {
            assert!(validate_secret_name(name).is_err(), "{name}");
        }
        assert!(validate_secret_name("DB_PASSWORD").is_ok());
        assert!(validate_secret_value("abc").is_err());
        let mut bad_timeout = input("x", vec![("a", "true")]);
        bad_timeout.timeout_seconds = Some(0);
        assert!(bad_timeout.validate().is_err());
    }
}
