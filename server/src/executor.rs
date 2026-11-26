//! Runs runbooks: each step is a shell command, run in order on each of the run's
//! hosts in turn. Steps for the OpSkills server run here; steps for enrolled hosts go
//! to their agents through the hub.
//!
//! A failed step can be retried, can continue on error, and otherwise either stops
//! the run or lets the remaining steps finish before the run fails, as the runbook
//! says. Commands get their `{{ ... }}` references expanded per host, and every
//! input is also an `OPSKILLS_INPUT_<NAME>` environment variable.
//!
//! Output is broadcast live to subscribers and written to the store in batches.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{Semaphore, broadcast, mpsc, watch};

use crate::hub::{AgentMsg, CONTACT_TIMEOUT, End, Hub, Job, JobEnd, PICKUP_TIMEOUT};
use crate::model::{
    CallStep, CopyStep, FailurePolicy, InputValue, LOCAL_HOST_ID, LOCAL_HOST_NAME, LimitAction, LogLine, MAX_CALL_DEPTH, Run, RunHost, Runbook, Status, now_ms, resolve_inputs,
};
use crate::store::Store;
use crate::template;
use crate::vault::{Sealed, Vault};

/// Why a run could not be started.
#[derive(Debug)]
pub enum StartError {
    /// The input values don't fit the runbook's inputs.
    BadInput(String),
    /// The runbook needs something that is gone, like a vault secret.
    Conflict(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for StartError {
    fn from(e: anyhow::Error) -> Self {
        StartError::Internal(e)
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::BadInput(m) | StartError::Conflict(m) => f.write_str(m),
            StartError::Internal(e) => write!(f, "{e}"),
        }
    }
}

/// A run's short number, as the web app shows it.
pub fn run_number(id: &str) -> String {
    let compact: String = id.chars().filter(|c| *c != '-').collect();
    compact[compact.len().saturating_sub(6)..].to_string()
}

const MAX_LINE_CHARS: usize = 4000;
const FLUSH_EVERY: Duration = Duration::from_millis(250);
const KILL_GRACE: Duration = Duration::from_secs(5);
pub const SAFE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Clone, Debug)]
pub enum Event {
    Log(LogLine),
    Run(Box<Run>),
}

/// State of a run that is queued or in progress.
struct Live {
    tx: broadcast::Sender<Event>,
    cancel: watch::Sender<bool>,
    /// Every line so far, so a new subscriber gets a gap-free backlog.
    lines: Mutex<Vec<LogLine>>,
}

/// Secret values a run needs, as environment variables. Held only in memory.
pub type SecretEnv = Vec<(String, String)>;

/// Everything a run needs besides what its record holds. Held only in memory.
#[derive(Default)]
pub struct RunValues {
    /// Vault secrets, as environment variables.
    pub secrets: SecretEnv,
    pub inputs: Vec<InputValue>,
}

impl RunValues {
    /// Values to hide in output: vault secrets and secret inputs.
    fn hidden(&self) -> Vec<String> {
        self.secrets.iter().map(|(_, v)| v.clone()).chain(self.inputs.iter().filter(|i| i.secret).map(|i| i.value.clone())).collect()
    }

    /// Environment a step on `host` gets besides the base environment.
    fn env(&self, run: &Run, host: &RunHost) -> Vec<(String, String)> {
        let mut env = vec![
            ("OPSKILLS_RUNBOOK_ID".to_string(), run.runbook_id.clone()),
            ("OPSKILLS_RUNBOOK_NAME".to_string(), run.runbook_name.clone()),
            ("OPSKILLS_STARTED_BY".to_string(), run.started_by.clone()),
            ("OPSKILLS_HOST_ID".to_string(), host.id.clone()),
            ("OPSKILLS_HOST_NAME".to_string(), host.name.clone()),
        ];
        env.extend(self.inputs.iter().map(|i| (i.env_name(), i.value.clone())));
        env.extend(self.secrets.iter().cloned());
        env
    }

