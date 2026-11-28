//! Steps on hosts without the agent, over SSH with the system OpenSSH client.
//!
//! The step travels on SSH's standard input as a small shell script: its environment
//! (secrets and copied file content included) as quoted exports, so nothing secret is
//! on a command line. Host keys are pinned per host in the database: on first contact
//! `ssh-keyscan` fetches them (checked against keys the source gives, if any), and every
//! connection then requires exactly those keys.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::config::Ssh as SshConfig;
use crate::executor::wait_cancel;
use crate::hub::{End, JobEnd};
use crate::model::{SshAuth, SshTarget, expected, fingerprint, normalize_key, now_ms};
use crate::store::Store;
use crate::template::shell_quote;

/// Printed by the remote script just before the step starts. Anything that fails before
/// it is SSH's or the script's own failure, not the step's.
const STARTED: &str = "\u{1e}opskills-started";
/// After a cancel or timeout closes standard input, how long the remote side gets to stop
/// the step before the connection is cut.
const STOP_GRACE: Duration = Duration::from_secs(15);
/// Lines kept from before the step starts, to explain a failure.
const EARLY_LINES: usize = 50;

/// How one attempt went, besides the step's own result.
#[derive(Debug, PartialEq)]
pub enum Contact {
    /// The step started on the host.
    Ran,
    /// SSH or the remote script failed before the step started.
    Failed(String),
    /// The host's key isn't the pinned one.
    KeyChanged,
    /// Cancelled or timed out before the step started.
    Stopped,
}

/// What a step needs on the remote side.
pub struct Step<'a> {
    pub run_id: &'a str,
    pub number: usize,
    pub command: &'a str,
    pub env: &'a [(String, String)],
    /// The last step on this host: remove the work folder after it.
    pub last: bool,
    /// Keep the work folder if this step fails (later steps still use it).
    pub keep_on_failure: bool,
}

/// The script SSH's remote shell reads on standard input. It is one `{ ... }` block, so
/// the shell reads all of it before running any of it and standard input is left to the
/// watcher, which stops the step when OpSkills closes it (cancel, timeout, lost server).
pub fn remote_script(step: &Step<'_>) -> String {
    // Standard input stays on fd 3 for the watcher: a background job's own standard
    // input is /dev/null in a non-interactive shell.
    let mut s = String::from("{\nexec 3<&0\numask 077\n");
    s.push_str(&format!("w=\"${{HOME:-/tmp}}/.opskills/work/{}\"\n", step.run_id.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-'), "")));
    s.push_str("mkdir -p \"$w\" && cd \"$w\" || { echo \"Could not use the work folder $w.\" >&2; exit 125; }\n");
    s.push_str(&format!("export HOME=\"$w\" PATH={} LANG=C.UTF-8\n", shell_quote(crate::executor::SAFE_PATH)));
    s.push_str(&format!("export OPSKILLS_RUN_ID={} OPSKILLS_STEP={}\n", shell_quote(step.run_id), step.number));
    for (k, v) in step.env {
        if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()) {
            s.push_str(&format!("export {k}={}\n", shell_quote(v)));
        }
    }
    s.push_str(&format!("c={}\n", shell_quote(step.command)));
    s.push_str(&format!("printf '%s\\n' {}\n", shell_quote(STARTED)));
    // Its own process group where setsid exists, so a stop reaches what it started.
    s.push_str("if command -v setsid >/dev/null 2>&1; then setsid /bin/sh -c \"$c\" </dev/null 3<&- & else /bin/sh -c \"$c\" </dev/null 3<&- & fi\n");
    s.push_str("p=$!\n");
    s.push_str(
        "( while read -r _; do :; done; kill -s TERM -- \"-$p\" 2>/dev/null || kill -s TERM \"$p\"; sleep 5; kill -s KILL -- \"-$p\" 2>/dev/null || kill -s KILL \"$p\" ) <&3 >/dev/null 2>&1 &\n",
    );
    // Some shells report killed jobs on standard error; those lines aren't the step's.
    s.push_str("k=$!\nexec 3<&-\n{ wait \"$p\"; rc=$?; kill \"$k\"; wait \"$k\"; } 2>/dev/null\n");
    if step.last {
        s.push_str("cd / && rm -rf \"$w\"\n");
    } else if !step.keep_on_failure {
        s.push_str("[ \"$rc\" -eq 0 ] || { cd / && rm -rf \"$w\"; }\n");
    }
    s.push_str("exit \"$rc\"\n}\n");
    s
}

/// A folder for one connection's files (known hosts, key), readable only by OpSkills and
/// removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(root: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = root.join(format!("ssh-{}", uuid::Uuid::now_v7()));
        std::fs::DirBuilder::new().mode(0o700).recursive(true).create(&dir)?;
        Ok(Scratch(dir))
    }

    fn write(&self, name: &str, content: &str) -> std::io::Result<PathBuf> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.0.join(name);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        f.write_all(content.as_bytes())?;
        if !content.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The alias pinned keys are filed under, so a changed hostname or port doesn't matter.
