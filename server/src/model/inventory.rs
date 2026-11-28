//! Inventory: host attributes, tag rules, and sources that list hosts (a static list, a
//! URL, or a program that prints them), all in one JSON format.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{normalize_tag, validate_secret_ref};
use crate::filter::HostFilter;

/// Filter keys a host has built in; attributes can't use them.
pub const BUILT_IN_KEYS: &[&str] = &["tags", "name", "hostname", "os", "arch"];
pub const MAX_ATTRIBUTES: usize = 50;
/// Most hosts one source can list.
pub const MAX_SOURCE_HOSTS: usize = 5000;
pub const MAX_SOURCES: usize = 50;
pub const MAX_TAG_RULES: usize = 100;
pub const DEFAULT_INTERVAL: u64 = 300;
pub const MIN_INTERVAL: u64 = 30;

/// An attribute name: 1 to 64 lowercase letters, digits, dots, dashes or underscores,
/// starting with a letter or digit, and not a built-in filter key.
pub fn validate_attribute_key(key: &str) -> Result<(), String> {
    let ok = !key.is_empty()
        && key.len() <= 64
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
        && key.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if !ok {
        return Err(format!("Attribute {key:?} must be 1 to 64 lowercase letters, digits, dots, dashes or underscores, like env or region."));
    }
    if BUILT_IN_KEYS.contains(&key) {
        return Err(format!("{key} is built into every host; use another attribute name."));
    }
    Ok(())
}

/// Check and tidy a host's attributes.
pub fn validate_attributes(attrs: &mut BTreeMap<String, String>) -> Result<(), String> {
    if attrs.len() > MAX_ATTRIBUTES {
        return Err(format!("A host can have at most {MAX_ATTRIBUTES} attributes."));
    }
    let mut out = BTreeMap::new();
    for (k, v) in std::mem::take(attrs) {
        let k = k.trim().to_lowercase();
        validate_attribute_key(&k)?;
        let v = v.trim().to_string();
        if v.chars().count() > 500 || v.contains(['\n', '\r', '\0']) {
            return Err(format!("The value of {k} must be one line of at most 500 characters."));
        }
        out.insert(k, v);
    }
    *attrs = out;
    Ok(())
}

/// Tags for every host a filter matches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagRule {
    pub filter: String,
    pub tags: Vec<String>,
}

pub fn validate_tag_rules(rules: &mut [TagRule]) -> Result<(), String> {
    if rules.len() > MAX_TAG_RULES {
        return Err(format!("At most {MAX_TAG_RULES} tag rules."));
    }
    for (i, r) in rules.iter_mut().enumerate() {
        r.filter = r.filter.split_whitespace().collect::<Vec<_>>().join(" ");
        if r.filter.is_empty() {
            return Err(format!("Tag rule {}: give a host filter, like os:debian*.", i + 1));
        }
        HostFilter::parse(&r.filter).map_err(|e| format!("Tag rule {}: {e}", i + 1))?;
        let mut tags = Vec::new();
        for t in &r.tags {
            if let Some(t) = normalize_tag(t).map_err(|e| format!("Tag rule {}: {e}", i + 1))?
                && !tags.contains(&t)
            {
                tags.push(t);
            }
        }
        if tags.is_empty() || tags.len() > 20 {
            return Err(format!("Tag rule {}: give 1 to 20 tags.", i + 1));
        }
        r.tags = tags;
    }
    Ok(())
}

/// A host as a source lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceHost {
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    /// How to reach it over SSH, when its source has SSH settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<super::HostSsh>,
}

impl SourceHost {
    fn validate(&mut self) -> Result<(), String> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() || self.name.chars().count() > 100 || self.name.chars().any(char::is_control) {
            return Err(format!("{:?} is not a host name: 1 to 100 characters on one line.", self.name));
        }
        self.hostname = self.hostname.trim().to_string();
        if self.hostname.chars().count() > 255 || self.hostname.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(format!("{}: the hostname must be one word of at most 255 characters.", self.name));
        }
        let mut tags = Vec::new();
        for t in &self.tags {
            if let Some(t) = normalize_tag(t).map_err(|e| format!("{}: {e}", self.name))?
                && !tags.contains(&t)
            {
                tags.push(t);
            }
        }
        if tags.len() > 20 {
            return Err(format!("{}: at most 20 tags.", self.name));
        }
        self.tags = tags;
        if let Some(ssh) = &mut self.ssh {
            ssh.validate().map_err(|e| format!("{}: {e}", self.name))?;
        }
        validate_attributes(&mut self.attributes).map_err(|e| format!("{}: {e}", self.name))
    }
}

/// The document a URL or program returns: `{"hosts": [...]}`.
#[derive(Deserialize)]
struct HostsDocument {
    hosts: Vec<SourceHost>,
}

/// Read and check a hosts document. Unknown fields are ignored, so documents can carry
/// more than OpSkills uses.
pub fn parse_hosts_document(bytes: &[u8]) -> Result<Vec<SourceHost>, String> {
    let doc: HostsDocument = serde_json::from_slice(bytes).map_err(|e| format!("the document isn't {{\"hosts\": [...]}} JSON: {e}"))?;
    validate_source_hosts(doc.hosts)
}