    /// What `{{ scope.name }}` references expand to on `host`.
    fn template_values(&self, run: &Run, host: &RunHost) -> BTreeMap<String, String> {
        let mut v: BTreeMap<String, String> = [
            ("run.id", run.id.as_str()),
            ("run.started_by", run.started_by.as_str()),
            ("runbook.id", run.runbook_id.as_str()),
            ("runbook.name", run.runbook_name.as_str()),
            ("host.id", host.id.as_str()),
            ("host.name", host.name.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        v.extend(self.inputs.iter().map(|i| (format!("inputs.{}", i.name), i.value.clone())));
        v
    }
}

/// What ended a run early, so every later step is skipped.
#[derive(Clone, Copy, PartialEq)]
enum Halt {
    Failed,
    Cancelled,
}

pub struct Executor {
    store: Arc<Store>,
    hub: Arc<Hub>,
    vault: Arc<Vault>,
    work_root: PathBuf,
    limit: Arc<Semaphore>,
    live: Mutex<HashMap<String, Arc<Live>>>,
}

impl Executor {
    pub fn new(store: Arc<Store>, hub: Arc<Hub>, vault: Arc<Vault>, work_root: PathBuf, max_concurrent: usize) -> Arc<Self> {
        Arc::new(Executor { store, hub, vault, work_root, limit: Arc::new(Semaphore::new(max_concurrent.max(1))), live: Mutex::new(HashMap::new()) })
    }

    /// Check input values against `rb` and decrypt the secrets it lists. The result
    /// lives only in memory, for one run.
    pub fn prepare(&self, rb: &Runbook, given: &serde_json::Map<String, serde_json::Value>) -> Result<RunValues, StartError> {
        let inputs = resolve_inputs(&rb.inputs, given).map_err(StartError::BadInput)?;
        let mut secrets = Vec::new();
        for name in &rb.secrets {
            let stored = self.store.secret(name)?.ok_or_else(|| {
                StartError::Conflict(format!("The runbook \"{}\" uses the secret {name}, which is no longer in the vault. Add it again or edit the runbook.", rb.name))
            })?;
            let value = self.vault.open(name, &Sealed { nonce: stored.nonce, ciphertext: stored.ciphertext })?;
            secrets.push((name.clone(), String::from_utf8_lossy(&value).into_owned()));
        }
        Ok(RunValues { secrets, inputs })
    }

    /// Subscribe to a live run. Returns the lines so far and a receiver for what follows,
    /// with no gap or overlap between them. `None` if the run is not live.
    pub fn subscribe(&self, run_id: &str) -> Option<(Vec<LogLine>, broadcast::Receiver<Event>)> {
        let live = self.live.lock().unwrap().get(run_id).cloned()?;
        let lines = live.lines.lock().unwrap();
        Some((lines.clone(), live.tx.subscribe()))
    }

    pub fn cancel(&self, run_id: &str) -> bool {
        match self.live.lock().unwrap().get(run_id) {
            Some(live) => {
                let _ = live.cancel.send(true);
                true
            }
            None => false,
        }
    }

    /// Persist the queued run and start it in the background.
    pub fn start(self: &Arc<Self>, run: Run, values: RunValues) -> anyhow::Result<Run> {
        self.store.put_run(&run)?;
        let (tx, _) = broadcast::channel(4096);
        let (cancel, cancel_rx) = watch::channel(false);
        let live = Arc::new(Live { tx, cancel, lines: Mutex::new(Vec::new()) });
        self.live.lock().unwrap().insert(run.id.clone(), live.clone());
        let me = self.clone();
        let snapshot = run.clone();
        tokio::spawn(async move {
            let id = run.id.clone();
            me.clone().execute(run, values, live, cancel_rx).await;
            me.live.lock().unwrap().remove(&id);
        });
        Ok(snapshot)
    }

    fn publish(&self, live: &Live, run: &Run) {
        if let Err(err) = self.store.put_run(run) {
            tracing::error!(run_id = %run.id, error = %err, "saving run failed");
        }
        let _ = live.tx.send(Event::Run(Box::new(run.clone())));
    }

    async fn execute(self: Arc<Self>, mut run: Run, values: RunValues, live: Arc<Live>, mut cancel_rx: watch::Receiver<bool>) {
        // A called run is part of its parent, which already holds a slot; waiting for
        // another could deadlock when every slot is taken by a parent.
        let nested = run.parent.is_some();
        let permit = if nested {
            None
        } else {
            tokio::select! {
                p = self.limit.clone().acquire_owned() => p.ok(),
                _ = wait_cancel(&mut cancel_rx) => None,
            }
        };
        let limit = run.output_limit;
        let mut sink = Sink::new(&run.id, &live, &self.store, &values.hidden(), limit.lines, limit.action == LimitAction::Fail);
        let first_host = run.hosts.first().map(|h| h.name.clone()).unwrap_or_else(|| LOCAL_HOST_NAME.into());
        if (!nested && permit.is_none()) || (nested && *cancel_rx.borrow()) {
            run.status = Status::Cancelled;
            for s in &mut run.steps {
                s.status = Status::Skipped;
            }
            run.finished_at = Some(now_ms());
            sink.system(0, &first_host, "Run cancelled before it started.");
            sink.flush();
            run.log_lines = sink.count;
            self.publish(&live, &run);
            return;
        }

        run.status = Status::Running;
        run.started_at = Some(now_ms());
        self.publish(&live, &run);
        tracing::info!(run_id = %run.id, runbook = %run.runbook_name, "run started");

        let hosts = if run.hosts.is_empty() { vec![RunHost { id: LOCAL_HOST_ID.into(), name: LOCAL_HOST_NAME.into() }] } else { run.hosts.clone() };
        let per_host = run.steps.len() / hosts.len();
        let waves = run.rollout.waves(hosts.len());
        let pause = run.rollout.pause_seconds;
        let shared = Shared {
            stop_on_host_failure: run.rollout.stops_on_host_failure(run.on_step_failure),
            run: Mutex::new(run),
            sink: Mutex::new(sink),
            state: Mutex::new(RunState::default()),
            live: &live,
            values: &values,
            hosts: &hosts,
            per_host,
        };
        for (w, wave) in waves.iter().enumerate() {
            let first = wave.hosts.start;
            let (step0, name0) = (first * per_host, hosts[first].name.as_str());
            if w > 0 && pause > 0 && shared.may_start_hosts() {
                shared.system(step0, name0, &format!("Waiting {} before the next hosts.", human_duration(pause)));
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(pause)) => {}
                    _ = wait_cancel(&mut cancel_rx) => {}
                }
            }
            if *cancel_rx.borrow() {
                shared.state.lock().unwrap().halt.get_or_insert(Halt::Cancelled);
            }
            if !wave.label.is_empty() && shared.may_start_hosts() {
                let names: Vec<&str> = hosts[wave.hosts.clone()].iter().map(|h| h.name.as_str()).collect();
                shared.system(step0, name0, &format!("{}: {}", wave.label, names.join(", ")));
            }
            // Hosts of a wave run concurrently, at most `concurrency` at a time, all in this task.
            stream::iter(wave.hosts.clone())
                .map(|h| self.run_host(&shared, h, cancel_rx.clone()))
                .buffer_unordered(wave.concurrency.max(1))
                .collect::<Vec<()>>()
                .await;
        }

        let Shared { run, sink, state, .. } = shared;
        let (mut run, mut sink, state) = (run.into_inner().unwrap(), sink.into_inner().unwrap(), state.into_inner().unwrap());
        for s in &mut run.steps {
            if s.status == Status::Pending {
                s.status = Status::Skipped;
            }
        }
        run.status = match state.halt {
            Some(Halt::Cancelled) => Status::Cancelled,
            _ if state.failed => Status::Failed,
            _ => Status::Succeeded,
        };
        run.finished_at = Some(now_ms());
        sink.flush();
        run.log_lines = sink.count;
        self.publish(&live, &run);
        tracing::info!(run_id = %run.id, status = ?run.status, "run finished");
        drop(permit);
    }

