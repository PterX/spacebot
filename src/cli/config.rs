//! `spacebot config` — global settings and raw config.toml access over the
//! control API.

use super::client::{self, ApiClient};
use super::output;
use anyhow::Context as _;
use clap::Subcommand;
use spacebot::api::settings::{
    GlobalSettingsResponse, GlobalSettingsUpdateResponse, RawConfigResponse,
    RawConfigUpdateResponse,
};

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show global settings
    Get,
    /// Update global settings from key=value pairs or a JSON file
    Set {
        /// key=value pairs; dotted keys reach nested settings
        /// (e.g. api_port=19898 opencode.enabled=true)
        #[arg(
            value_name = "KEY=VALUE",
            required_unless_present = "file",
            conflicts_with = "file"
        )]
        pairs: Vec<String>,
        /// Read the update as JSON from a file
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
    },
    /// Raw config.toml access
    #[command(subcommand)]
    Raw(RawConfigCommand),
}

#[derive(Subcommand)]
pub enum RawConfigCommand {
    /// Print the raw config.toml content
    Get,
    /// Replace config.toml (validated by the daemon before writing)
    Set {
        /// Read content from a file instead of stdin
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
    },
}

pub async fn run(ctx: &super::Context, cmd: ConfigCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match cmd {
        ConfigCommand::Get => {
            let value = client.get("settings").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let settings: GlobalSettingsResponse = client::parse(value)?;
            println!("Company name:     {}", settings.company_name);
            println!(
                "Brave search key: {}",
                if settings.brave_search_key.is_some() {
                    "(set)"
                } else {
                    "(not set)"
                }
            );
            println!("API enabled:      {}", settings.api_enabled);
            println!("API bind:         {}", settings.api_bind);
            println!("API port:         {}", settings.api_port);
            println!("Worker log mode:  {}", settings.worker_log_mode);
            println!("SSH enabled:      {}", settings.ssh_enabled);
            println!("OpenCode:");
            println!("  Enabled:             {}", settings.opencode.enabled);
            println!("  Path:                {}", settings.opencode.path);
            println!("  Max servers:         {}", settings.opencode.max_servers);
            println!(
                "  Startup timeout (s): {}",
                settings.opencode.server_startup_timeout_secs
            );
            println!(
                "  Max restart retries: {}",
                settings.opencode.max_restart_retries
            );
            println!(
                "  Permissions:         edit={} bash={} webfetch={}",
                settings.opencode.permissions.edit,
                settings.opencode.permissions.bash,
                settings.opencode.permissions.webfetch,
            );
            Ok(())
        }
        ConfigCommand::Set { pairs, file } => {
            let body = match &file {
                Some(path) => {
                    let content = std::fs::read_to_string(path)
                        .with_context(|| format!("failed to read {}", path.display()))?;
                    serde_json::from_str(&content).context("failed to parse update file as JSON")?
                }
                None => {
                    let mut body = serde_json::json!({});
                    for pair in &pairs {
                        let (key, raw) = pair
                            .split_once('=')
                            .with_context(|| format!("expected key=value, got '{pair}'"))?;
                        set_path(&mut body, key, parse_scalar(raw))?;
                    }
                    body
                }
            };

            let value = client.put("settings", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: GlobalSettingsUpdateResponse = client::parse(value)?;
            if !result.success {
                anyhow::bail!("{}", result.message);
            }
            eprintln!("{}", result.message);
            Ok(())
        }
        ConfigCommand::Raw(RawConfigCommand::Get) => {
            let value = client.get("settings/raw").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: RawConfigResponse = client::parse(value)?;
            print!("{}", response.content);
            if !response.content.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        ConfigCommand::Raw(RawConfigCommand::Set { file }) => {
            let content = match &file {
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("failed to read {}", path.display()))?,
                None => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                }
            };

            let value = client
                .put("settings/raw", &serde_json::json!({ "content": content }))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: RawConfigUpdateResponse = client::parse(value)?;
            if !result.success {
                anyhow::bail!("{}", result.message);
            }
            eprintln!("{}", result.message);
            Ok(())
        }
    }
}

/// Interpret a CLI value: JSON booleans, numbers, and null pass through
/// typed; everything else is a string.
fn parse_scalar(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(
            value @ (serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::Null),
        ) => value,
        _ => serde_json::Value::String(raw.to_string()),
    }
}

/// Assign `value` at a dotted key path, creating intermediate objects.
fn set_path(
    body: &mut serde_json::Value,
    key: &str,
    value: serde_json::Value,
) -> anyhow::Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|part| part.is_empty()) {
        anyhow::bail!("invalid settings key '{key}'");
    }
    let mut current = body;
    for part in &parts[..parts.len() - 1] {
        if !current.get(*part).is_some_and(serde_json::Value::is_object) {
            current[*part] = serde_json::json!({});
        }
        current = current.get_mut(*part).expect("entry ensured above");
    }
    current[*parts.last().expect("split yields at least one part")] = value;
    Ok(())
}
