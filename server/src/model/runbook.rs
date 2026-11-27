//! Runbooks: steps, inputs, targets and rollouts, and how they are checked.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::filter::HostFilter;
use crate::template::{self, seconds};

use super::*;

/// A step writes a file on its host instead of running a command.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyStep {
    /// Where to write. A relative path is inside the run's working folder.
    pub to: String,
    /// The file's content; `{{ ... }}` references are filled in as plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Or the value of this vault secret, which the runbook must list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// File mode in octal, like "0644". Defaults to 0644, or 0600 for a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

pub const MAX_COPY_CONTENT: usize = 64 * 1024;

impl CopyStep {
    pub fn mode_or_default(&self) -> String {
        self.mode.clone().unwrap_or_else(|| if self.secret.is_some() { "0600".into() } else { "0644".into() })
    }

    fn validate(&mut self, secrets: &[String], inputs: &[String]) -> Result<(), String> {
        self.to = self.to.trim().to_string();
        if self.to.is_empty() || self.to.len() > 4096 || self.to.contains(['\0', '\n', '\r']) || self.to.ends_with('/') {
            return Err("the destination must be a file path of up to 4096 characters, on one line.".into());
        }
        template::check(&self.to, inputs)?;
        match (&self.content, &self.secret) {
            (Some(c), None) => {
                if c.len() > MAX_COPY_CONTENT || c.contains('\0') {
                    return Err("the file content must be text of at most 64 KB.".into());
                }
                template::check(c, inputs)?;
            }
            (None, Some(s)) => {
                if !secrets.contains(s) {
                    return Err(format!("the secret {s} must also be selected under the runbook's secrets."));
                }
            }
            _ => return Err("give either the file content or a vault secret to copy, not both.".into()),
        }
        if let Some(m) = &self.mode {
            let digits = m.trim().trim_start_matches("0o");
            match u32::from_str_radix(digits, 8) {
                Ok(n) if n <= 0o7777 && !digits.is_empty() => self.mode = Some(format!("{n:04o}")),
                _ => return Err(format!("{m:?} is not a file mode. Use octal, like 0644 or 0600.")),
            }
        }
        Ok(())
    }
}

/// A step runs another runbook on its host and waits for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallStep {
    /// Id of the runbook to run.
    pub runbook: String,
    /// Values for the called runbook's inputs; `{{ ... }}` references are filled in as plain text.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

/// How deep calls may nest: a run, the runbook it calls, and so on.
pub const MAX_CALL_DEPTH: u32 = 5;

impl CallStep {
    fn validate(&mut self, inputs: &[String]) -> Result<(), String> {
        self.runbook = self.runbook.trim().to_string();
        if self.runbook.is_empty() {
            return Err("choose a runbook to call.".into());
        }
        for (name, value) in &self.inputs {
            if value.chars().count() > MAX_INPUT_VALUE {
                return Err(format!("the value for {name} must be at most {MAX_INPUT_VALUE} characters."));
            }
            template::check(value, inputs)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    /// The shell command. Empty when the step copies a file or calls a runbook.
    #[serde(default)]
    pub run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallStep>,
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

/// How a run spreads over its hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMode {
    /// One host after another.
    #[default]
    Sequential,
    /// Many hosts at once, up to `max_parallel`.
    Parallel,
    /// Groups of `batch_size` hosts, one group after another.
    Batches,
}

/// What a failed host means for the hosts that haven't started yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostFailure {
    /// Start no more hosts; hosts already running finish.
    Stop,
    /// Keep going with the other hosts; the run still fails.
    Continue,
}

/// A number of hosts, or a share of the run's hosts. JSON: `5` or `"25%"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchSize {
    Hosts(u32),
    Percent(u32),
}

impl BatchSize {
    /// Hosts per batch for a run on `total` hosts, at least 1.
    pub fn of(self, total: usize) -> usize {
        match self {
            BatchSize::Hosts(n) => n as usize,
            BatchSize::Percent(p) => (total * p as usize).div_ceil(100),
        }
        .max(1)
    }
}

impl Serialize for BatchSize {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            BatchSize::Hosts(n) => s.serialize_u32(*n),
            BatchSize::Percent(p) => s.serialize_str(&format!("{p}%")),
        }
    }
}

