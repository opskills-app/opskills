//! The OpSkills agent: runs on an enrolled host, takes steps from the server and runs them.
//!
//! The same binary as the server, installed as `opskills-agent`. It only makes outgoing
//! HTTPS requests to the server, so hosts need no open ports and no SSH keys.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::executor::{base_env, run_command};
use crate::hub::{AgentInfo, End, Job, JobEnd, OutLine};

const DEFAULT_CONFIG: &str = "/etc/opskills-agent/agent.toml";
const DEFAULT_WORK_DIR: &str = "/var/lib/opskills-agent/work";
/// Exit code for "this host was removed from OpSkills": the service doesn't restart.
const EXIT_REVOKED: u8 = 78;

#[derive(Parser)]
#[command(name = "opskills-agent", version, about = "Runs OpSkills runbook steps on this host")]
pub struct AgentCli {
    /// Path to the agent config file
    #[arg(long, global = true, env = "OPSKILLS_AGENT_CONFIG", default_value = DEFAULT_CONFIG)]
    config: PathBuf,

    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Connect this host to an OpSkills server, using a token from the Hosts page
    Enroll {
        /// Address of the OpSkills server, for example https://opskills.example.com
        #[arg(long)]
        server: String,
        /// Enrollment token from the Hosts page (each token works once)
        #[arg(long, env = "OPSKILLS_ENROLL_TOKEN", hide_env_values = true)]
        token: String,
        /// Name shown in OpSkills (default: this machine's hostname)
        #[arg(long)]
        name: Option<String>,
        /// Tags for targeting, comma separated, for example web,prod
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Replace an existing enrollment
        #[arg(long)]
        force: bool,
    },
    /// Run the agent in the foreground (what the opskills-agent service runs)
    Run,
    /// Show which server this host is enrolled with
    Status,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentConfig {
    server: String,
    host_id: String,
    secret: String,
    #[serde(default = "default_work_dir")]
    work_dir: PathBuf,
}

fn default_work_dir() -> PathBuf {
    PathBuf::from(DEFAULT_WORK_DIR)
}

pub fn main() -> ExitCode {
    let cli = AgentCli::parse();
    let result = match cli.command {
        AgentCommand::Enroll { server, token, name, tags, force } => enroll(&cli.config, &server, &token, name, tags, force),
        AgentCommand::Status => status(&cli.config),
        AgentCommand::Run => return run(&cli.config),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("opskills-agent: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Facts about this machine shown on the Hosts page.
pub fn machine_info() -> AgentInfo {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its full length; the result is NUL-terminated or truncated.
    let hostname = if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        "unknown".into()
    };
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())))
        .unwrap_or_else(|| std::env::consts::OS.into());
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    AgentInfo { hostname, os, arch: arch.into(), version: crate::VERSION.into() }.clean()
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(45)))
        .http_status_as_error(false)
        .user_agent(format!("opskills-agent/{}", crate::VERSION))
        .build()
        .into()
}

fn normalize_server(server: &str) -> Result<String> {
    let s = server.trim().trim_end_matches('/');
    if !(s.starts_with("https://") || s.starts_with("http://")) {
        bail!("--server must start with https:// (or http:// on a trusted network), for example https://opskills.example.com");
    }
    Ok(s.to_string())
}

/// The `detail` of an API error response, or the status line.
fn problem(res: &mut ureq::http::Response<ureq::Body>) -> String {
    let status = res.status();
    res.body_mut()
        .read_json::<serde_json::Value>()
        .ok()
        .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(String::from))
        .unwrap_or_else(|| format!("the server answered {status}"))
}

enum Check {
    /// The server knows this host: (name).
    Valid(String),
    /// The server rejects the credential: the host was removed.
    Rejected,
    Unreachable(String),
}