pub fn validate_source_hosts(mut hosts: Vec<SourceHost>) -> Result<Vec<SourceHost>, String> {
    if hosts.len() > MAX_SOURCE_HOSTS {
        return Err(format!("a source can list at most {MAX_SOURCE_HOSTS} hosts; this one listed {}.", hosts.len()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for h in &mut hosts {
        h.validate()?;
        if !seen.insert(h.name.to_lowercase()) {
            return Err(format!("{} is listed twice.", h.name));
        }
    }
    Ok(hosts)
}

/// Where a source gets its hosts.
#[derive(Clone, Debug, Serialize, Deserialize)]
// No deny_unknown_fields: it doesn't work with `flatten`; `check_source_fields` does it.
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceKind {
    /// Hosts written in OpSkills.
    Static { hosts: Vec<SourceHost> },
    /// A URL that returns a hosts document. `header` and `secret` add a header whose
    /// value comes from the vault, like an API key.
    Url {
        url: String,
        #[serde(default)]
        header: Option<String>,
        #[serde(default)]
        secret: Option<String>,
    },
    /// A program on the server that prints a hosts document: the inventory plugin
    /// protocol. Its settings arrive as `OPSKILLS_CONFIG_<NAME>`, its secrets under their
    /// own names.
    Program {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        config: BTreeMap<String, String>,
        #[serde(default)]
        secrets: Vec<String>,
    },
}

impl SourceKind {
    pub fn label(&self) -> &'static str {
        match self {
            SourceKind::Static { .. } => "static list",
            SourceKind::Url { .. } => "URL",
            SourceKind::Program { .. } => "program",
        }
    }

    pub fn validate(&mut self) -> Result<(), String> {
        match self {
            SourceKind::Static { hosts } => {
                *hosts = validate_source_hosts(std::mem::take(hosts))?;
            }
            SourceKind::Url { url, header, secret } => {
                *url = url.trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2000 || url.contains(char::is_whitespace) {
                    return Err("The URL must be an http or https address.".into());
                }
                *header = header.take().map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
                *secret = secret.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                match (header.as_deref(), secret.as_deref()) {
                    (Some(h), Some(s)) => {
                        if h.len() > 100 || !h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                            return Err(format!("{h:?} is not a header name, like Authorization or X-Api-Key."));
                        }
                        validate_secret_ref(s)?;
                    }
                    (None, None) => {}
                    _ => return Err("Give both a header name and the vault secret for its value, or neither.".into()),
                }
            }
            SourceKind::Program { command, args, config, secrets } => {
                *command = command.trim().to_string();
                if !command.starts_with('/') || command.len() > 500 || command.chars().any(char::is_control) {
                    return Err("The program must be an absolute path on the server, like /opt/opskills/plugins/ec2-inventory/run.".into());
                }
                if args.len() > 20 || args.iter().any(|a| a.len() > 500 || a.contains('\0')) {
                    return Err("At most 20 arguments of up to 500 characters.".into());
                }
                if config.len() > 50 {
                    return Err("At most 50 settings.".into());
                }
                for (k, v) in config.iter() {
                    let ok = !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                    if !ok {
                        return Err(format!("Setting {k:?} must be capital letters, digits and underscores; the program reads it as OPSKILLS_CONFIG_{k}."));
                    }
                    if v.len() > 2000 || v.contains('\0') {
                        return Err(format!("The value of {k} is too long."));
                    }
                }
                if secrets.len() > 20 {
                    return Err("At most 20 secrets.".into());
                }
                for s in secrets.iter() {
                    validate_secret_ref(s)?;
                }
            }
        }
        Ok(())
    }
}

/// A source of hosts in a workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InventorySource {
    pub id: String,
    pub workspace: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: SourceKind,
    /// Seconds between refreshes.
    pub interval_seconds: u64,
    pub enabled: bool,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    /// Reach its hosts that no agent enrolled over SSH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<super::SourceSsh>,
}

/// A source as the API takes it; `check_source_fields` refuses unknown fields first.
#[derive(Clone, Debug, Deserialize)]
pub struct SourceInput {
    pub name: String,
    #[serde(flatten)]
    pub kind: SourceKind,
    #[serde(default)]
    pub interval_seconds: Option<u64>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub ssh: Option<super::SourceSsh>,
}

/// Refuse fields a source of its type doesn't have, so a typo isn't silently ignored.
pub fn check_source_fields(v: &serde_json::Value) -> Result<(), String> {
    let obj = v.as_object().ok_or("An inventory source is a JSON object.")?;
    let kind_fields: &[&str] = match obj.get("type").and_then(|t| t.as_str()) {
        Some("static") => &["hosts"],
        Some("url") => &["url", "header", "secret"],
        Some("program") => &["command", "args", "config", "secrets"],
        Some(other) => return Err(format!("{other:?} is not a source type. Use static, url or program.")),
        None => return Err("Give the source's type: static, url or program.".into()),
    };
    for k in obj.keys() {
        if !["name", "type", "interval_seconds", "enabled", "ssh"].contains(&k.as_str()) && !kind_fields.contains(&k.as_str()) {
            return Err(format!("A {} source has no field {k:?}.", obj["type"].as_str().unwrap_or("")));
        }
    }
    Ok(())
}

pub fn validate_source_name(name: &str) -> Result<String, String> {
    let n = name.trim().to_string();
    if n.is_empty() || n.chars().count() > 60 { Err("A source name is 1 to 60 characters.".into()) } else { Ok(n) }
}

pub fn validate_interval(seconds: u64) -> Result<u64, String> {
    if (MIN_INTERVAL..=86_400).contains(&seconds) { Ok(seconds) } else { Err(format!("Refresh every {MIN_INTERVAL} seconds to 1 day.")) }
}

/// A host a source listed, with an id that stays the same across refreshes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRecord {
    pub id: String,
    #[serde(flatten)]
    pub host: SourceHost,
    pub first_seen: u64,
}

/// What the last refreshes of a source found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceState {
    pub last_attempt: Option<u64>,
    pub last_success: Option<u64>,
    /// Why the last refresh failed; the hosts are then from the last good one.
    pub error: Option<String>,
    pub hosts: Vec<SourceRecord>,
}