    /// Run every step of host `h`, one after another, unless the run already stopped
    /// starting hosts. Updates the shared run as it goes.
    async fn run_host(self: &Arc<Self>, sh: &Shared<'_>, h: usize, mut cancel_rx: watch::Receiver<bool>) {
        let host = &sh.hosts[h];
        let range = h * sh.per_host..(h + 1) * sh.per_host;
        if *cancel_rx.borrow() {
            sh.state.lock().unwrap().halt.get_or_insert(Halt::Cancelled);
        }
        if !sh.may_start_hosts() {
            sh.update(self, |r| range.clone().for_each(|i| r.steps[i].status = Status::Skipped));
            return;
        }
        let (run_id, run_timeout, finish_anyway, limit_lines, env, template_values) = {
            let r = sh.run.lock().unwrap();
            (r.id.clone(), r.timeout_seconds, r.on_step_failure == FailurePolicy::FinishThenFail, r.output_limit.lines, sh.values.env(&r, host), sh.values.template_values(&r, host))
        };
        let name = host.name.as_str();
        let local = host.id == LOCAL_HOST_ID;
        let workdir = self.work_root.join(&run_id);
        // A step failed that doesn't continue on error: this host fails.
        let mut host_failed = false;
        // This host's later steps are skipped.
        let mut host_done = false;
        if local
            && let Err(err) = std::fs::create_dir_all(&workdir) {
                sh.system(range.start, name, &format!("Could not create the working directory: {err}"));
                host_failed = true;
                host_done = true;
            }
        for i in range.clone() {
            if host_done || sh.state.lock().unwrap().halt.is_some() {
                sh.update(self, |r| r.steps[i].status = Status::Skipped);
                continue;
            }
            let number = i - range.start + 1;
            let last_on_host = i + 1 == range.end;
            let step = sh.run.lock().unwrap().steps[i].clone();
            sh.update(self, |r| {
                r.steps[i].status = Status::Running;
                r.steps[i].started_at = Some(now_ms());
            });
            if sh.hosts.len() > 1 && number == 1 {
                sh.system(i, name, &format!("Host {} of {}: {}", h + 1, sh.hosts.len(), name));
            }
            sh.system(i, name, &format!("Step {number}: {}", step.name));

            // What the step runs: its command, or the script that writes a copied file.
            let prepared = match &step.copy {
                Some(copy) => copy_script(copy, &template_values, sh.values).map(|(script, extra)| {
                    let mut e = env.clone();
                    e.push(extra);
                    (script, e)
                }),
                None => Ok((template::expand(&step.run, &template_values), env.clone())),
            };
            let timeout = Duration::from_secs(if step.timeout_seconds > 0 { step.timeout_seconds } else { run_timeout }.max(1));
            let tries = step.retries + 1;
            // Later steps on this host may still run after this one fails, so its files must stay.
            let more_after_failure = !last_on_host && (step.continue_on_error || finish_anyway);
            let mut attempt = 0;
            let result = loop {
                attempt += 1;
                if attempt > 1 {
                    sh.system(i, name, &format!("Attempt {attempt} of {tries}."));
                }
                sh.update(self, |r| r.steps[i].attempts = attempt);
                let result = if let Some(call) = &step.call {
                    let parent = sh.run.lock().unwrap().clone();
                    match self.start_call(&parent, i, host, call, &template_values) {
                        Ok(child) => {
                            sh.system(i, name, &format!("Running \"{}\" as run #{} on {}.", child.runbook_name, run_number(&child.id), name));
                            sh.update(self, |r| r.steps[i].child_run_id = Some(child.id.clone()));
                            self.wait_child(&child.id, timeout, &mut cancel_rx).await
                        }
                        Err(msg) => JobEnd::not_run(msg),
                    }
                } else {
                    match &prepared {
                        Err(msg) => JobEnd::not_run(msg.clone()),
                        Ok((command, step_env)) if local => {
                            let mut full_env = base_env(&workdir, &run_id, number);
                            full_env.extend(step_env.iter().cloned());
                            run_command(command, &workdir, &full_env, timeout, &mut cancel_rx, |stream, line| sh.sink.lock().unwrap().push(i, name, stream, &line)).await
                        }
                        Ok((command, step_env)) => {
                            let job = Job {
                                id: uuid::Uuid::now_v7().to_string(),
                                run_id: run_id.clone(),
                                step: number,
                                command: command.clone(),
                                env: step_env.clone(),
                                timeout_seconds: timeout.as_secs(),
                                last: last_on_host,
                                keep_on_failure: attempt < tries || more_after_failure,
                            };
                            self.run_remote(&host.id, name, job, timeout, &mut cancel_rx, &sh.sink, i).await
                        }
                    }
                };
                let succeeded = result.end == End::Exited && result.exit_code == Some(0);
                if succeeded || result.end == End::Cancelled || attempt >= tries {
                    break result;
                }
                sh.system(i, name, &failure_message(&result, timeout));
                let delay = Duration::from_secs(step.retry_delay_seconds);
                if !delay.is_zero() {
                    sh.system(i, name, &format!("Trying again in {}.", human_duration(step.retry_delay_seconds)));
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = wait_cancel(&mut cancel_rx) => break JobEnd { end: End::Cancelled, exit_code: None, error: None },
                    }
                }
            };

            let over_limit = sh.sink.lock().unwrap().limit_hit;
            let status = match result.end {
                End::Exited if result.exit_code == Some(0) => Status::Succeeded,
                End::Cancelled if !over_limit => Status::Cancelled,
                _ => Status::Failed,
            };
            sh.update(self, |r| {
                let s = &mut r.steps[i];
                s.finished_at = Some(now_ms());
                s.exit_code = result.exit_code;
                s.status = status;
            });
            if status == Status::Succeeded {
                continue;
            }
            if over_limit {
                sh.system(i, name, &format!("The run reached its output limit of {limit_lines} lines and was stopped."));
                let mut s = sh.state.lock().unwrap();
                s.failed = true;
                s.halt.get_or_insert(Halt::Failed);
            } else if result.end == End::Cancelled {
                sh.system(i, name, "Run cancelled.");
                sh.state.lock().unwrap().halt.get_or_insert(Halt::Cancelled);
            } else {
                sh.system(i, name, &failure_message(&result, timeout));
                if step.continue_on_error {
                    sh.system(i, name, "This step continues on error, so the run goes on.");
                } else {
                    host_failed = true;
                    if finish_anyway {
                        sh.system(i, name, "The remaining steps on this host still run; the run will end as failed.");
                    } else {
                        host_done = true;
                    }
                }
            }
        }
        if local {
            let _ = std::fs::remove_dir_all(&workdir);
        }
        if host_failed {
            let stop = sh.stop_on_host_failure;
            {
                let mut s = sh.state.lock().unwrap();
                s.failed = true;
                if stop {
                    s.stop_new_hosts = true;
                }
            }
            let waiting = sh.run.lock().unwrap().steps.iter().any(|s| s.status == Status::Pending);
            if stop && waiting {
                sh.system(range.end - 1, name, &format!("{name} failed, so hosts that haven't started are skipped."));
            }
        }
    }
}

