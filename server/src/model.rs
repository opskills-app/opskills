//! Domain types shared by the API, the store and the executor.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::template::{self, seconds};

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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub run: String,
    /// Overrides the runbook's step timeout. Accepts seconds or a duration like "10m".
    #[serde(default, deserialize_with = "seconds::option::deserialize")]
    pub timeout_seconds: Option<u64>,
    /// How many more times to try the step after it fails or times out.
    #[serde(default)]
    pub retries: u32,
    /// Wait between attempts. Accepts seconds or a duration like "30s".
    #[serde(default, deserialize_with = "seconds::deserialize")]
    pub retry_delay_seconds: u64,
    /// When the step still fails, record it and go on as if it had succeeded.
    #[serde(default)]
    pub continue_on_error: bool,
}

pub const MAX_RETRIES: u32 = 10;
pub const MAX_RETRY_DELAY: u64 = 3600;
pub const MAX_TIMEOUT: u64 = 86_400;

/// What a run does after a step fails (unless that step continues on error).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    /// Skip every later step; the run fails.
    #[default]
    Stop,
    /// Run every later step anyway, then fail the run.
    FinishThenFail,
}

pub const MAX_OUTPUT_LINES: u64 = 20_000;
const MIN_OUTPUT_LINES: u64 = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitAction {
    /// Keep running and drop further output.
    #[default]
    Truncate,
    /// Stop the run and fail it.
    Fail,
}

/// How much output a run keeps, counting system lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputLimit {
    pub lines: u64,
    #[serde(default)]
    pub action: LimitAction,
}

impl Default for OutputLimit {
    fn default() -> Self {
        OutputLimit { lines: MAX_OUTPUT_LINES, action: LimitAction::Truncate }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    #[default]
    Text,
    Number,
    Boolean,
    Choice,
    /// Like text, but masked in output and never stored with the run.
    Secret,
}

/// A value the person starting a run fills in.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    /// Lower case, used as `{{ inputs.<name> }}` and `$OPSKILLS_INPUT_<NAME>`.
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type", default)]
    pub kind: InputKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    /// Allowed values of a choice input.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Regular expression a text or secret value must match in full.
    #[serde(default)]
    pub pattern: Option<String>,
}

pub const MAX_INPUTS: usize = 30;
const MAX_INPUT_VALUE: usize = 4096;

/// An input value for one run. Held only in memory.
#[derive(Clone, Debug)]
pub struct InputValue {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

impl InputValue {
    pub fn env_name(&self) -> String {
        format!("OPSKILLS_INPUT_{}", self.name.to_ascii_uppercase())
    }
}

fn compile_pattern(pattern: &str) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("Pattern {pattern:?} is not a valid regular expression: {e}"))
}

impl InputDef {
    fn display(&self) -> &str {
        if self.label.is_empty() { &self.name } else { &self.label }
    }

    /// Check a value given for this input and return it normalized.
    pub fn check_value(&self, value: &str) -> Result<String, String> {
        let who = self.display();
        match self.kind {
            InputKind::Text | InputKind::Secret => {
                if value.chars().count() > MAX_INPUT_VALUE || value.contains('\0') {
                    return Err(format!("{who} must be at most {MAX_INPUT_VALUE} characters."));
                }
                if let Some(p) = &self.pattern
                    && !compile_pattern(p)?.is_match(value) {
                        return Err(format!("{who} must match the pattern {p}."));
                    }
                Ok(value.to_string())
            }
            InputKind::Number => {
                let t = value.trim();
                match t.parse::<f64>() {
                    Ok(n) if n.is_finite() => Ok(t.to_string()),
                    _ => Err(format!("{who} must be a number.")),
                }
            }
            InputKind::Boolean => match value.trim().to_ascii_lowercase().as_str() {
                "true" => Ok("true".into()),
                "false" => Ok("false".into()),
                _ => Err(format!("{who} must be true or false.")),
            },
            InputKind::Choice => {
                if self.choices.iter().any(|c| c == value) {
                    Ok(value.to_string())
                } else {
                    Err(format!("{who} must be one of: {}.", self.choices.join(", ")))
                }
            }
        }
    }

