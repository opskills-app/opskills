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
    Ok(())
}

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
    }
}