/// What every host of a run shares while it runs. Locks are held only briefly and
/// never across an await.
struct Shared<'a> {
    run: Mutex<Run>,
    sink: Mutex<Sink<'a>>,
    state: Mutex<RunState>,
    live: &'a Live,
    values: &'a RunValues,
    hosts: &'a [RunHost],
    per_host: usize,
    stop_on_host_failure: bool,
}

#[derive(Default)]
struct RunState {
    /// A host failed: the run ends as failed.
    failed: bool,
    /// Cancelled, or stopped by the output limit: nothing more runs.
    halt: Option<Halt>,
    /// A host failed and the rollout stops on host failure: no new host starts.
    stop_new_hosts: bool,
}

impl Shared<'_> {
    fn may_start_hosts(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.halt.is_none() && !s.stop_new_hosts
    }

    fn system(&self, step: usize, host: &str, text: &str) {
        self.sink.lock().unwrap().system(step, host, text);
    }

    /// Change the run, then save and broadcast it.
    fn update(&self, ex: &Executor, change: impl FnOnce(&mut Run)) {
        let count = self.sink.lock().unwrap().count;
        let mut run = self.run.lock().unwrap();
        change(&mut run);
        run.log_lines = count;
        ex.publish(self.live, &run);
    }
}

impl Executor {
    /// Start the run a call step asks for, on the calling step's host.
    fn start_call(self: &Arc<Self>, parent: &Run, step: usize, host: &RunHost, call: &CallStep, values: &BTreeMap<String, String>) -> Result<Run, String> {
        if parent.depth + 1 > MAX_CALL_DEPTH {
            return Err(format!("Calls can nest at most {MAX_CALL_DEPTH} deep."));
        }
        let rb = self
            .store
            .runbook(&call.runbook)
            .map_err(|e| e.to_string())?
            .ok_or("The called runbook no longer exists. Edit this runbook to call another one.")?;
        let given: serde_json::Map<String, serde_json::Value> =
            call.inputs.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(template::expand_plain(v, values)))).collect();
        let run_values = self.prepare(&rb, &given).map_err(|e| format!("Could not start \"{}\": {e}", rb.name))?;
        let child = Run::called(&rb, parent, step, host.clone(), &run_values.inputs);
        self.start(child, run_values).map_err(|e| e.to_string())
    }

    /// Wait for a called run to end, passing on a cancel and enforcing the step's timeout.
    async fn wait_child(&self, child_id: &str, timeout: Duration, cancel_rx: &mut watch::Receiver<bool>) -> JobEnd {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        let mut stopped: Option<End> = None;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let child = match self.store.run(child_id) {
                        Ok(Some(r)) => r,
                        Ok(None) => return JobEnd::not_run("The called run disappeared."),
                        Err(e) => return JobEnd::not_run(e.to_string()),
                    };
                    if !child.status.is_finished() {
                        continue;
                    }
                    let n = run_number(child_id);
                    return match (stopped, child.status) {
                        (Some(end), _) => JobEnd { end, exit_code: None, error: None },
                        (None, Status::Succeeded) => JobEnd { end: End::Exited, exit_code: Some(0), error: None },
                        (None, Status::Cancelled) => JobEnd::not_run(format!("The called run #{n} was cancelled.")),
                        (None, _) => JobEnd::not_run(format!("The called run #{n} failed.")),
                    };
                }
                _ = wait_cancel(cancel_rx), if stopped.is_none() => {
                    self.cancel(child_id);
                    stopped = Some(End::Cancelled);
                }
                _ = &mut deadline, if stopped.is_none() => {
                    self.cancel(child_id);
                    stopped = Some(End::TimedOut);
                }
            }
        }
    }

    /// Hand a step to the agent on `host_id` and follow it until it ends.
    #[allow(clippy::too_many_arguments)]
    async fn run_remote(&self, host_id: &str, host_name: &str, job: Job, timeout: Duration, cancel_rx: &mut watch::Receiver<bool>, sink: &Mutex<Sink<'_>>, step: usize) -> JobEnd {
        let job_id = job.id.clone();
        let (slot, mut rx) = self.hub.dispatch(host_id, job);
        let queued = Instant::now();
        // Picked: a poll took the step. Acked: the agent has since been in touch about it,
        // so it really runs (a poll from an agent that just died can take a step and lose it).
        let mut picked = false;
        let mut acked: Option<Instant> = None;
        let mut contact = Instant::now();
        let mut cancelled_at: Option<Instant> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        let end = loop {
            tokio::select! {
                // Messages first, so a cancel never overtakes the news that the step was picked up.
                biased;
                msg = rx.recv() => match msg {
                    Some(AgentMsg::Picked) => picked = true,
                    Some(AgentMsg::Output(lines)) => {
                        contact = Instant::now();
                        acked.get_or_insert(contact);
                        for l in lines {
                            let stream = if l.stream == "stderr" { "stderr" } else { "stdout" };
                            sink.lock().unwrap().push(step, host_name, stream, &l.line);
                        }
                    }
                    Some(AgentMsg::Finished(end)) => break end,
                    Some(AgentMsg::Heartbeat) => {
                        contact = Instant::now();
                        acked.get_or_insert(contact);
                    }
                    None => break JobEnd::not_run("Lost contact with the agent on this host."),
                },
                _ = wait_cancel(cancel_rx), if cancelled_at.is_none() => {
                    slot.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                    if !picked {
                        break JobEnd { end: End::Cancelled, exit_code: None, error: None };
                    }
                    cancelled_at = Some(Instant::now());
                }
                _ = tick.tick() => {
                    let now = Instant::now();
                    match acked {
                        None if now - queued > PICKUP_TIMEOUT => break JobEnd::not_run(
                            "The host didn't pick up the step. Check that it is online and that the opskills-agent service is running on it.",
                        ),
                        Some(_) if now - contact > CONTACT_TIMEOUT => break JobEnd::not_run("Lost contact with the agent on this host."),
                        Some(p) if now - p > timeout + Duration::from_secs(30) => break JobEnd { end: End::TimedOut, exit_code: None, error: None },
                        _ => {}
                    }
                    if cancelled_at.is_some_and(|c| now - c > Duration::from_secs(20)) {
                        break JobEnd { end: End::Cancelled, exit_code: None, error: None };
                    }
                }
            }
        };
        slot.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        self.hub.release(&job_id);
        end
    }
}