fn alias(host_id: &str) -> String {
    format!("opskills-{host_id}")
}

/// The `ssh` command for one connection to `target`, trusting only `keys`.
fn command(cfg: &SshConfig, scratch: &Scratch, host_id: &str, target: &SshTarget, keys: &[String], credential: &str) -> std::io::Result<Command> {
    let known: String = keys.iter().map(|k| format!("{} {k}\n", alias(host_id))).collect();
    let known_hosts = scratch.write("known_hosts", &known)?;
    let mut cmd = Command::new(&cfg.client);
    cmd.env_clear().env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())).env("HOME", &scratch.0);
    let o = |opt: String| ["-o".to_string(), opt];
    let mut args: Vec<String> = vec!["-F".into(), "/dev/null".into(), "-T".into()];
    for opt in [
        "StrictHostKeyChecking=yes".to_string(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "GlobalKnownHostsFile=/dev/null".into(),
        format!("HostKeyAlias={}", alias(host_id)),
        "CheckHostIP=no".into(),
        "UpdateHostKeys=no".into(),
        format!("ConnectTimeout={}", cfg.connect_timeout_seconds.max(1)),
        "ServerAliveInterval=15".into(),
        "ServerAliveCountMax=3".into(),
        "LogLevel=ERROR".into(),
        "ForwardAgent=no".into(),
        "ForwardX11=no".into(),
        "ClearAllForwardings=yes".into(),
        "IdentitiesOnly=yes".into(),
        "IdentityAgent=none".into(),
    ] {
        args.extend(o(opt));
    }
    match target.auth {
        SshAuth::Key => {
            let key = scratch.write("key", credential)?;
            args.extend(["-i".into(), key.display().to_string()]);
            for opt in ["BatchMode=yes", "PreferredAuthentications=publickey", "PasswordAuthentication=no", "KbdInteractiveAuthentication=no"] {
                args.extend(o(opt.into()));
            }
        }
        SshAuth::Password => {
            // No terminal, so SSH asks the askpass program, which prints the password.
            let askpass = if cfg.askpass.is_empty() { std::env::current_exe()?.display().to_string() } else { cfg.askpass.clone() };
            cmd.env("SSH_ASKPASS", askpass).env("SSH_ASKPASS_REQUIRE", "force").env("DISPLAY", "none").env(ASKPASS_FLAG, "1").env(ASKPASS_VALUE, credential);
            for opt in ["BatchMode=no", "PreferredAuthentications=password,keyboard-interactive", "PubkeyAuthentication=no", "NumberOfPasswordPrompts=1"] {
                args.extend(o(opt.into()));
            }
        }
    }
    args.extend(["-p".into(), target.port.to_string(), "-l".into(), target.user.clone(), "--".into(), target.address.clone()]);
    let remote = if target.sudo && target.user != "root" { "exec sudo -n -H /bin/sh -s" } else { "exec /bin/sh -s" };
    args.push(remote.into());
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    Ok(cmd)
}

/// Set for the askpass program, so the opskills binary knows to print the password.
pub const ASKPASS_FLAG: &str = "OPSKILLS_SSH_ASKPASS";
pub const ASKPASS_VALUE: &str = "OPSKILLS_SSH_PASSWORD";

/// When this binary runs as SSH's askpass program: print the password and exit.
pub fn askpass_mode() -> bool {
    if std::env::var(ASKPASS_FLAG).as_deref() == Ok("1") {
        println!("{}", std::env::var(ASKPASS_VALUE).unwrap_or_default());
        return true;
    }
    false
}

