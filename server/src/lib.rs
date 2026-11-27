//! OpSkills server: serves the web app and the HTTP API from a single binary. Installed
//! as `opskills-agent`, the same binary is the agent for enrolled hosts.

pub mod agent;
pub mod api;
pub mod auth;
pub mod config;
pub mod executor;
pub mod filter;
pub mod firststart;
pub mod hub;
pub mod model;
pub mod notify;
pub mod schedule;
pub mod store;
pub mod template;
pub mod vault;
pub mod web;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where people using this server can get its source code (AGPL-3.0, section 13).
/// If you run a modified version for others, point this at your own source.
pub const SOURCE_URL: &str = "https://github.com/opskills-app/opskills";

#[cfg(test)]
mod tests;