impl<'de> Deserialize<'de> for BatchSize {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u32),
            Text(String),
        }
        let bad = |t: &str| serde::de::Error::custom(format!("{t:?} is not a batch size. Use a number of hosts, like 5, or a percentage, like \"25%\"."));
        match Raw::deserialize(d)? {
            Raw::Number(n) => Ok(BatchSize::Hosts(n)),
            Raw::Text(t) => match t.trim().strip_suffix('%') {
                Some(p) => p.trim().parse().map(BatchSize::Percent).map_err(|_| bad(&t)),
                None => t.trim().parse().map(BatchSize::Hosts).map_err(|_| bad(&t)),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rollout {
    #[serde(default)]
    pub mode: RolloutMode,
    /// Parallel mode: the most hosts at once; 0 means all of them.
    #[serde(default)]
    pub max_parallel: u32,
    /// Batches mode: hosts per batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<BatchSize>,
    /// Hosts that run first, on their own, before any other host starts.
    #[serde(default)]
    pub canary: u32,
    /// Wait after the canary and between batches. Accepts seconds or a duration like "30s".
    #[serde(default, deserialize_with = "seconds::deserialize")]
    pub pause_seconds: u64,
    /// Unset: stop when the runbook stops at a failed step, continue when it finishes then fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_host_failure: Option<HostFailure>,
}

/// Hosts that run together, and how many of them at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wave {
    /// "Canary" or "Batch 2 of 4"; empty when the run is a single wave.
    pub label: String,
    pub hosts: std::ops::Range<usize>,
    pub concurrency: usize,
}

pub const MAX_ROLLOUT_HOSTS: u32 = 1000;

impl Rollout {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.max_parallel > MAX_ROLLOUT_HOSTS || self.canary > MAX_ROLLOUT_HOSTS {
            return Err(format!("Rollout: max_parallel and canary must be at most {MAX_ROLLOUT_HOSTS}."));
        }
        if self.pause_seconds > MAX_RETRY_DELAY {
            return Err("Rollout: the pause must be at most 1 hour.".into());
        }
        match (self.mode, self.batch_size) {
            (RolloutMode::Batches, None) => return Err("Rollout: batches need a batch_size, like 5 or \"25%\".".into()),
            (RolloutMode::Batches, Some(BatchSize::Hosts(n))) if n == 0 || n > MAX_ROLLOUT_HOSTS => {
                return Err(format!("Rollout: a batch must be 1 to {MAX_ROLLOUT_HOSTS} hosts."));
            }
            (RolloutMode::Batches, Some(BatchSize::Percent(p))) if p == 0 || p > 100 => return Err("Rollout: a batch must be 1% to 100% of the hosts.".into()),
            (RolloutMode::Batches, _) => {}
            (_, Some(_)) => return Err("Rollout: batch_size only applies to the batches mode.".into()),
            (_, None) => {}
        }
        if self.mode != RolloutMode::Parallel && self.max_parallel != 0 {
            return Err("Rollout: max_parallel only applies to the parallel mode.".into());
        }
        Ok(())
    }

    /// Whether a failed host stops the hosts that haven't started.
    pub fn stops_on_host_failure(&self, steps: FailurePolicy) -> bool {
        match self.on_host_failure {
            Some(HostFailure::Stop) => true,
            Some(HostFailure::Continue) => false,
            None => steps == FailurePolicy::Stop,
        }
    }

    /// The waves a run on `total` hosts goes through, in order: the canary hosts first
    /// (one at a time in sequential mode, together otherwise), then the rest.
    pub fn waves(&self, total: usize) -> Vec<Wave> {
        let mut waves = Vec::new();
        let canary = (self.canary as usize).min(total);
        let one_at_a_time = self.mode == RolloutMode::Sequential;
        if canary > 0 {
            waves.push(Wave { label: "Canary".into(), hosts: 0..canary, concurrency: if one_at_a_time { 1 } else { canary } });
        }
        let rest = canary..total;
        if !rest.is_empty() {
            match self.mode {
                RolloutMode::Sequential => waves.push(Wave { label: String::new(), hosts: rest, concurrency: 1 }),
                RolloutMode::Parallel => {
                    let cap = if self.max_parallel == 0 { rest.len() } else { self.max_parallel as usize };
                    waves.push(Wave { label: String::new(), hosts: rest.clone(), concurrency: cap.min(rest.len()) });
                }
                RolloutMode::Batches => {
                    let size = self.batch_size.unwrap_or(BatchSize::Hosts(1)).of(total);
                    let starts: Vec<usize> = rest.clone().step_by(size).collect();
                    for (k, start) in starts.iter().enumerate() {
                        let end = (start + size).min(total);
                        waves.push(Wave { label: format!("Batch {} of {}", k + 1, starts.len()), hosts: *start..end, concurrency: end - start });
                    }
                }
            }
        }
        if waves.len() == 1 {
            waves[0].label.clear();
        }
        waves
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
    /// Set when it's created; runbooks don't move between workspaces.
    #[serde(default = "default_workspace")]
    pub workspace: String,
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
    #[serde(default)]
    pub rollout: Rollout,
    #[serde(default)]
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub notifications: Vec<Notification>,
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
    pub rollout: Rollout,
    #[serde(default)]
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub notifications: Vec<Notification>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub targets: Targets,
    #[serde(default, deserialize_with = "seconds::option::deserialize")]
    pub timeout_seconds: Option<u64>,
}

/// Hosts a runbook runs on: specific hosts by id, every host with one of the tags,
/// and every host the filter matches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// A host filter expression, like `tags:web !name:db-*`. Never matches the server.
    #[serde(default)]
    pub filter: String,
}