/// Run one step on `target`. `keys` are the host's pinned keys; `credential` is the
/// private key or password. Output lines go to `emit` as they arrive.
#[allow(clippy::too_many_arguments)]
pub async fn run_step(
    cfg: &SshConfig,
    work_root: &Path,
    host_id: &str,
    target: &SshTarget,
    keys: &[String],
    credential: &str,
    script: &str,
    timeout: Duration,
    cancel_rx: &mut watch::Receiver<bool>,
    mut emit: impl FnMut(&'static str, String),
) -> (JobEnd, Contact) {
    let scratch = match Scratch::new(work_root) {
        Ok(s) => s,
        Err(e) => return (JobEnd::not_run(format!("Could not prepare the SSH connection: {e}")), Contact::Failed(e.to_string())),
    };
    let mut child = match command(cfg, &scratch, host_id, target, keys, credential).and_then(|mut c| c.spawn()) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Could not start {}: {e}. Is the OpenSSH client installed on the OpSkills server?", cfg.client);
            return (JobEnd::not_run(msg.clone()), Contact::Failed(msg));
        }
    };
    let mut stdin = child.stdin.take();
    if let Some(i) = stdin.as_mut()
        && let Err(e) = i.write_all(script.as_bytes()).await
    {
        tracing::debug!(error = %e, "writing the step to ssh");
    }
    let (tx, mut rx) = mpsc::channel::<(&'static str, String)>(1024);
    let readers = [
        child.stdout.take().map(|o| tokio::spawn(read_lines(o, "stdout", tx.clone()))),
        child.stderr.take().map(|e| tokio::spawn(read_lines(e, "stderr", tx.clone()))),
    ];
    drop(tx);

    let mut started = false;
    let mut early: Vec<(&'static str, String)> = Vec::new();
    let mut handle = |stream: &'static str, line: String, started: &mut bool, early: &mut Vec<(&'static str, String)>| {
        if *started {
            emit(stream, line);
        } else if stream == "stdout" && line == STARTED {
            *started = true;
            // Lines the step wrote before the marker was read (another stream) are its own.
            for (s, l) in early.drain(..) {
                emit(s, l);
            }
        } else if early.len() < EARLY_LINES {
            early.push((stream, line));
        }
    };
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut cut_at: Option<tokio::time::Instant> = None;
    let mut end = End::Exited;
    let status = loop {
        let cut_timer = async {
            match cut_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            Some((stream, line)) = rx.recv() => handle(stream, line, &mut started, &mut early),
            status = child.wait() => break status.ok(),
            _ = &mut deadline, if cut_at.is_none() => {
                end = End::TimedOut;
                stdin = None;
                cut_at = Some(tokio::time::Instant::now() + STOP_GRACE);
            }
            _ = wait_cancel(cancel_rx), if cut_at.is_none() => {
                end = End::Cancelled;
                stdin = None;
                cut_at = Some(tokio::time::Instant::now() + STOP_GRACE);
            }
            _ = cut_timer => {
                let _ = child.start_kill();
                cut_at = Some(tokio::time::Instant::now() + Duration::from_secs(3600));
            }
        }
    };
    drop(stdin);
    let drain = async {
        while let Some((stream, line)) = rx.recv().await {
            handle(stream, line, &mut started, &mut early);
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
    for r in readers.into_iter().flatten() {
        r.abort();
    }
    let code = status.and_then(|s| {
        use std::os::unix::process::ExitStatusExt;
        s.code().or_else(|| s.signal().map(|n| 128 + n))
    });
    if started {
        return (JobEnd { end, exit_code: code, error: None }, Contact::Ran);
    }
    if end != End::Exited {
        return (JobEnd { end, exit_code: None, error: None }, Contact::Stopped);
    }
    let text: Vec<&str> = early.iter().map(|(_, l)| l.as_str()).filter(|l| !l.trim().is_empty() && !l.starts_with("@@@")).collect();
    if text.iter().any(|l| l.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") || l.contains("Host key verification failed")) {
        return (JobEnd::not_run(String::new()), Contact::KeyChanged);
    }
    let why = if text.is_empty() { format!("SSH exited with code {}", code.unwrap_or(-1)) } else { text[text.len().saturating_sub(3)..].join(" ") };
    let mut msg = format!("Could not run the step over SSH ({}): {why}", target.label());
    if why.contains("sudo") && why.contains("password") {
        msg.push_str(" Allow this user to run sudo without a password, or connect as root.");
    }
    (JobEnd::not_run(msg.clone()), Contact::Failed(why))
}

async fn read_lines(reader: impl tokio::io::AsyncRead + Unpin, stream: &'static str, tx: mpsc::Sender<(&'static str, String)>) {
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

/// The public keys `address` presents now, as `type base64`.
pub async fn keyscan(cfg: &SshConfig, address: &str, port: u16) -> Result<Vec<String>, String> {
    let wait = cfg.connect_timeout_seconds.max(1);
    let out = Command::new(&cfg.keyscan)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()))
        .args(["-T", &wait.to_string(), "-p", &port.to_string(), "--", address])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(wait + 10), out)
        .await
        .map_err(|_| "fetching its SSH keys took too long".to_string())?
        .map_err(|e| format!("could not start {}: {e}", cfg.keyscan))?;
    let mut keys: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        if line.starts_with('#') || parts.next().is_none() {
            continue;
        }
        if let Some(k) = normalize_key(&parts.collect::<Vec<_>>().join(" "))
            && !keys.contains(&k)
        {
            keys.push(k);
        }
    }
    if keys.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).lines().rev().find(|l| !l.starts_with('#') && !l.trim().is_empty()).unwrap_or("no answer").to_string();
        return Err(format!("could not get its SSH host keys: {err}"));
    }
    Ok(keys)
}