    fn validate(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_string();
        let n = &self.name;
        let ok = n.len() <= 40 && n.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(format!("Input name {n:?} must be 1 to 40 lower case letters, digits or underscores, starting with a letter, like service_name."));
        }
        self.label = self.label.trim().to_string();
        self.description = self.description.trim().to_string();
        if self.label.chars().count() > 100 || self.description.chars().count() > 500 {
            return Err(format!("Input {n}: label must be at most 100 characters and description at most 500."));
        }
        if self.kind == InputKind::Choice {
            let mut seen: Vec<String> = Vec::new();
            for c in &self.choices {
                let c = c.trim().to_string();
                if c.is_empty() || c.chars().count() > 200 {
                    return Err(format!("Input {n}: each choice must be 1 to 200 characters."));
                }
                if !seen.contains(&c) {
                    seen.push(c);
                }
            }
            if seen.is_empty() || seen.len() > 100 {
                return Err(format!("Input {n}: a choice input needs 1 to 100 choices."));
            }
            self.choices = seen;
        } else if !self.choices.is_empty() {
            return Err(format!("Input {n}: only choice inputs have choices."));
        }
        self.pattern = self.pattern.take().filter(|p| !p.trim().is_empty());
        if let Some(p) = &self.pattern {
            if !matches!(self.kind, InputKind::Text | InputKind::Secret) {
                return Err(format!("Input {n}: only text and secret inputs can have a pattern."));
            }
            if p.chars().count() > 500 {
                return Err(format!("Input {n}: the pattern must be at most 500 characters."));
            }
            compile_pattern(p)?;
        }
        if self.kind == InputKind::Secret && self.default.is_some() {
            return Err(format!("Input {n}: a secret input can't have a default. Use a vault secret for fixed values."));
        }
        if let Some(d) = self.default.take()
            && (!d.is_empty() || self.kind == InputKind::Text) {
                self.default = Some(self.check_value(&d).map_err(|e| format!("Default for {e}"))?);
            }
        Ok(self)
    }
}

