//! CLI command tree and dispatch.
//!
//! Resource commands are HTTP clients of the running daemon's control API,
//! authenticated with the bearer token from config. Lifecycle commands
//! (start/stop/restart/status) talk to the daemon socket and stay in
//! `main.rs` alongside the daemon entry point.

mod auth;
mod client;
mod output;
mod secrets;
mod skill;

use anyhow::Context as _;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "spacebot", version)]
#[command(about = "A Rust agentic system with dedicated processes for every task")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to config file (optional)
    #[arg(short, long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Enable debug logging
    #[arg(short, long, global = true)]
    pub debug: bool,

    /// Print raw API responses as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Base URL of a spacebot instance (defaults to the local daemon)
    #[arg(long, global = true, env = "SPACEBOT_URL")]
    pub url: Option<String>,

    /// API auth token (defaults to api.auth_token from config)
    #[arg(long, global = true, env = "SPACEBOT_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon (default when no subcommand is given)
    Start {
        /// Run in the foreground instead of daemonizing
        #[arg(short, long)]
        foreground: bool,
    },
    /// Stop the running daemon
    Stop,
    /// Restart the daemon (stop + start)
    Restart {
        /// Run in the foreground instead of daemonizing
        #[arg(short, long)]
        foreground: bool,
    },
    /// Show status of the running daemon
    Status,
    /// Manage skills
    #[command(subcommand)]
    Skill(skill::SkillCommand),
    /// Manage authentication
    #[command(subcommand)]
    Auth(auth::AuthCommand),
    /// Manage secrets stored in the running instance
    #[command(subcommand)]
    Secrets(secrets::SecretsCommand),
}

/// Global options shared by every resource command.
pub struct Context {
    pub config_path: Option<std::path::PathBuf>,
    pub json: bool,
    pub url: Option<String>,
    pub token: Option<String>,
}

/// Run a resource command. Lifecycle commands are dispatched in `main.rs`
/// before this is reached.
pub fn dispatch(command: Command, ctx: Context) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    runtime.block_on(async {
        match command {
            Command::Skill(cmd) => skill::run(&ctx, cmd).await,
            Command::Auth(cmd) => auth::run(&ctx, cmd).await,
            Command::Secrets(cmd) => secrets::run(&ctx, cmd).await,
            Command::Start { .. } | Command::Stop | Command::Restart { .. } | Command::Status => {
                unreachable!("lifecycle commands are dispatched in main")
            }
        }
    })
}

pub fn load_config(
    config_path: &Option<std::path::PathBuf>,
) -> anyhow::Result<spacebot::config::Config> {
    if let Some(path) = config_path {
        spacebot::config::Config::load_from_path(path)
            .with_context(|| format!("failed to load config from {}", path.display()))
    } else {
        spacebot::config::Config::load().with_context(|| "failed to load configuration")
    }
}