/// The keys to trust for `host_id`: the pinned ones, or on first contact the keys it
/// presents now, checked against those its source gives. Returns the keys and, when
/// they were just pinned, their fingerprints.
pub async fn trusted_keys(cfg: &SshConfig, store: &Store, host_id: &str, target: &SshTarget) -> Result<(Vec<String>, Option<Vec<String>>), String> {
    let state = store.ssh_host(host_id).map_err(|e| e.to_string())?;
    if !state.keys.is_empty() {
        return Ok((state.keys, None));
    }
    let found = keyscan(cfg, &target.address, target.port).await?;
    let keep: Vec<String> = if target.expected_keys.is_empty() { found.clone() } else { found.iter().filter(|k| expected(k, &target.expected_keys)).cloned().collect() };
    if keep.is_empty() {
        let fps: Vec<String> = found.iter().filter_map(|k| fingerprint(k)).collect();
        return Err(format!("its SSH host keys ({}) don't match the ones its inventory source gives, so nothing was sent to it", fps.join(", ")));
    }
    let mut pinned_now = None;
    let state = store
        .update_ssh_host(host_id, |s| {
            // Another step may have pinned keys in the meantime: those win.
            if s.keys.is_empty() {
                s.keys = keep.clone();
                s.pinned_at = Some(now_ms());
                s.pinned_by = Some(if target.expected_keys.is_empty() { "first connection".into() } else { "inventory source".into() });
                s.key_changed_at = None;
                pinned_now = Some(keep.iter().filter_map(|k| fingerprint(k)).collect());
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((state.keys, pinned_now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_script_quotes_everything_and_cleans_up() {
        let env = vec![("DB_PASSWORD".to_string(), "it's $secret".to_string()), ("BAD-NAME".to_string(), "x".to_string())];
        let script = remote_script(&Step { run_id: "0190-ab", number: 2, command: "echo 'hi' && exit 3", env: &env, last: true, keep_on_failure: false });
        assert!(script.starts_with("{\n") && script.ends_with("}\n"));
        assert!(script.contains("export DB_PASSWORD='it'\\''s $secret'"));
        assert!(!script.contains("BAD-NAME"));
        assert!(script.contains("c='echo '\\''hi'\\'' && exit 3'"));
        assert!(script.contains("rm -rf \"$w\""));
        let keep = remote_script(&Step { run_id: "r", number: 1, command: "true", env: &[], last: false, keep_on_failure: true });
        assert!(!keep.contains("rm -rf"));
    }
}
