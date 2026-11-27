//! Schedules: when a runbook runs by itself.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};


use super::*;

/// When a runbook runs by itself: repeating on a cron expression in a time zone, or
/// once at a set time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// Stable id; the server assigns one when it's empty.
    #[serde(default)]
    pub id: String,
    /// A short label, shown on runs it starts.
    #[serde(default)]
    pub name: String,
    /// Repeating: 5 fields (minute hour day-of-month month day-of-week) or `@daily`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Once: when to run, in epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// IANA time zone the cron expression is read in.
    #[serde(default = "utc")]
    pub timezone: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Values for the runbook's inputs. Secret inputs can't be scheduled.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

fn utc() -> String {
    "UTC".into()
}

pub const MAX_SCHEDULES: usize = 10;

impl Schedule {
    pub(super) fn validate(&mut self, defs: &[InputDef]) -> Result<(), String> {
        self.name = self.name.trim().to_string();
        if self.name.chars().count() > 60 {
            return Err("a schedule name must be at most 60 characters.".into());
        }
        if self.id.is_empty() {
            // Random, not time-based: several schedules are created in the same millisecond.
            self.id = hex::encode(crate::firststart::random_bytes::<6>());
        }
        self.timezone = self.timezone.trim().to_string();
        crate::schedule::parse_tz(&self.timezone)?;
        match (&self.cron, self.at) {
            (Some(c), None) => {
                crate::schedule::parse_cron(c)?;
                self.cron = Some(c.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            (None, Some(at)) => {
                // A one-time schedule that's still waiting must be in the future.
                if self.enabled && at <= now_ms() {
                    return Err("a one-time schedule must be in the future.".into());
                }
            }
            _ => return Err("give either a cron expression or a one-time date, not both.".into()),
        }
        if let Some(d) = defs.iter().find(|d| d.kind == InputKind::Secret && d.required && d.default.is_none()) {
            return Err(format!("the runbook has a required secret input, {}, and schedules can't hold secrets.", d.name));
        }
        if let Some(k) = self.inputs.keys().find(|k| defs.iter().any(|d| &d.name == *k && d.kind == InputKind::Secret)) {
            return Err(format!("{k} is a secret input; schedules can't hold secrets."));
        }
        let given = self.inputs.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
        resolve_inputs(defs, &given)?;
        Ok(())
    }
}
