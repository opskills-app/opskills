//! Inbound webhooks and their deliveries.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};


use super::*;

/// An inbound webhook: a secret URL that starts a run of one runbook.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub runbook_id: String,
    pub name: String,
    pub enabled: bool,
    /// Runbook input name to a dotted path in the JSON body, like `repository.name`.
    pub inputs: BTreeMap<String, String>,
    /// Calls must carry an HMAC-SHA256 signature of the body.
    pub signed: bool,
    pub created_at: u64,
    /// The latest deliveries, newest first.
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

pub const MAX_DELIVERIES: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delivery {
    pub at: u64,
    /// The HTTP status the caller got.
    pub status: u16,
    pub run_id: Option<String>,
    pub error: Option<String>,
}

/// What the store keeps: the webhook, the SHA-256 of its token's secret, and the
/// signing secret sealed with the vault key (the server needs it to check signatures).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredWebhook {
    #[serde(flatten)]
    pub hook: Webhook,
    pub token_hash: String,
    #[serde(default)]
    pub signing_nonce: Option<String>,
    #[serde(default)]
    pub signing_ciphertext: Option<String>,
}

/// Check a webhook's input mapping against the runbook's inputs.
pub fn validate_hook_inputs(map: &BTreeMap<String, String>, defs: &[InputDef]) -> Result<(), String> {
    for (name, path) in map {
        if !defs.iter().any(|d| &d.name == name) {
            return Err(format!("The runbook has no input named {name}."));
        }
        let ok = !path.is_empty()
            && path.len() <= 200
            && path.split('.').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        if !ok {
            return Err(format!("{path:?} for {name} is not a path. Use names and array positions separated by dots, like repository.name or commits.0.id."));
        }
    }
    Ok(())
}

/// The value at a dotted path in a JSON document: strings as they are, other values as JSON.
pub fn json_path(doc: &serde_json::Value, path: &str) -> Option<String> {
    let mut v = doc;
    for seg in path.split('.') {
        v = match v {
            serde_json::Value::Object(m) => m.get(seg)?,
            serde_json::Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}