/// The shell script for a copy step, and the environment variable holding the file's
/// content. The content travels in the environment, not the command line, so other
/// users on the host can't read it from the process list. The file is written next
/// to its destination and moved into place, so it is never seen half written.
fn copy_script(copy: &CopyStep, values: &BTreeMap<String, String>, run_values: &RunValues) -> Result<(String, (String, String)), String> {
    let content = match (&copy.content, &copy.secret) {
        (Some(c), _) => template::expand_plain(c, values),
        (None, Some(name)) => run_values
            .secrets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("The secret {name} is not available to this run."))?,
        (None, None) => return Err("This copy step has nothing to copy.".into()),
    };
    let to = template::expand_plain(&copy.to, values);
    if to.contains(['\0', '\n', '\r']) {
        return Err("The destination path can't contain line breaks.".into());
    }
    let mode = copy.mode_or_default();
    let script = format!(
        r#"set -e
dest={dest}
dir=$(dirname -- "$dest")
mkdir -p -- "$dir"
tmp=$(mktemp "$dir/.opskills-copy.XXXXXX")
trap 'rm -f -- "$tmp"' EXIT
printf '%s' "$OPSKILLS_COPY_CONTENT" > "$tmp"
chmod {mode} "$tmp"
mv -f -- "$tmp" "$dest"
trap - EXIT
echo "Wrote {bytes} bytes to $dest (mode {mode})""#,
        dest = template::shell_quote(&to),
        bytes = content.len(),
    );
    Ok((script, ("OPSKILLS_COPY_CONTENT".into(), content)))
}

