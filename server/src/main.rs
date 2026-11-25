//! OpSkills server: serves the web app and the HTTP API from a single binary.

mod api;
mod auth;
mod config;
mod executor;
mod firststart;
mod model;
mod store;
mod vault;
mod web;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "opskills", version, about = "Runbooks your whole team can run")]
struct Cli {
    /// Path to the config file (default: /etc/opskills/opskills.toml if it exists)
    #[arg(long, global = true, env = "OPSKILLS_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server (the default)
    Serve,
    /// Exit 0 if the local server answers /healthz, 1 otherwise
    Healthcheck,
    /// Validate the config and print the effective settings
    Config,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = match config::load(cli.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("opskills: invalid configuration: {err:#}");
            return ExitCode::from(2);
        }
    };

    match cli.command.unwrap_or(Command::Serve) {
        Command::Config => {
            print!("{}", cfg.to_toml());
            ExitCode::SUCCESS
        }
        Command::Healthcheck => match healthcheck(&cfg) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("opskills: unhealthy: {err:#}");
                ExitCode::FAILURE
            }
        },
        Command::Serve => {
            // Remove OPSKILLS_* variables before any thread starts, so secrets passed
            // through the environment never reach processes the server runs later.
            config::scrub_environment();
            match run(cfg) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("opskills: {err:#}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run(cfg: config::Config) -> Result<()> {
    init_logging(&cfg);
    let state = firststart::prepare(&cfg).context("preparing the data directory")?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(web::serve(cfg, state))
}

fn init_logging(cfg: &config::Config) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_new(&cfg.logs.level).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    if cfg.logs.format == "pretty" {
        builder.init();
    } else {
        builder.json().flatten_event(true).init();
    }
}

/// Minimal HTTP/1.0 probe so the container image needs no curl or shell.
fn healthcheck(cfg: &config::Config) -> Result<()> {
    let listen: SocketAddr = cfg.server.listen.parse().context("parsing server.listen")?;
    let target = SocketAddr::from(([127, 0, 0, 1], listen.port()));
    let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(3)).with_context(|| format!("connecting to {target}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut buf = [0u8; 32];
    let n = stream.read(&mut buf)?;
    let head = String::from_utf8_lossy(&buf[..n]);
    if head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        anyhow::bail!("unexpected response: {}", head.lines().next().unwrap_or(""))
    }
}

#[cfg(test)]
mod tests;
