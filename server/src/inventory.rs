//! Inventory sources, refreshed on their interval, and each workspace's hosts as runbooks
//! and the Hosts page see them: enrolled hosts with their attributes and what sources
//! say about them, hosts only a source lists, and tags from tag rules.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::audit::{self, By};
use crate::filter::HostFilter;
use crate::model::{AuditTarget, Host, InventorySource, SourceHost, SourceKind, SourceRecord, SourceState, now_ms, parse_hosts_document, secret_env_name};
use crate::web::AppState;

/// Most a URL or program may return.
const MAX_DOCUMENT: u64 = 4_000_000;
const URL_TIMEOUT: Duration = Duration::from_secs(20);
const PROGRAM_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the loop looks for sources that are due.
const TICK: Duration = Duration::from_secs(10);

/// Sources being refreshed now, so a slow one isn't started twice.
fn running() -> &'static Mutex<HashSet<String>> {
    static RUNNING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Refresh each enabled source when its interval has passed.
pub fn spawn(st: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            let Ok(sources) = st.store.inventory_sources() else { continue };
            let now = now_ms();
            for src in sources.into_iter().filter(|s| s.enabled) {
                let due = st.store.source_state(&src.id).map(|s| s.last_attempt.is_none_or(|t| now.saturating_sub(t) >= src.interval_seconds * 1000)).unwrap_or(false);
                if due && running().lock().unwrap().insert(src.id.clone()) {
                    let st = st.clone();
                    tokio::task::spawn_blocking(move || {
                        // Cleared even if the refresh panics, so the source isn't stuck.
                        struct Done(String);
                        impl Drop for Done {
                            fn drop(&mut self) {
                                running().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
                            }
                        }
                        let _done = Done(src.id.clone());
                        refresh(&st, &src, By::system("inventory refresh"), false);
                    });
                }
            }
        }
    });
}

/// Ask a source for its hosts. Blocking.
pub fn fetch(st: &AppState, src: &InventorySource) -> Result<Vec<SourceHost>, String> {
    match &src.kind {
        SourceKind::Static { hosts } => Ok(hosts.clone()),
        SourceKind::Url { url, header, secret } => {
            let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(URL_TIMEOUT)).http_status_as_error(false).max_redirects(0).build().into();
            let mut req = agent.get(url.as_str());
            let mut hidden = None;
            if let (Some(h), Some(s)) = (header, secret) {
                let value = crate::secret_refs::value(&st.store, &st.vault, &src.workspace, s).map_err(|e| format!("the header's secret: {e}"))?;
                req = req.header(h.as_str(), value.as_str());
                hidden = Some(value);
            }
            let scrub = |e: String| match &hidden {
                Some(v) if !v.is_empty() => e.replace(v.as_str(), "••••••"),
                _ => e,
            };
            let mut res = req.call().map_err(|e| scrub(format!("couldn't fetch {}: {e}", host_of(url))))?;
            if res.status().is_redirection() {
                // Not followed, so the header's secret never goes to another server.
                return Err(format!("{} answered {}; redirects aren't followed, so use the final URL", host_of(url), res.status()));
            }
            if !res.status().is_success() {
                return Err(format!("{} answered {}", host_of(url), res.status()));
            }
            let body = res.body_mut().with_config().limit(MAX_DOCUMENT).read_to_vec().map_err(|e| scrub(format!("reading the answer from {}: {e}", host_of(url))))?;
            parse_hosts_document(&body)
        }
        SourceKind::Program { command, args, config, secrets } => {
            let mut cmd = Command::new(command);
            cmd.args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            for k in ["PATH", "LANG", "HOME", "TMPDIR"] {
                if let Ok(v) = std::env::var(k) {
                    cmd.env(k, v);
                }
            }
            cmd.env("OPSKILLS_WORKSPACE", &src.workspace).env("OPSKILLS_SOURCE", &src.name);
            for (k, v) in config {
                cmd.env(format!("OPSKILLS_CONFIG_{k}"), v);
            }
            let mut hidden = Vec::new();
            for s in secrets {
                let value = crate::secret_refs::value(&st.store, &st.vault, &src.workspace, s).map_err(|e| format!("the secret {s}: {e}"))?;
                cmd.env(secret_env_name(s), &value);
                hidden.push(value);
            }
            let scrub = |mut e: String| {
                for v in hidden.iter().filter(|v| !v.is_empty()) {
                    e = e.replace(v.as_str(), "••••••");
                }
                e
            };
            let out = run_program(cmd).map_err(|e| scrub(format!("{command}: {e}")))?;
            parse_hosts_document(&out).map_err(scrub)
        }
    }
}