impl Targets {
    pub fn is_server(&self) -> bool {
        self.hosts.is_empty() && self.tags.is_empty() && self.filter.trim().is_empty()
    }

    /// Whether an enrolled host (not the server) is a target.
    pub fn includes(&self, host: &Host) -> bool {
        self.hosts.contains(&host.id)
            || host.tags.iter().any(|t| self.tags.contains(t))
            || HostFilter::parse(&self.filter).is_ok_and(|f| f.matches(host))
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
        self.targets.filter = self.targets.filter.split_whitespace().collect::<Vec<_>>().join(" ");
        HostFilter::parse(&self.targets.filter)?;
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
        let mut secrets: Vec<String> = Vec::new();
        for reference in &self.secrets {
            let reference = reference.trim().to_string();
            secret_ref_expansions(&reference, &self.inputs)?;
            if !secrets.contains(&reference) {
                secrets.push(reference);
            }
        }
        if secrets.len() > 20 {
            return Err("A runbook can use at most 20 secrets.".into());
        }
        for (i, a) in secrets.iter().enumerate() {
            if let Some(b) = secrets[i + 1..].iter().find(|b| secret_env_name(b) == secret_env_name(a)) {
                return Err(format!("{a} and {b} would both be ${}. Use one of them, or pick the folder with a choice input.", secret_env_name(a)));
            }
        }
        self.secrets = secrets;
        self.rollout.validate()?;
        if self.schedules.len() > MAX_SCHEDULES {
            return Err(format!("A runbook can have at most {MAX_SCHEDULES} schedules."));
        }
        let defs = self.inputs.clone();
        for (i, s) in self.schedules.iter_mut().enumerate() {
            s.validate(&defs).map_err(|e| format!("Schedule {}: {e}", i + 1))?;
        }
        let mut ids: Vec<&str> = self.schedules.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != self.schedules.len() {
            return Err("Two schedules have the same id.".into());
        }
        if self.notifications.len() > MAX_NOTIFICATIONS {
            return Err(format!("A runbook can have at most {MAX_NOTIFICATIONS} notifications."));
        }
        for (i, n) in self.notifications.iter_mut().enumerate() {
            n.validate().map_err(|e| format!("Notification {}: {e}", i + 1))?;
        }
        if self.steps.is_empty() || self.steps.len() > 50 {
            return Err("A runbook needs 1 to 50 steps.".into());
        }
        for (i, step) in self.steps.iter_mut().enumerate() {
            let n = i + 1;
            if step.timeout_seconds.is_some_and(|t| !(1..=MAX_TIMEOUT).contains(&t)) {
                return Err(format!("Step {n} timeout must be 1 second to 1 day."));
            }
            if step.retries > MAX_RETRIES {
                return Err(format!("Step {n} can retry at most {MAX_RETRIES} times."));
            }
            if step.retry_delay_seconds > MAX_RETRY_DELAY {
                return Err(format!("Step {n} retry delay must be at most 1 hour."));
            }
            step.name = step.name.trim().to_string();
            if step.name.is_empty() {
                step.name = format!("Step {n}");
            }
            if step.name.chars().count() > 100 {
                return Err(format!("Step {n} name must be at most 100 characters."));
            }
            let has_command = !step.run.trim().is_empty();
            match (has_command, step.copy.as_mut(), step.call.as_mut()) {
                (true, None, None) => {
                    if step.run.len() > 20_000 {
                        return Err(format!("Step {n} command must be at most 20000 characters."));
                    }
                    template::check(&step.run, &input_names).map_err(|e| format!("Step {n}: {e}"))?;
                }
                (false, Some(copy), None) => copy.validate(&self.secrets, &input_names).map_err(|e| format!("Step {n}: {e}"))?,
                (false, None, Some(call)) => call.validate(&input_names).map_err(|e| format!("Step {n}: {e}"))?,
                (false, None, None) => return Err(format!("Step {n} needs a command.")),
                _ => return Err(format!("Step {n} must do one thing: run a command, copy a file, or call a runbook.")),
            }
        }
        match self.timeout_seconds {
            Some(t) if !(1..=MAX_TIMEOUT).contains(&t) => return Err("Step timeout must be 1 second to 1 day.".into()),
            _ => {}
        }
        Ok(self)
    }
}