fn check_credential(cfg: &AgentConfig) -> Check {
    let res = http().get(format!("{}/api/v1/agent/whoami", cfg.server)).header("Authorization", format!("Bearer {}.{}", cfg.host_id, cfg.secret)).call();
    match res {
        Ok(mut r) if r.status().as_u16() == 200 => {
            let name = r.body_mut().read_json::<serde_json::Value>().ok().and_then(|v| v.get("name").and_then(|n| n.as_str()).map(String::from)).unwrap_or_default();
            Check::Valid(name)
        }
        Ok(r) if r.status().as_u16() == 401 => Check::Rejected,
        Ok(mut r) => Check::Unreachable(problem(&mut r)),
        Err(e) => Check::Unreachable(e.to_string()),
    }
}

fn enroll(config: &Path, server: &str, token: &str, name: Option<String>, tags: Vec<String>, force: bool) -> Result<()> {
    if config.exists() && !force {
        // A credential the server no longer accepts (the host was removed) is replaced
        // without asking; a working one needs --force so it isn't dropped by accident.
        match load(config) {
            Ok(existing) => match check_credential(&existing) {
                Check::Rejected => println!("Replacing the old enrollment with {}: that server no longer accepts it.", existing.server),
                Check::Valid(name) => bail!("this host is already enrolled with {} as \"{name}\". Use --force to enroll it again.", existing.server),
                Check::Unreachable(err) => {
                    bail!("this host is already enrolled with {}, which can't be reached to check it ({err}). Use --force to enroll it again.", existing.server)
                }
            },
            Err(_) => println!("Replacing the unreadable file {}.", config.display()),
        }
    }
    let server = normalize_server(server)?;
    if server.starts_with("http://") {
        eprintln!("Warning: {server} is not HTTPS. Steps, their output and secrets will cross the network unencrypted.");
    }
    let body = serde_json::json!({ "token": token.trim(), "info": machine_info(), "name": name, "tags": tags });
    let mut res = http().post(format!("{server}/api/v1/agent/enroll")).send_json(&body).with_context(|| format!("could not reach {server}"))?;
    if res.status().as_u16() != 201 {
        bail!("enrollment failed: {}", problem(&mut res));
    }
    #[derive(Deserialize)]
    struct Enrolled {
        host_id: String,
        name: String,
        secret: String,
    }
    let e: Enrolled = res.body_mut().read_json().context("reading the enrollment response")?;
    let cfg = AgentConfig { server: server.clone(), host_id: e.host_id, secret: e.secret, work_dir: default_work_dir() };
    save(config, &cfg)?;
    println!("Enrolled as \"{}\" with {server}.", e.name);
    println!("Saved the credential to {}.", config.display());
    // A running agent still uses the old credential: restart it on the new one.
    let systemd = Path::new("/run/systemd/system").is_dir();
    let running = systemd && std::process::Command::new("systemctl").args(["is-active", "--quiet", "opskills-agent"]).status().is_ok_and(|s| s.success());
    if running && std::process::Command::new("systemctl").args(["restart", "opskills-agent"]).status().is_ok_and(|s| s.success()) {
        println!("Restarted the opskills-agent service with the new enrollment.");
    } else {
        println!("Start the agent: sudo systemctl enable --now opskills-agent");
    }
    Ok(())
}

fn status(config: &Path) -> Result<()> {
    let cfg = load(config)?;
    println!("Server:   {}", cfg.server);
    println!("Host id:  {}", cfg.host_id);
    println!("Work dir: {}", cfg.work_dir.display());
    match check_credential(&cfg) {
        Check::Valid(name) => println!("Status:   enrolled as \"{name}\", the server accepts this host"),
        Check::Rejected => bail!("the server no longer accepts this host (it was removed). Enroll it again with a new token."),
        Check::Unreachable(err) => bail!("can't reach the server: {err}"),
    }
    Ok(())
}

fn load(path: &Path) -> Result<AgentConfig> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {} (is this host enrolled? run opskills-agent enroll)", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn save(path: &Path, cfg: &AgentConfig) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string_pretty(cfg)?;
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

// The agent loop

enum PollError {
    Revoked,
    Other(String),
}

struct Client {
    http: ureq::Agent,
    server: String,
    auth: String,
}

enum OutputReply {
    Continue { cancel: bool },
    Gone,
    Failed,
}