/// The host part of a URL, for messages that shouldn't show paths or keys in them.
fn host_of(url: &str) -> String {
    url.split("://").nth(1).and_then(|r| r.split(['/', '?']).next()).unwrap_or(url).to_string()
}

/// Run a program with a time limit; its standard output, or why it failed.
fn run_program(mut cmd: Command) -> Result<Vec<u8>, String> {
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout).take(MAX_DOCUMENT + 1).read_to_end(&mut buf);
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(64_000).read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if started.elapsed() > PROGRAM_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("still running after {} seconds; stopped it", PROGRAM_TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = out.join().unwrap_or_default();
    let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).trim().to_string();
    if !status.success() {
        let tail: String = err.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(format!("exited with {status}{}", if tail.is_empty() { String::new() } else { format!(": {tail}") }));
    }
    if out.len() as u64 > MAX_DOCUMENT {
        return Err(format!("printed more than {} MB", MAX_DOCUMENT / 1_000_000));
    }
    Ok(out)
}

/// Refresh a source now and keep what it found. A failure keeps the hosts of the last
/// good refresh and records why. A source that lists nobody after listing hosts counts
/// as a failure unless `accept_empty`. Blocking.
pub fn refresh(st: &AppState, src: &InventorySource, by: By, accept_empty: bool) -> SourceState {
    let mut state = st.store.source_state(&src.id).unwrap_or_default();
    let now = now_ms();
    state.last_attempt = Some(now);
    let target = || Some(AuditTarget::new("inventory_source", &src.id, &src.name));
    let result = fetch(st, src).and_then(|hosts| {
        if hosts.is_empty() && !state.hosts.is_empty() && !accept_empty {
            Err(format!("it listed no hosts after listing {}; kept those. If that's right, refresh it and accept an empty list.", state.hosts.len()))
        } else {
            Ok(hosts)
        }
    });
    match result {
        Ok(hosts) => {
            let before: BTreeMap<String, SourceRecord> = state.hosts.drain(..).map(|r| (r.host.name.to_lowercase(), r)).collect();
            let mut added = Vec::new();
            let records: Vec<SourceRecord> = hosts
                .into_iter()
                .map(|h| match before.get(&h.name.to_lowercase()) {
                    Some(old) => SourceRecord { id: old.id.clone(), host: h, first_seen: old.first_seen },
                    None => {
                        added.push(h.name.clone());
                        SourceRecord { id: uuid::Uuid::now_v7().to_string(), host: h, first_seen: now }
                    }
                })
                .collect();
            let kept: HashSet<String> = records.iter().map(|r| r.host.name.to_lowercase()).collect();
            let removed: Vec<String> = before.values().filter(|r| !kept.contains(&r.host.name.to_lowercase())).map(|r| r.host.name.clone()).collect();
            if state.error.is_some() {
                audit::record(st, &by, Some(&src.workspace), "inventory_source.recovered", target(), Some(format!("{} hosts", records.len())));
            }
            if !added.is_empty() || !removed.is_empty() {
                let list = |v: &[String]| if v.len() > 10 { format!("{} and {} more", v[..10].join(", "), v.len() - 10) } else { v.join(", ") };
                let mut parts = Vec::new();
                if !added.is_empty() {
                    parts.push(format!("added {}", list(&added)));
                }
                if !removed.is_empty() {
                    parts.push(format!("removed {}", list(&removed)));
                }
                audit::record(st, &by, Some(&src.workspace), "inventory_source.hosts_change", target(), Some(parts.join("; ")));
            }
            state.hosts = records;
            state.last_success = Some(now);
            state.error = None;
        }
        Err(e) => {
            tracing::warn!(source = %src.name, workspace = %src.workspace, error = %e, "inventory refresh failed");
            if state.error.is_none() {
                audit::record(st, &by, Some(&src.workspace), "inventory_source.refresh_failed", target(), Some(e.clone()));
            }
            state.error = Some(e);
        }
    }
    if let Err(e) = st.store.put_source_state(&src.id, &state) {
        tracing::error!(error = %e, "saving an inventory refresh failed");
    }
    state
}