/// The input a whole folder like `{{ inputs.env }}` stands for.
fn folder_input(folder: &str) -> Option<&str> {
    let inner = folder.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    let name = inner.strip_prefix("inputs.")?;
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(name)
}

/// Most secrets one reference can stand for.
const MAX_SECRET_EXPANSIONS: usize = 50;

/// Check a runbook's secret reference and list every secret it can stand for. A whole
/// folder can be a choice input, like `{{ inputs.env }}/DB_PASSWORD`; only choice inputs,
/// so whoever starts a run can pick only among secrets the runbook's editor chose.
pub fn secret_ref_expansions(reference: &str, inputs: &[InputDef]) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = reference.split('/').collect();
    let mut options: Vec<Vec<String>> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        match folder_input(part) {
            Some(name) => {
                if i + 1 == parts.len() {
                    return Err(format!("In {reference}, the secret's name can't come from an input; only a folder can."));
                }
                let def = inputs.iter().find(|d| d.name == name).ok_or_else(|| format!("{reference} uses {{{{ inputs.{name} }}}}, but the runbook has no input named {name}."))?;
                if def.kind != InputKind::Choice {
                    return Err(format!("{reference} uses {{{{ inputs.{name} }}}}, which must be a choice input, so a run can only pick among folders you list."));
                }
                options.push(def.choices.clone());
            }
            None if part.contains("{{") || part.contains("}}") => {
                return Err(format!("In {reference}, an input can only be a whole folder, like {{{{ inputs.env }}}}/DB_PASSWORD."));
            }
            None => options.push(vec![part.to_string()]),
        }
    }
    let mut out = vec![String::new()];
    for choices in options {
        let mut next = Vec::new();
        for prefix in &out {
            for c in &choices {
                next.push(if prefix.is_empty() { c.clone() } else { format!("{prefix}/{c}") });
            }
        }
        if next.len() > MAX_SECRET_EXPANSIONS {
            return Err(format!("{reference} can stand for more than {MAX_SECRET_EXPANSIONS} secrets. Use fewer choices."));
        }
        out = next;
    }
    for e in &out {
        SecretRef::parse(e).map_err(|err| if e == reference { err } else { format!("{reference} as {e}: {err}") })?;
    }
    Ok(out)
}

/// A runbook's secret reference with its inputs filled in for one run.
pub fn resolve_secret_ref(reference: &str, inputs: &[InputValue]) -> String {
    reference
        .split('/')
        .map(|part| match folder_input(part).and_then(|name| inputs.iter().find(|i| i.name == name)) {
            Some(i) => i.value.clone(),
            None => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl Runbook {
    /// Every secret this runbook can use, as references: each one its `secrets` can
    /// stand for, and those its notifications name.
    pub fn secret_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self.secrets.iter().flat_map(|r| secret_ref_expansions(r, &self.inputs).unwrap_or_default()).collect();
        out.extend(self.notifications.iter().flat_map(|n| n.secret_refs()));
        out.sort();
        out.dedup();
        out
    }
}