impl Client {
    fn post(&self, path: &str, body: &impl Serialize) -> Result<ureq::http::Response<ureq::Body>, String> {
        self.http.post(format!("{}/api/v1/agent{path}", self.server)).header("Authorization", &self.auth).send_json(body).map_err(|e| e.to_string())
    }

    fn poll(&self, info: &AgentInfo) -> Result<Option<Job>, PollError> {
        let mut res = self.post("/poll", info).map_err(PollError::Other)?;
        match res.status().as_u16() {
            200 => res.body_mut().read_json::<Job>().map(Some).map_err(|e| PollError::Other(e.to_string())),
            204 => Ok(None),
            401 => Err(PollError::Revoked),
            _ => Err(PollError::Other(problem(&mut res))),
        }
    }

    fn output(&self, job_id: &str, lines: &[OutLine]) -> OutputReply {
        match self.post(&format!("/jobs/{job_id}/output"), &serde_json::json!({ "lines": lines })) {
            Ok(mut res) if res.status().as_u16() == 200 => {
                let cancel = res.body_mut().read_json::<serde_json::Value>().ok().and_then(|v| v.get("cancel").and_then(|c| c.as_bool())).unwrap_or(false);
                OutputReply::Continue { cancel }
            }
            Ok(res) if matches!(res.status().as_u16(), 401 | 404) => OutputReply::Gone,
            _ => OutputReply::Failed,
        }
    }

