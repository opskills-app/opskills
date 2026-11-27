//! Runs, their steps and hosts, and their output lines.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};


use super::*;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallStep>,
    /// The run a call step started (its latest attempt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run_id: Option<String>,
}

/// The run and step that started a run through a call step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParentRef {
    pub run_id: String,
    pub runbook_name: String,
    /// Index of the calling step in the parent run's steps.
    pub step: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    /// The runbook's workspace when the run started.
    #[serde(default = "default_workspace")]
    pub workspace: String,
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
    #[serde(default)]
    pub rollout: Rollout,
    /// Hosts in the order they run.
    #[serde(default)]
    pub hosts: Vec<RunHost>,
    pub steps: Vec<StepResult>,
    pub error: Option<String>,
    pub log_lines: u64,
    /// Set when a call step in another run started this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRef>,
    /// 0 for a run started directly, 1 for a run it called, and so on.
    #[serde(default)]
    pub depth: u32,
    /// The run whose unfinished hosts this run retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
}

impl Run {
    /// Hosts of this run that didn't finish well: a step failed (and doesn't continue
    /// on error), or was skipped, cancelled or never ran. In run order.
    pub fn unfinished_hosts(&self) -> Vec<RunHost> {
        let ok = |s: &StepResult| s.status == Status::Succeeded || (s.status == Status::Failed && s.continue_on_error);
        let mut out: Vec<RunHost> = Vec::new();
        for s in &self.steps {
            if !ok(s) && !out.iter().any(|h| h.id == s.host_id) {
                out.push(RunHost { id: s.host_id.clone(), name: s.host_name.clone() });
            }
        }
        out
    }
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
            rollout: runbook.rollout,
            id: uuid::Uuid::now_v7().to_string(),
            workspace: runbook.workspace.clone(),
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
                        copy: s.copy.clone(),
                        call: s.call.clone(),
                        child_run_id: None,
                    })
                })
                .collect(),
            hosts,
            error: None,
            log_lines: 0,
            parent: None,
            depth: 0,
            retry_of: None,
        }
    }

    /// A run of `runbook` started by step `step` of `parent`, on that step's host.
    pub fn called(runbook: &Runbook, parent: &Run, step: usize, host: RunHost, inputs: &[InputValue]) -> Self {
        let mut run = Run::new(runbook, &parent.started_by, vec![host], inputs);
        run.trigger = "runbook".into();
        run.parent = Some(ParentRef { run_id: parent.id.clone(), runbook_name: parent.runbook_name.clone(), step });
        run.depth = parent.depth + 1;
        run
    }
}

/// Ids of the runbooks each runbook calls, for finding loops.
pub fn calls_of(rb_steps: &[Step]) -> Vec<String> {
    rb_steps.iter().filter_map(|s| s.call.as_ref().map(|c| c.runbook.clone())).collect()
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