/// How a host is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// The OpSkills agent.
    Agent,
    /// Only listed by a source: nothing reaches it yet.
    Listed,
}

/// A host of a workspace as runbooks see it.
#[derive(Clone, Debug)]
pub struct InvHost {
    /// Tags and attributes here include those from sources and tag rules.
    pub host: Host,
    pub reach: Reach,
    /// The sources that list it, as (id, name).
    pub sources: Vec<(String, String)>,
    /// Tags it has from tag rules.
    pub rule_tags: Vec<String>,
    /// A source that lists it failed its last refresh, so this may be out of date.
    pub stale: bool,
}

/// Every host of workspace `ws` (without the OpSkills server): enrolled hosts first, each
/// with what sources say about it, then hosts only a source lists. A source host matches
/// an enrolled host with the same hostname (or name, when it gives no hostname). An
/// Admin's attribute wins over a source's; among sources, the first to set it.
pub fn workspace_hosts(st: &AppState, ws: &str) -> anyhow::Result<Vec<InvHost>> {
    let mut out: Vec<InvHost> = st
        .store
        .hosts()?
        .into_iter()
        .filter(|h| h.workspaces.iter().any(|w| w == ws))
        .map(|host| InvHost { host, reach: Reach::Agent, sources: vec![], rule_tags: vec![], stale: false })
        .collect();
    // Enrolled hosts by lowercase hostname and name; the first host in order wins a key.
    let mut by_key: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, h) in out.iter().enumerate() {
        for k in [&h.host.hostname, &h.host.name] {
            if !k.is_empty() {
                by_key.entry(k.to_lowercase()).or_insert(i);
            }
        }
    }
    for src in st.store.inventory_sources()?.into_iter().filter(|s| s.workspace == ws && s.enabled) {
        let state = st.store.source_state(&src.id)?;
        let stale = state.error.is_some();
        for rec in state.hosts {
            let key = if rec.host.hostname.is_empty() { rec.host.name.to_lowercase() } else { rec.host.hostname.to_lowercase() };
            let found = by_key.get(&key).map(|&i| &mut out[i]);
            match found {
                Some(h) => {
                    for t in &rec.host.tags {
                        if !h.host.tags.contains(t) {
                            h.host.tags.push(t.clone());
                        }
                    }
                    for (k, v) in &rec.host.attributes {
                        h.host.attributes.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    h.sources.push((src.id.clone(), src.name.clone()));
                    h.stale |= stale;
                }
                None => out.push(InvHost {
                    host: Host {
                        id: rec.id,
                        name: rec.host.name,
                        workspaces: vec![ws.to_string()],
                        hostname: rec.host.hostname,
                        os: String::new(),
                        arch: String::new(),
                        agent_version: String::new(),
                        tags: rec.host.tags,
                        enrolled_at: rec.first_seen,
                        last_seen: state.last_success.unwrap_or(0),
                        attributes: rec.host.attributes,
                    },
                    reach: Reach::Listed,
                    sources: vec![(src.id.clone(), src.name.clone())],
                    rule_tags: vec![],
                    stale,
                }),
            }
        }
    }
    let rules: Vec<(HostFilter, Vec<String>)> =
        st.store.workspace(ws)?.map(|w| w.tag_rules).unwrap_or_default().into_iter().filter_map(|r| HostFilter::parse(&r.filter).ok().map(|f| (f, r.tags))).collect();
    for h in &mut out {
        // Rules match the host as sources left it; their tags don't feed other rules.
        let matched: Vec<String> = rules.iter().filter(|(f, _)| f.matches(&h.host)).flat_map(|(_, tags)| tags.clone()).collect();
        for t in matched {
            if !h.host.tags.contains(&t) {
                h.host.tags.push(t.clone());
                h.rule_tags.push(t);
            }
        }
    }
    Ok(out)
}