    fn finish(&self, job_id: &str, end: &JobEnd) -> bool {
        matches!(self.post(&format!("/jobs/{job_id}/finish"), end), Ok(r) if r.status().is_success())
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("agent request task")
}

fn run(config: &Path) -> ExitCode {
    init_logging();
    let cfg = match load(config) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!("{err:#}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(err) => {
            tracing::error!("starting the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(agent_loop(cfg, Some(config.to_path_buf())))
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("OPSKILLS_AGENT_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).with_ansi(false).without_time().init();
}

async fn agent_loop(cfg: AgentConfig, config_path: Option<PathBuf>) -> ExitCode {
    let client = Arc::new(Client { http: http(), server: cfg.server.clone(), auth: format!("Bearer {}.{}", cfg.host_id, cfg.secret) });
    let work_dir = cfg.work_dir.clone();
    if let Err(err) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&work_dir) {
        tracing::error!("creating {}: {err}", work_dir.display());
        return ExitCode::FAILURE;
    }
    tracing::info!(server = %cfg.server, host_id = %cfg.host_id, version = crate::VERSION, "agent started");
    let mut backoff = Duration::from_secs(1);
    let mut connected = false;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        let c = client.clone();
        let info = machine_info();
        let polled = tokio::select! {
            r = blocking(move || c.poll(&info)) => r,
            _ = &mut shutdown => {
                tracing::info!("stopping");
                return ExitCode::SUCCESS;
            }
        };
        match polled {
            Ok(job) => {
                if !connected {
                    tracing::info!("connected to the server");
                    connected = true;
                }
                backoff = Duration::from_secs(1);
                if let Some(job) = job {
                    tokio::spawn(run_job(client.clone(), job, work_dir.clone()));
                }
            }
            Err(PollError::Revoked) => {
                // Enrolled again while running? Switch to the new credential.
                if let Some(path) = &config_path
                    && let Ok(fresh) = load(path)
                        && (fresh.host_id != cfg.host_id || fresh.secret != cfg.secret || fresh.server != cfg.server) {
                            tracing::info!(host_id = %fresh.host_id, "found a new enrollment, switching to it");
                            return Box::pin(agent_loop(fresh, config_path.clone())).await;
                        }
                tracing::error!("the server doesn't accept this host's credential: it was removed from OpSkills. Enroll it again to reconnect.");
                return ExitCode::from(EXIT_REVOKED);
            }
            Err(PollError::Other(err)) => {
                if connected {
                    tracing::warn!("lost the connection to the server: {err}");
                    connected = false;
                } else {
                    tracing::warn!("can't reach the server, retrying in {}s: {err}", backoff.as_secs());
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

async fn shutdown_signal() {
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term => {} }
}

/// Run ids are UUIDs; anything else is refused so a job can't point outside the work folder.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

async fn run_job(client: Arc<Client>, job: Job, work_root: PathBuf) {
    tracing::info!(run_id = %job.run_id, step = job.step, "running step");
    let end = if !safe_id(&job.run_id) || !safe_id(&job.id) {
        JobEnd::not_run("The agent refused a step with an invalid id.")
    } else {
        execute(&client, &job, &work_root.join(&job.run_id)).await
    };
    // Report the result; retry for a while if the server is briefly unreachable.
    for attempt in 0..10u32 {
        let (c, id, e) = (client.clone(), job.id.clone(), end.clone());
        if blocking(move || c.finish(&id, &e)).await {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1 + attempt as u64)).await;
    }
    tracing::info!(run_id = %job.run_id, step = job.step, end = ?end.end, exit_code = ?end.exit_code, "step finished");
}

async fn execute(client: &Arc<Client>, job: &Job, workdir: &Path) -> JobEnd {
    if let Err(err) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(workdir) {
        return JobEnd::not_run(format!("Could not create the working directory on the host: {err}"));
    }
    let mut env = base_env(workdir, &job.run_id, job.step);
    env.extend(job.env.iter().cloned());
    let (line_tx, line_rx) = mpsc::unbounded_channel::<OutLine>();
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let forwarder = tokio::spawn(forward(client.clone(), job.id.clone(), line_rx, cancel_tx));
    let timeout = Duration::from_secs(job.timeout_seconds.max(1));
    let end = run_command(&job.command, workdir, &env, timeout, &mut cancel_rx, |stream, line| {
        let _ = line_tx.send(OutLine { stream: stream.into(), line });
    })
    .await;
    drop(line_tx);
    let _ = forwarder.await;
    let succeeded = end.end == End::Exited && end.exit_code == Some(0);
    let done = if succeeded { job.last } else { !job.keep_on_failure };
    if done {
        // No later step of this run comes here.
        let _ = std::fs::remove_dir_all(workdir);
    }
    end
}

/// Send output to the server about once a second (an empty batch doubles as a
/// heartbeat) and pass on a cancel request.
async fn forward(client: Arc<Client>, job_id: String, mut rx: mpsc::UnboundedReceiver<OutLine>, cancel_tx: watch::Sender<bool>) {
    let mut pending: Vec<OutLine> = Vec::new();
    let mut open = true;
    let mut failures = 0u32;
    let mut first = true;
    while open || !pending.is_empty() {
        // Check in right away so the server knows the step started, then once a second.
        let tick = tokio::time::sleep(if first { Duration::ZERO } else { Duration::from_secs(1) });
        first = false;
        tokio::pin!(tick);
        while open && pending.len() < 500 {
            tokio::select! {
                line = rx.recv() => match line {
                    Some(l) => pending.push(l),
                    None => open = false,
                },
                _ = &mut tick => break,
            }
        }
        let batch: Vec<OutLine> = pending.drain(..pending.len().min(500)).collect();
        let (c, id, b) = (client.clone(), job_id.clone(), batch.clone());
        match blocking(move || c.output(&id, &b)).await {
            OutputReply::Continue { cancel } => {
                failures = 0;
                if cancel {
                    let _ = cancel_tx_send(&cancel_tx);
                }
            }
            OutputReply::Gone => {
                // The server stopped following this step: stop it and drop its output.
                let _ = cancel_tx_send(&cancel_tx);
                pending.clear();
            }
            OutputReply::Failed => {
                failures += 1;
                if !open && failures > 30 {
                    tracing::warn!(job_id = %job_id, "giving up sending output: the server is unreachable");
                    break;
                }
                // Keep the lines for the next attempt, within reason.
                if pending.len() + batch.len() <= 20_000 {
                    let rest = std::mem::take(&mut pending);
                    pending = batch;
                    pending.extend(rest);
                }
                if !open {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

fn cancel_tx_send(tx: &watch::Sender<bool>) -> Result<(), watch::error::SendError<bool>> {
    tx.send(true)
}