/// Why a step attempt didn't succeed, for the run log.
fn failure_message(result: &JobEnd, timeout: Duration) -> String {
    match result.end {
        End::Exited => format!("Step failed with exit code {}.", result.exit_code.unwrap_or(-1)),
        End::TimedOut => format!("Step timed out after {} and was stopped.", human_duration(timeout.as_secs())),
        End::Cancelled => "Step cancelled.".into(),
        End::NotRun => result.error.clone().unwrap_or_else(|| "The step could not start.".into()),
    }
}

/// "45 seconds", "1 minute", "1 hour 30 minutes".
pub fn human_duration(secs: u64) -> String {
    let unit = |n: u64, word: &str| if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") };
    let parts: Vec<String> = [(secs / 86_400, "day"), (secs % 86_400 / 3600, "hour"), (secs % 3600 / 60, "minute"), (secs % 60, "second")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| unit(n, w))
        .collect();
    if parts.is_empty() { "0 seconds".into() } else { parts.join(" ") }
}

/// Environment every step starts from, before secrets are added.
pub fn base_env(workdir: &std::path::Path, run_id: &str, step: usize) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), SAFE_PATH.into()),
        ("HOME".into(), workdir.display().to_string()),
        ("LANG".into(), "C.UTF-8".into()),
        ("OPSKILLS_RUN_ID".into(), run_id.into()),
        ("OPSKILLS_STEP".into(), step.to_string()),
    ]
}