/// Check the values given when starting a run against a runbook's inputs.
pub fn resolve_inputs(defs: &[InputDef], given: &serde_json::Map<String, serde_json::Value>) -> Result<Vec<InputValue>, String> {
    if let Some(unknown) = given.keys().find(|k| !defs.iter().any(|d| &d.name == *k)) {
        return Err(format!("This runbook has no input named {unknown}."));
    }
    let mut out = Vec::new();
    for def in defs {
        let raw = match given.get(&def.name) {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            Some(_) => return Err(format!("{} must be a single value.", def.display())),
        };
        let raw = raw.filter(|v| !v.is_empty() || def.kind == InputKind::Text && !def.required);
        let value = match (raw, &def.default) {
            (Some(v), _) => def.check_value(&v)?,
            (None, Some(d)) => d.clone(),
            (None, None) if def.required => return Err(format!("{} is required.", def.display())),
            (None, None) => String::new(),
        };
        out.push(InputValue { name: def.name.clone(), value, secret: def.kind == InputKind::Secret });
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runbook {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<InputDef>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub on_step_failure: FailurePolicy,
    #[serde(default)]
    pub output_limit: OutputLimit,
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
    #[serde(default)]
    pub inputs: Vec<InputDef>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub on_step_failure: FailurePolicy,
    #[serde(default)]
    pub output_limit: Option<OutputLimit>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub targets: Targets,
    #[serde(default, deserialize_with = "seconds::option::deserialize")]
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
        if self.inputs.len() > MAX_INPUTS {
            return Err(format!("A runbook can have at most {MAX_INPUTS} inputs."));
        }
        let mut inputs: Vec<InputDef> = Vec::new();
        for def in std::mem::take(&mut self.inputs) {
            let def = def.validate()?;
            if inputs.iter().any(|d| d.name == def.name) {
                return Err(format!("Two inputs are named {}. Names must be unique.", def.name));
            }
            inputs.push(def);
        }
        self.inputs = inputs;
        let input_names: Vec<String> = self.inputs.iter().map(|d| d.name.clone()).collect();
        if let Some(limit) = self.output_limit
            && !(MIN_OUTPUT_LINES..=MAX_OUTPUT_LINES).contains(&limit.lines) {
                return Err(format!("The output limit must be {MIN_OUTPUT_LINES} to {MAX_OUTPUT_LINES} lines."));
            }
        if self.steps.is_empty() || self.steps.len() > 50 {
            return Err("A runbook needs 1 to 50 steps.".into());
        }
        for (i, step) in self.steps.iter_mut().enumerate() {
            if step.timeout_seconds.is_some_and(|t| !(1..=MAX_TIMEOUT).contains(&t)) {
                return Err(format!("Step {} timeout must be 1 second to 1 day.", i + 1));
            }
            if step.retries > MAX_RETRIES {
                return Err(format!("Step {} can retry at most {MAX_RETRIES} times.", i + 1));
            }
            if step.retry_delay_seconds > MAX_RETRY_DELAY {
                return Err(format!("Step {} retry delay must be at most 1 hour.", i + 1));
            }
            template::check(&step.run, &input_names).map_err(|e| format!("Step {}: {e}", i + 1))?;
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
            Some(t) if !(1..=MAX_TIMEOUT).contains(&t) => return Err("Step timeout must be 1 second to 1 day.".into()),
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
    /// This step's timeout; 0 in runs from before per-step timeouts (use the run's).
    #[serde(default)]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub retries: u32,
    #[serde(default)]
    pub retry_delay_seconds: u64,
    #[serde(default)]
    pub continue_on_error: bool,
    /// How many times the step was started.
    #[serde(default)]
    pub attempts: u32,
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
    /// Input values the run started with; secret ones are masked.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub on_step_failure: FailurePolicy,
    #[serde(default)]
    pub output_limit: OutputLimit,
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
    pub fn new(runbook: &Runbook, started_by: &str, hosts: Vec<RunHost>, inputs: &[InputValue]) -> Self {
        Run {
            inputs: inputs.iter().map(|i| (i.name.clone(), if i.secret { MASK.to_string() } else { i.value.clone() })).collect(),
            on_step_failure: runbook.on_step_failure,
            output_limit: runbook.output_limit,
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
                        timeout_seconds: s.timeout_seconds.unwrap_or(runbook.timeout_seconds),
                        retries: s.retries,
                        retry_delay_seconds: s.retry_delay_seconds,
                        continue_on_error: s.continue_on_error,
                        attempts: 0,
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
            inputs: vec![],
            steps: steps.into_iter().map(|(n, r)| Step { name: n.into(), run: r.into(), ..Default::default() }).collect(),
            on_step_failure: FailurePolicy::Stop,
            output_limit: None,
            secrets: vec![],
            targets: Targets::default(),
            timeout_seconds: None,
        }
    }

    fn def(name: &str, kind: InputKind) -> InputDef {
        InputDef { name: name.into(), kind, ..Default::default() }
    }

    #[test]
    fn inputs_are_validated() {
        let mut rb = input("x", vec![("a", "echo {{ inputs.service }}")]);
        assert!(rb.clone().validate().is_err(), "reference to a missing input");
        rb.inputs = vec![def("service", InputKind::Text)];
        assert!(rb.clone().validate().is_ok());
        for bad in ["Service", "1x", "a-b", "", &"x".repeat(41)] {
            let mut r = rb.clone();
            r.inputs = vec![def(bad, InputKind::Text)];
            assert!(r.validate().is_err(), "{bad}");
        }
        let mut dup = rb.clone();
        dup.inputs = vec![def("service", InputKind::Text), def("service", InputKind::Number)];
        assert!(dup.validate().is_err());
        let mut choice = rb.clone();
        choice.inputs = vec![def("service", InputKind::Choice)];
        assert!(choice.clone().validate().is_err(), "choice without choices");
        choice.inputs[0].choices = vec!["nginx".into(), " nginx ".into(), "redis".into()];
        assert_eq!(choice.validate().unwrap().inputs[0].choices, vec!["nginx", "redis"]);
        let mut secret_default = rb.clone();
        secret_default.inputs = vec![InputDef { default: Some("hunter22".into()), ..def("service", InputKind::Secret) }];
        assert!(secret_default.validate().is_err());
        let mut bad_default = rb.clone();
        bad_default.inputs = vec![InputDef { default: Some("ten".into()), ..def("service", InputKind::Number) }];
        assert!(bad_default.validate().is_err());
        let mut bad_pattern = rb.clone();
        bad_pattern.inputs = vec![InputDef { pattern: Some("(".into()), ..def("service", InputKind::Text) }];
        assert!(bad_pattern.validate().is_err());
    }

    #[test]
    fn step_options_are_checked() {
        let mut rb = input("x", vec![("a", "true")]);
        rb.steps[0].retries = MAX_RETRIES + 1;
        assert!(rb.clone().validate().is_err());
        rb.steps[0].retries = 2;
        rb.steps[0].retry_delay_seconds = MAX_RETRY_DELAY + 1;
        assert!(rb.clone().validate().is_err());
        rb.steps[0].retry_delay_seconds = 5;
        rb.steps[0].timeout_seconds = Some(0);
        assert!(rb.clone().validate().is_err());
        rb.steps[0].timeout_seconds = Some(30);
        rb.output_limit = Some(OutputLimit { lines: 5, action: LimitAction::Fail });
        assert!(rb.clone().validate().is_err());
        rb.output_limit = Some(OutputLimit { lines: 100, action: LimitAction::Fail });
        assert!(rb.validate().is_ok());
        let parsed: RunbookInput = serde_json::from_value(serde_json::json!({
            "name": "x", "timeout_seconds": "2h",
            "steps": [{"name": "a", "run": "true", "timeout_seconds": "90s", "retry_delay_seconds": "1m"}]
        }))
        .unwrap();
        assert_eq!(parsed.timeout_seconds, Some(7200));
        assert_eq!(parsed.steps[0].timeout_seconds, Some(90));
        assert_eq!(parsed.steps[0].retry_delay_seconds, 60);
    }

    #[test]
    fn input_values_are_resolved() {
        let defs = vec![
            InputDef { required: true, pattern: Some("[a-z]+".into()), ..def("service", InputKind::Text) },
            InputDef { default: Some("3".into()), ..def("count", InputKind::Number) },
            def("dry_run", InputKind::Boolean),
            InputDef { choices: vec!["eu".into(), "us".into()], default: Some("eu".into()), ..def("region", InputKind::Choice) },
            def("token", InputKind::Secret),
        ];
        let given = |v: serde_json::Value| v.as_object().unwrap().clone();
        let ok = resolve_inputs(&defs, &given(serde_json::json!({"service": "nginx", "dry_run": true, "token": "abcd"}))).unwrap();
        let get = |n: &str| ok.iter().find(|i| i.name == n).unwrap().value.clone();
        assert_eq!((get("service"), get("count"), get("dry_run"), get("region"), get("token")), ("nginx".into(), "3".into(), "true".into(), "eu".into(), "abcd".into()));
        assert!(ok.iter().find(|i| i.name == "token").unwrap().secret);
        assert_eq!(ok[0].env_name(), "OPSKILLS_INPUT_SERVICE");
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"service": ""}),
            serde_json::json!({"service": "Nginx"}),
            serde_json::json!({"service": "nginx", "count": "many"}),
            serde_json::json!({"service": "nginx", "dry_run": "maybe"}),
            serde_json::json!({"service": "nginx", "region": "asia"}),
            serde_json::json!({"service": "nginx", "nope": 1}),
            serde_json::json!({"service": ["a"]}),
        ] {
            assert!(resolve_inputs(&defs, &given(bad.clone())).is_err(), "{bad}");
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
