//! Host filter expressions, like `tags:web,api !name:db-* os:debian*`.
//!
//! A filter is a list of terms separated by spaces, and a host must match every term.
//! A term is `key:value`, with several values separated by commas (any of them may
//! match), `*` matching any run of characters and `?` any one character. A leading
//! `!` turns a term around. A term without a key matches the host's name. Matching
//! ignores case. Keys: `tags`, `name`, `hostname`, `os`, `arch`, and any attribute, like
//! `env:prod`; a host without the attribute doesn't match.

use crate::model::Host;

const KEYS: &[&str] = &["tags", "name", "hostname", "os", "arch"];
pub const MAX_FILTER_LEN: usize = 500;
const MAX_TERMS: usize = 20;

#[derive(Clone, Debug, PartialEq)]
struct Term {
    negate: bool,
    key: String,
    /// Lower-case glob patterns; the term matches when any of them does.
    values: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostFilter {
    terms: Vec<Term>,
}

impl HostFilter {
    pub fn parse(text: &str) -> Result<HostFilter, String> {
        if text.len() > MAX_FILTER_LEN {
            return Err(format!("A host filter must be at most {MAX_FILTER_LEN} characters."));
        }
        let mut terms = Vec::new();
        for word in text.split_whitespace() {
            let (negate, rest) = match word.strip_prefix('!') {
                Some(r) => (true, r),
                None => (false, word),
            };
            let (key, values) = match rest.split_once(':') {
                Some((k, v)) => {
                    let k = k.to_ascii_lowercase();
                    if !KEYS.contains(&k.as_str()) {
                        crate::model::validate_attribute_key(&k).map_err(|_| format!("{k:?} is not a host filter key. Use {}, or an attribute name like env.", KEYS.join(", ")))?;
                    }
                    (k, v)
                }
                None => ("name".to_string(), rest),
            };
            let values: Vec<String> = values.split(',').map(|v| v.trim().to_lowercase()).collect();
            if values.iter().any(String::is_empty) {
                return Err(format!("{word:?} needs a value after the colon, like tags:web or name:web-*."));
            }
            terms.push(Term { negate, key, values });
        }
        if terms.len() > MAX_TERMS {
            return Err(format!("A host filter can have at most {MAX_TERMS} terms."));
        }
        Ok(HostFilter { terms })
    }

    /// Whether `host` matches every term. An empty filter matches nothing.
    pub fn matches(&self, host: &Host) -> bool {
        !self.terms.is_empty() && self.terms.iter().all(|t| t.matches(host) != t.negate)
    }
}

impl Term {
    fn matches(&self, host: &Host) -> bool {
        let any = |field: &str| {
            let field = field.to_lowercase();
            self.values.iter().any(|v| glob(v, &field))
        };
        match self.key.as_str() {
            "tags" => host.tags.iter().any(|t| any(t)),
            "name" => any(&host.name),
            "hostname" => any(&host.hostname),
            "os" => any(&host.os),
            "arch" => any(&host.arch),
            attr => host.attributes.get(attr).is_some_and(|v| any(v)),
        }
    }
}

/// Whether `text` matches `pattern`, where `*` is any run of characters and `?` one.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    // Where the last `*` was, and how much of the text it has taken so far.
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, os: &str, arch: &str, tags: &[&str]) -> Host {
        Host {
            id: name.into(),
            name: name.into(),
            workspaces: vec!["default".into()],
            hostname: format!("{name}.internal"),
            os: os.into(),
            arch: arch.into(),
            agent_version: "0.5.0".into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            enrolled_at: 0,
            last_seen: 0,
            attributes: [("env".to_string(), if name.starts_with("db") { "prod" } else { "staging" }.to_string())].into(),
        }
    }

    #[test]
    fn attributes_can_be_filtered_on() {
        let hosts = [host("web-1", "Debian", "amd64", &[]), host("db-1", "Debian", "amd64", &[])];
        let pick = |f: &str| hosts.iter().filter(|h| HostFilter::parse(f).unwrap().matches(h)).map(|h| h.name.as_str()).collect::<Vec<_>>();
        assert_eq!(pick("env:prod"), ["db-1"]);
        assert_eq!(pick("env:stag*"), ["web-1"]);
        assert_eq!(pick("!env:prod"), ["web-1"]);
        assert!(pick("region:eu").is_empty(), "a host without the attribute doesn't match");
        assert_eq!(pick("!region:eu"), ["web-1", "db-1"]);
    }

    #[test]
    fn globs() {
        assert!(glob("web-*", "web-12"));
        assert!(glob("*-db", "eu-db"));
        assert!(glob("w?b", "web"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "axxbyyc"));
        assert!(!glob("web-*", "db-1"));
        assert!(!glob("w?b", "weeb"));
    }

    #[test]
    fn filters_match_every_term() {
        let web1 = host("web-1", "Debian GNU/Linux 13", "arm64", &["web", "prod"]);
        let web2 = host("web-2", "Rocky Linux 9", "amd64", &["web", "staging"]);
        let db1 = host("DB-1", "Debian GNU/Linux 13", "amd64", &["db", "prod"]);
        let pick = |f: &str| {
            let filter = HostFilter::parse(f).unwrap();
            [&web1, &web2, &db1].iter().filter(|h| filter.matches(h)).map(|h| h.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(pick("tags:web"), vec!["web-1", "web-2"]);
        assert_eq!(pick("tags:web tags:prod"), vec!["web-1"]);
        assert_eq!(pick("tags:web,db !tags:staging"), vec!["web-1", "DB-1"]);
        assert_eq!(pick("os:debian*"), vec!["web-1", "DB-1"]);
        assert_eq!(pick("db-*"), vec!["DB-1"], "a bare term matches the name, ignoring case");
        assert_eq!(pick("arch:amd64 !name:web-*"), vec!["DB-1"]);
        assert_eq!(pick("hostname:*.internal"), vec!["web-1", "web-2", "DB-1"]);
        assert!(pick("").is_empty(), "an empty filter matches nothing");
    }

    #[test]
    fn bad_filters_are_explained() {
        for bad in ["-color:blue", "c!olor:x", "tags:", "env:", "tags:web,", &"x ".repeat(21), &"a".repeat(501)] {
            assert!(HostFilter::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(HostFilter::parse("  ").unwrap(), HostFilter::default());
    }
}