pub async fn wait_cancel(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Collects output lines: broadcasts each immediately, stores them in batches.
struct Sink<'a> {
    run_id: String,
    live: &'a Live,
    store: &'a Store,
    pending: Vec<LogLine>,
    count: u64,
    last_flush: Instant,
    truncated: bool,
    /// Secret values to hide in output, longest first.
    masks: Vec<String>,
    /// Lines the run may keep, system lines included.
    limit: u64,
    /// Reaching the limit stops the run instead of dropping output.
    fail_on_limit: bool,
    /// The limit was reached and the run is being stopped for it.
    limit_hit: bool,
}

pub use crate::model::MASK;

/// Every secret value, and each line of a multi-line value, at least 4 characters long.
fn mask_list(secrets: &[String]) -> Vec<String> {
    let mut masks: Vec<String> = Vec::new();
    for value in secrets {
        let candidates = std::iter::once(value.as_str()).chain(value.lines().map(str::trim));
        for c in candidates {
            if c.chars().count() >= 4 && !masks.iter().any(|m| m == c) {
                masks.push(c.to_string());
            }
        }
    }
    masks.sort_by_key(|m| std::cmp::Reverse(m.len()));
    masks
}

impl<'a> Sink<'a> {
    fn new(run_id: &str, live: &'a Live, store: &'a Store, hidden: &[String], limit: u64, fail_on_limit: bool) -> Self {
        Sink {
            run_id: run_id.to_string(),
            live,
            store,
            pending: Vec::new(),
            count: 0,
            last_flush: Instant::now(),
            truncated: false,
            masks: mask_list(hidden),
            limit,
            fail_on_limit,
            limit_hit: false,
        }
    }

    fn mask(&self, text: &str) -> String {
        let mut out = text.to_string();
        for m in &self.masks {
            if out.contains(m.as_str()) {
                out = out.replace(m.as_str(), MASK);
            }
        }
        out
    }

