//! Text helpers for runbooks: duration strings, shell quoting and step templates.
//!
//! A step command can refer to run values with `{{ inputs.name }}`, `{{ host.name }}`,
//! `{{ host.id }}`, `{{ host.attr.<key> }}` (empty when the host lacks that attribute),
//! `{{ run.id }}`, `{{ run.started_by }}`, `{{ runbook.id }}` and `{{ runbook.name }}`. Each one becomes a single, already quoted shell word, so a
//! value can never turn into extra commands. Text that doesn't match that exact
//! shape, such as `{{.Names}}` in a `docker ps --format` argument, is left alone.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;

/// Seconds in a duration such as `90`, `"90"`, `"90s"`, `"10m"`, `"2h"`, `"1d"` or `"1h30m"`.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let bad = || format!("{text:?} is not a duration. Use seconds, or a number with s, m, h or d, like 90s, 10m, 2h or 1h30m.");
    if t.is_empty() {
        return Err(bad());
    }
    if let Ok(n) = t.parse::<u64>() {
        return Ok(n);
    }
    let mut total: u64 = 0;
    let mut number = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return Err(bad()),
        };
        let n: u64 = number.parse().map_err(|_| bad())?;
        total = n.checked_mul(unit).and_then(|v| total.checked_add(v)).ok_or_else(bad)?;
        number.clear();
    }
    if !number.is_empty() {
        return Err(bad());
    }
    Ok(total)
}

/// Accepts a number of seconds or a duration string in JSON.
pub mod seconds {
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        Text(String),
    }

    fn convert<E: serde::de::Error>(raw: Raw) -> Result<u64, E> {
        match raw {
            Raw::Number(n) => Ok(n),
            Raw::Text(t) => super::parse_duration(&t).map_err(E::custom),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        convert(Raw::deserialize(d)?)
    }

    pub mod option {
        use super::*;

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
            match Option::<Raw>::deserialize(d)? {
                Some(raw) => convert(raw).map(Some),
                None => Ok(None),
            }
        }
    }
}

/// Quote `value` as one POSIX shell word.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*(inputs|host|run|runbook)\.(attr\.[A-Za-z0-9._-]+|[A-Za-z0-9_]+)\s*\}\}").unwrap())
}

/// Every `(scope, name)` a command refers to, in order of appearance.
pub fn references(command: &str) -> Vec<(String, String)> {
    pattern().captures_iter(command).map(|c| (c[1].to_string(), c[2].to_string())).collect()
}

/// Names each scope offers besides `inputs`.
pub const BUILT_IN: &[(&str, &[&str])] = &[("host", &["id", "name"]), ("run", &["id", "started_by"]), ("runbook", &["id", "name"])];

/// Check that every reference in `command` names a known value.
pub fn check(command: &str, inputs: &[String]) -> Result<(), String> {
    for (scope, name) in references(command) {
        let known = match scope.as_str() {
            "inputs" => inputs.contains(&name),
            // Any attribute; a host without it gets an empty value.
            "host" if name.strip_prefix("attr.").is_some_and(|k| crate::model::validate_attribute_key(k).is_ok()) => true,
            _ => BUILT_IN.iter().any(|(s, names)| *s == scope && names.contains(&name.as_str())),
        };
        if !known {
            let hint = if scope == "inputs" { "Add an input with that name or fix the spelling." } else { "See the list of run values in the docs." };
            return Err(format!("{{{{ {scope}.{name} }}}} is not a known value. {hint}"));
        }
    }
    Ok(())
}

/// A reference's value: `host.attr.<key>` is empty for a host without that attribute.
fn lookup(values: &BTreeMap<String, String>, scope: &str, name: &str) -> Option<String> {
    let key = format!("{scope}.{name}");
    let attr = scope == "host" && name.strip_prefix("attr.").is_some_and(|k| crate::model::validate_attribute_key(k).is_ok());
    values.get(&key).cloned().or_else(|| attr.then(String::new))
}

/// Replace every reference with its quoted value. `values` is keyed by `scope.name`.
pub fn expand(command: &str, values: &BTreeMap<String, String>) -> String {
    pattern()
        .replace_all(command, |c: &regex::Captures| match lookup(values, &c[1], &c[2]) {
            Some(v) => shell_quote(&v),
            None => c[0].to_string(),
        })
        .into_owned()
}

/// Replace every reference with its value as plain text, for file content and the
/// inputs of a called runbook. Never use this for a shell command.
pub fn expand_plain(text: &str, values: &BTreeMap<String, String>) -> String {
    pattern()
        .replace_all(text, |c: &regex::Captures| lookup(values, &c[1], &c[2]).unwrap_or_else(|| c[0].to_string()))
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_attributes_expand_and_missing_ones_are_empty() {
        let mut v = BTreeMap::new();
        v.insert("host.attr.env".to_string(), "prod's".to_string());
        assert_eq!(expand("deploy {{ host.attr.env }} [{{host.attr.nope}}]", &v), "deploy 'prod'\\''s' ['']");
        assert_eq!(expand_plain("env={{ host.attr.env }}", &v), "env=prod's");
        assert!(check("{{ host.attr.region }}", &[]).is_ok());
        assert!(check("{{ host.attr.Region }}", &[]).is_err(), "attribute names are lowercase");
        assert!(check("{{ host.nope }}", &[]).is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert_eq!(parse_duration("90s").unwrap(), 90);
        assert_eq!(parse_duration("10m").unwrap(), 600);
        assert_eq!(parse_duration(" 1h30m ").unwrap(), 5400);
        assert_eq!(parse_duration("2d").unwrap(), 172_800);
        for bad in ["", "m", "10x", "1.5h", "-3", "10m5"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn quoting_keeps_values_as_one_word() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's; rm -rf /"), r"'it'\''s; rm -rf /'");
    }

    #[test]
    fn templates_expand_known_values_only() {
        let mut v = BTreeMap::new();
        v.insert("inputs.service".to_string(), "nginx; reboot".to_string());
        v.insert("host.name".to_string(), "web-1".to_string());
        let out = expand("systemctl restart {{ inputs.service }} on {{host.name}} {{.Names}} {{ other.x }}", &v);
        assert_eq!(out, "systemctl restart 'nginx; reboot' on 'web-1' {{.Names}} {{ other.x }}");
        assert_eq!(expand_plain("server_name {{ host.name }};", &v), "server_name web-1;");
        assert!(check("echo {{ inputs.service }} {{ run.id }}", &["service".into()]).is_ok());
        assert!(check("echo {{ inputs.missing }}", &["service".into()]).is_err());
        assert!(check("echo {{ host.address }}", &[]).is_err());
        assert!(check("docker ps --format '{{.Names}}'", &[]).is_ok());
    }
}
