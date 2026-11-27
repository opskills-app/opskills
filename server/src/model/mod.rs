//! Domain types shared by the API, the store and the executor.

use serde::{Deserialize, Serialize};

mod audit;
mod schedule;
mod notification;
mod runbook;
mod run;
mod host;
mod webhook;
mod token;
mod secret;
mod user;
mod workspace;
mod sign_in;

pub use audit::*;
pub use schedule::*;
pub use notification::*;
pub use runbook::*;
pub use run::*;
pub use host::*;
pub use webhook::*;
pub use token::*;
pub use secret::*;
pub use user::*;
pub use workspace::*;
pub use sign_in::*;

/// Serde default for flags that start out on.
fn yes() -> bool {
    true
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
}

impl Status {
    pub fn is_finished(self) -> bool {
        matches!(self, Status::Succeeded | Status::Failed | Status::Cancelled | Status::Skipped)
    }
}

#[cfg(test)]
mod tests;