    /// One output line from the step at index `step`, on the host named `host`.
    fn push(&mut self, step: usize, host: &str, stream: &str, text: &str) {
        if self.count >= self.limit {
            if !self.truncated {
                self.truncated = true;
                if self.fail_on_limit {
                    self.limit_hit = true;
                    // Stops the step the same way a cancel does; the executor reports it as a failure.
                    let _ = self.live.cancel.send(true);
                } else {
                    self.emit(step, host, "system", &format!("Output limit of {} lines reached; further output is not kept.", self.limit));
                }
            }
            return;
        }
        let masked = self.mask(text);
        let text: String = masked.chars().take(MAX_LINE_CHARS).collect();
        self.emit(step, host, stream, &text);
    }

    fn system(&mut self, step: usize, host: &str, text: &str) {
        self.emit(step, host, "system", text);
    }

    fn emit(&mut self, step: usize, host: &str, stream: &str, text: &str) {
        self.count += 1;
        let line = LogLine { seq: self.count, ts: now_ms(), step, host: host.into(), stream: stream.into(), line: text.into() };
        {
            // Hold the backlog lock while broadcasting so subscribers never miss or repeat a line.
            let mut lines = self.live.lines.lock().unwrap();
            lines.push(line.clone());
            let _ = self.live.tx.send(Event::Log(line.clone()));
        }
        self.pending.push(line);
        if self.pending.len() >= 200 || self.last_flush.elapsed() >= FLUSH_EVERY {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if let Err(err) = self.store.append_logs(&self.run_id, &self.pending) {
            tracing::error!(run_id = %self.run_id, error = %err, "saving output failed");
        }
        self.pending.clear();
        self.last_flush = Instant::now();
    }
}

/// Run one shell command to the end, a timeout or a cancel. Each output line goes to
/// `emit` as it is printed. Used for steps on the server and by the agent on hosts.
pub async fn run_command(
    command: &str,
    workdir: &std::path::Path,
    env: &[(String, String)],
    timeout: Duration,
    cancel_rx: &mut watch::Receiver<bool>,
    mut emit: impl FnMut(&'static str, String),
) -> JobEnd {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(workdir)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => return JobEnd::not_run(format!("Could not start the step: {err}")),
    };
    let pgid = child.id().map(|p| p as i32);
    let (tx, mut rx) = mpsc::channel::<(&'static str, String)>(1024);
    let readers = [
        child.stdout.take().map(|o| tokio::spawn(read_lines(o, "stdout", tx.clone()))),
        child.stderr.take().map(|e| tokio::spawn(read_lines(e, "stderr", tx.clone()))),
    ];
    drop(tx);

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut kill_at: Option<tokio::time::Instant> = None;
    let mut end = End::Exited;
    let status = loop {
        let kill_timer = async {
            match kill_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            Some((stream, line)) = rx.recv() => emit(stream, line),
            status = child.wait() => break status.ok(),
            _ = &mut deadline, if kill_at.is_none() => {
                end = End::TimedOut;
                signal_group(pgid, libc::SIGTERM);
                kill_at = Some(tokio::time::Instant::now() + KILL_GRACE);
            }
            _ = wait_cancel(cancel_rx), if kill_at.is_none() => {
                end = End::Cancelled;
                signal_group(pgid, libc::SIGTERM);
                kill_at = Some(tokio::time::Instant::now() + KILL_GRACE);
            }
            _ = kill_timer => {
                signal_group(pgid, libc::SIGKILL);
                kill_at = Some(tokio::time::Instant::now() + Duration::from_secs(3600));
            }
        }
    };
    // Stop anything the step left running in its process group, then drain output.
    signal_group(pgid, libc::SIGKILL);
    let drain = async {
        while let Some((stream, line)) = rx.recv().await {
            emit(stream, line);
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
    for r in readers.into_iter().flatten() {
        r.abort();
    }

    let exit_code = status.map(|s| s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)));
    JobEnd { end, exit_code, error: None }
}

fn signal_group(pgid: Option<i32>, signal: i32) {
    if let Some(pgid) = pgid {
        // SAFETY: plain syscall on a process group this server created.
        unsafe {
            libc::killpg(pgid, signal);
        }
    }
}

async fn read_lines(reader: impl AsyncRead + Unpin, stream: &'static str, tx: mpsc::Sender<(&'static str, String)>) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
                if tx.send((stream, text)).await.is_err() {
                    break;
                }
            }
        }
    }
}
