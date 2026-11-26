//! Connects runs to the agents on enrolled hosts.
//!
//! Agents call the server, never the other way round: each agent long-polls for the
//! next step to run, streams its output back about once a second (learning in the
//! reply whether to cancel), and reports how the step ended.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::model::now_ms;

/// How long one poll waits for a step before answering "nothing to do".
pub const POLL_WAIT: Duration = Duration::from_secs(25);
/// A step that no agent picked up within this time fails: the host is offline.
pub const PICKUP_TIMEOUT: Duration = Duration::from_secs(45);
/// A running step fails if its agent is silent for this long.
pub const CONTACT_TIMEOUT: Duration = Duration::from_secs(30);
/// A host whose agent hasn't been in touch for this long is shown as offline.
pub const ONLINE_WINDOW_MS: u64 = 60_000;

/// One step for an agent to run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub run_id: String,
    /// Step number on this host, from 1.
    pub step: usize,
    pub command: String,
    /// Run values, inputs and vault secrets for the step, as environment variables.
    pub env: Vec<(String, String)>,
    pub timeout_seconds: u64,
    /// No later step of this run comes to this host, so its working folder can go.
    pub last: bool,
    /// If this step fails, the run still comes back to this host (a retry or a later
    /// step), so keep the working folder. Agents from before 0.4 ignore this and
    /// remove the folder after any failed step.
    #[serde(default)]
    pub keep_on_failure: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutLine {
    pub stream: String,
    pub line: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    Exited,
    TimedOut,
    Cancelled,
    /// The step never ran: it couldn't start, or the host was unreachable.
    NotRun,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobEnd {
    pub end: End,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
}

impl JobEnd {
    pub fn not_run(error: impl Into<String>) -> Self {
        JobEnd { end: End::NotRun, exit_code: None, error: Some(error.into()) }
    }
}

/// What an agent tells the server about its machine on every poll.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub version: String,
}

impl AgentInfo {
    pub fn clean(mut self) -> Self {
        let tidy = |s: &mut String| *s = s.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string();
        tidy(&mut self.hostname);
        tidy(&mut self.os);
        tidy(&mut self.arch);
        tidy(&mut self.version);
        self
    }
}

pub enum AgentMsg {
    Picked,
    /// The agent checked in about the step without new output.
    Heartbeat,
    Output(Vec<OutLine>),
    Finished(JobEnd),
}

pub struct Slot {
    host_id: String,
    tx: mpsc::UnboundedSender<AgentMsg>,
    pub cancel: AtomicBool,
}

struct Queue {
    tx: mpsc::UnboundedSender<Job>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>,
}

#[derive(Default)]
pub struct Hub {
    queues: Mutex<HashMap<String, Arc<Queue>>>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    /// Last contact per host, in ms since the epoch.
    seen: Mutex<HashMap<String, u64>>,
    /// Polls currently waiting, per host.
    polling: Mutex<HashMap<String, usize>>,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Hub::default())
    }

    fn queue(&self, host_id: &str) -> Arc<Queue> {
        self.queues
            .lock()
            .unwrap()
            .entry(host_id.to_string())
            .or_insert_with(|| {
                let (tx, rx) = mpsc::unbounded_channel();
                Arc::new(Queue { tx, rx: tokio::sync::Mutex::new(rx) })
            })
            .clone()
    }

    /// Queue a step for a host. Messages from its agent arrive on the receiver.
    pub fn dispatch(&self, host_id: &str, job: Job) -> (Arc<Slot>, mpsc::UnboundedReceiver<AgentMsg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let slot = Arc::new(Slot { host_id: host_id.to_string(), tx, cancel: AtomicBool::new(false) });
        self.slots.lock().unwrap().insert(job.id.clone(), slot.clone());
        let _ = self.queue(host_id).tx.send(job);
        (slot, rx)
    }

    /// The step is over for the server; late messages about it are ignored.
    pub fn release(&self, job_id: &str) {
        self.slots.lock().unwrap().remove(job_id);
    }

    /// Wait up to `wait` for the next step for this host.
    pub async fn next_job(&self, host_id: &str, wait: Duration) -> Option<Job> {
        self.touch(host_id);
        *self.polling.lock().unwrap().entry(host_id.to_string()).or_default() += 1;
        let queue = self.queue(host_id);
        let found = tokio::time::timeout(wait, async {
            let mut rx = queue.rx.lock().await;
            while let Some(job) = rx.recv().await {
                // Skip steps the server gave up on or cancelled before an agent took them.
                let slot = self.slots.lock().unwrap().get(&job.id).cloned();
                if let Some(slot) = slot
                    && !slot.cancel.load(Ordering::SeqCst) {
                        let _ = slot.tx.send(AgentMsg::Picked);
                        return Some(job);
                    }
            }
            None
        })
        .await
        .ok()
        .flatten();
        if let Some(n) = self.polling.lock().unwrap().get_mut(host_id) {
            *n = n.saturating_sub(1);
        }
        self.touch(host_id);
        found
    }

    fn slot_for(&self, host_id: &str, job_id: &str) -> Option<Arc<Slot>> {
        self.slots.lock().unwrap().get(job_id).filter(|s| s.host_id == host_id).cloned()
    }

    /// Output from an agent. Returns whether the step should be cancelled, or
    /// `None` if the server no longer knows the step (the agent should stop it).
    pub fn output(&self, host_id: &str, job_id: &str, lines: Vec<OutLine>) -> Option<bool> {
        self.touch(host_id);
        let slot = self.slot_for(host_id, job_id)?;
        let _ = slot.tx.send(if lines.is_empty() { AgentMsg::Heartbeat } else { AgentMsg::Output(lines) });
        Some(slot.cancel.load(Ordering::SeqCst))
    }

    pub fn finish(&self, host_id: &str, job_id: &str, end: JobEnd) -> bool {
        self.touch(host_id);
        match self.slot_for(host_id, job_id) {
            Some(slot) => slot.tx.send(AgentMsg::Finished(end)).is_ok(),
            None => false,
        }
    }

    pub fn touch(&self, host_id: &str) {
        self.seen.lock().unwrap().insert(host_id.to_string(), now_ms());
    }

    pub fn is_online(&self, host_id: &str, stored_last_seen: u64) -> bool {
        if self.polling.lock().unwrap().get(host_id).copied().unwrap_or(0) > 0 {
            return true;
        }
        let seen = self.seen.lock().unwrap().get(host_id).copied().unwrap_or(0).max(stored_last_seen);
        now_ms().saturating_sub(seen) < ONLINE_WINDOW_MS
    }

    pub fn last_seen(&self, host_id: &str) -> Option<u64> {
        self.seen.lock().unwrap().get(host_id).copied()
    }

    /// A removed host: drop its queue and stop its steps.
    pub fn forget(&self, host_id: &str) {
        self.queues.lock().unwrap().remove(host_id);
        self.seen.lock().unwrap().remove(host_id);
        for slot in self.slots.lock().unwrap().values().filter(|s| s.host_id == host_id) {
            slot.cancel.store(true, Ordering::SeqCst);
        }
    }
}
