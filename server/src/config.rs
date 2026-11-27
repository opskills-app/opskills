//! Configuration: one TOML file, with `OPSKILLS_<SECTION>__<KEY>` environment overrides.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_CONFIG: &str = "/etc/opskills/opskills.toml";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub data: Data,
    pub logs: Logs,
    pub auth: Auth,
    pub runs: Runs,
    pub hosts: Hosts,
    pub audit: Audit,
    pub email: Email,
}

/// The SMTP server notifications are sent through. Email is off until `smtp_host` is set.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Email {
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    /// "starttls" (default, usually port 587), "tls" (port 465), or "none" (only for
    /// a relay on the same machine or network).
    pub security: String,
    pub username: Option<String>,
    /// Never written out by `opskills config`.
    #[serde(skip_serializing)]
    pub password: Option<String>,
    /// Sender, like "OpSkills <opskills@example.com>".
    pub from: Option<String>,
}

impl Default for Email {
    fn default() -> Self {
        Self { smtp_host: None, smtp_port: 587, security: "starttls".into(), username: None, password: None, from: None }
    }
}

impl Email {
    pub fn enabled(&self) -> bool {
        self.smtp_host.as_deref().is_some_and(|h| !h.is_empty())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    /// Address and port to listen on.
    pub listen: String,
    /// Public URL users reach the server at, used in links.
    pub public_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Data {
    /// Directory for the server identity, keys and (later) the database.
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Logs {
    /// "json" (default) or "pretty".
    pub format: String,
    /// Level or filter, e.g. "info" or "opskills=debug,info".
    pub level: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// How long a sign-in lasts, in hours.
    pub session_hours: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Audit {
    /// How many days of audit log to keep, at most `MAX_AUDIT_RETENTION_DAYS`; older
    /// events are removed every hour.
    pub retention_days: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Runs {
    /// How many runs may execute at the same time; others wait in the queue.
    pub max_concurrent: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hosts {
    /// Folder with the agent packages offered for download. Missing packages are
    /// downloaded from the project's GitHub release instead.
    pub packages_dir: PathBuf,
}

impl Default for Hosts {
    fn default() -> Self {
        Self { packages_dir: PathBuf::from("/usr/share/opskills/packages") }
    }
}

impl Default for Auth {
    fn default() -> Self {
        Self { session_hours: 8 }
    }
}

impl Default for Audit {
    fn default() -> Self {
        Self { retention_days: 90 }
    }
}

impl Default for Runs {
    fn default() -> Self {
        Self { max_concurrent: 4 }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self { listen: "0.0.0.0:8080".into(), public_url: None }
    }
}

impl Default for Data {
    fn default() -> Self {
        Self { dir: PathBuf::from("/var/lib/opskills") }
    }
}

impl Default for Logs {
    fn default() -> Self {
        Self { format: "json".into(), level: "info".into() }
    }
}

impl Config {
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

/// Load the config file (explicit path, or the default path if present), then apply
/// environment overrides.
pub fn load(explicit: Option<&Path>) -> Result<Config> {
    let mut cfg = match explicit {
        Some(path) => read(path)?,
        None if Path::new(DEFAULT_CONFIG).exists() => read(Path::new(DEFAULT_CONFIG))?,
        None => Config::default(),
    };
    apply_env(&mut cfg, |key| std::env::var(key).ok())?;
    validate(&cfg)?;
    Ok(cfg)
}

fn read(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn apply_env(cfg: &mut Config, get: impl Fn(&str) -> Option<String>) -> Result<()> {
    if let Some(v) = get("OPSKILLS_SERVER__LISTEN") {
        cfg.server.listen = v;
    }
    if let Some(v) = get("OPSKILLS_SERVER__PUBLIC_URL") {
        cfg.server.public_url = Some(v).filter(|s| !s.is_empty());
    }
    if let Some(v) = get("OPSKILLS_DATA__DIR") {
        cfg.data.dir = PathBuf::from(v);
    }
    if let Some(v) = get("OPSKILLS_LOGS__FORMAT") {
        cfg.logs.format = v;
    }
    if let Some(v) = get("OPSKILLS_LOGS__LEVEL") {
        cfg.logs.level = v;
    }
    if let Some(v) = get("OPSKILLS_AUTH__SESSION_HOURS") {
        cfg.auth.session_hours = v.parse().context("OPSKILLS_AUTH__SESSION_HOURS must be a whole number")?;
    }
    if let Some(v) = get("OPSKILLS_HOSTS__PACKAGES_DIR") {
        cfg.hosts.packages_dir = PathBuf::from(v);
    }
    if let Some(v) = get("OPSKILLS_AUDIT__RETENTION_DAYS") {
        cfg.audit.retention_days = v.parse().context("OPSKILLS_AUDIT__RETENTION_DAYS must be a whole number")?;
    }
    if let Some(v) = get("OPSKILLS_RUNS__MAX_CONCURRENT") {
        cfg.runs.max_concurrent = v.parse().context("OPSKILLS_RUNS__MAX_CONCURRENT must be a whole number")?;
    }
    let some = |v: String| Some(v).filter(|s| !s.is_empty());
    if let Some(v) = get("OPSKILLS_EMAIL__SMTP_HOST") {
        cfg.email.smtp_host = some(v);
    }
    if let Some(v) = get("OPSKILLS_EMAIL__SMTP_PORT") {
        cfg.email.smtp_port = v.parse().context("OPSKILLS_EMAIL__SMTP_PORT must be a port number")?;
    }
    if let Some(v) = get("OPSKILLS_EMAIL__SECURITY") {
        cfg.email.security = v;
    }
    if let Some(v) = get("OPSKILLS_EMAIL__USERNAME") {
        cfg.email.username = some(v);
    }
    if let Some(v) = get("OPSKILLS_EMAIL__PASSWORD") {
        cfg.email.password = some(v);
    }
    if let Some(v) = get("OPSKILLS_EMAIL__FROM") {
        cfg.email.from = some(v);
    }
    Ok(())
}

/// The longest the audit log is kept.
pub const MAX_AUDIT_RETENTION_DAYS: u64 = 90;

fn validate(cfg: &Config) -> Result<()> {
    cfg.server
        .listen
        .parse::<std::net::SocketAddr>()
        .with_context(|| format!("server.listen must be an address like 0.0.0.0:8080, got {:?}", cfg.server.listen))?;
    anyhow::ensure!(
        cfg.logs.format == "json" || cfg.logs.format == "pretty",
        "logs.format must be \"json\" or \"pretty\", got {:?}",
        cfg.logs.format
    );
    anyhow::ensure!((1..=720).contains(&cfg.auth.session_hours), "auth.session_hours must be 1 to 720");
    anyhow::ensure!((1..=64).contains(&cfg.runs.max_concurrent), "runs.max_concurrent must be 1 to 64");
    anyhow::ensure!(
        (1..=MAX_AUDIT_RETENTION_DAYS).contains(&cfg.audit.retention_days),
        "audit.retention_days must be 1 to {MAX_AUDIT_RETENTION_DAYS}"
    );
    let e = &cfg.email;
    anyhow::ensure!(
        ["starttls", "tls", "none"].contains(&e.security.as_str()),
        "email.security must be \"starttls\", \"tls\" or \"none\", got {:?}",
        e.security
    );
    if e.enabled() {
        let from = e.from.as_deref().context("email.from is required when email.smtp_host is set, like \"OpSkills <opskills@example.com>\"")?;
        from.parse::<lettre::message::Mailbox>().map_err(|err| anyhow::anyhow!("email.from {from:?} is not an address: {err}"))?;
        anyhow::ensure!(e.username.is_some() == e.password.is_some(), "set both email.username and email.password, or neither");
    }
    Ok(())
}

/// Remove every OPSKILLS_* variable from this process's environment.
pub fn scrub_environment() {
    let keys: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| k.starts_with("OPSKILLS_"))
        .collect();
    for key in keys {
        // SAFETY: called from main before the async runtime or any other thread starts.
        unsafe { std::env::remove_var(key) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let cfg = Config::default();
        validate(&cfg).unwrap();
        assert_eq!(cfg.server.listen, "0.0.0.0:8080");
    }

    #[test]
    fn env_overrides_file_values() {
        let mut cfg: Config = toml::from_str("[server]\nlisten = \"127.0.0.1:9000\"\n").unwrap();
        apply_env(&mut cfg, |k| match k {
            "OPSKILLS_SERVER__LISTEN" => Some("0.0.0.0:7000".into()),
            "OPSKILLS_LOGS__FORMAT" => Some("pretty".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.server.listen, "0.0.0.0:7000");
        assert_eq!(cfg.logs.format, "pretty");
    }

    #[test]
    fn email_settings_are_checked_and_the_password_is_never_printed() {
        let mut cfg = Config::default();
        apply_env(&mut cfg, |k| match k {
            "OPSKILLS_EMAIL__SMTP_HOST" => Some("smtp.example.com".into()),
            "OPSKILLS_EMAIL__USERNAME" => Some("ops".into()),
            "OPSKILLS_EMAIL__PASSWORD" => Some("hunter2-smtp".into()),
            _ => None,
        })
        .unwrap();
        assert!(validate(&cfg).is_err(), "from is required");
        cfg.email.from = Some("OpSkills <ops@example.com>".into());
        validate(&cfg).unwrap();
        assert!(cfg.email.enabled());
        let printed = cfg.to_toml();
        assert!(printed.contains("smtp.example.com") && !printed.contains("hunter2-smtp"), "{printed}");
        cfg.email.security = "ssl".into();
        assert!(validate(&cfg).is_err());
        cfg.email.security = "tls".into();
        cfg.email.password = None;
        assert!(validate(&cfg).is_err(), "username without password");
        assert!(!Config::default().email.enabled());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("[server]\nlistn = \"x\"\n").is_err());
    }

    #[test]
    fn bad_values_are_rejected() {
        let mut cfg = Config::default();
        cfg.logs.format = "xml".into();
        assert!(validate(&cfg).is_err());
        cfg = Config::default();
        cfg.server.listen = "8080".into();
        assert!(validate(&cfg).is_err());
        cfg = Config::default();
        cfg.audit.retention_days = 91;
        assert!(validate(&cfg).unwrap_err().to_string().contains("1 to 90"));
        cfg.audit.retention_days = 30;
        assert!(validate(&cfg).is_ok());
    }
}
