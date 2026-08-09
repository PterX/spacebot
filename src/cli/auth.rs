//! `spacebot auth` — Anthropic OAuth credential management.

use anyhow::Context as _;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Log in to Anthropic via OAuth (opens browser)
    Login {
        /// Use API console instead of Claude Pro/Max
        #[arg(long)]
        console: bool,
    },
    /// Show current auth status
    Status,
    /// Log out (remove stored credentials)
    Logout,
    /// Refresh the access token
    Refresh,
}

pub async fn run(ctx: &super::Context, auth_cmd: AuthCommand) -> anyhow::Result<()> {
    // We need the instance_dir for credential storage. Try loading config,
    // but fall back to the default instance dir if config doesn't exist yet
    // (auth login may be the first thing a user runs).
    let instance_dir = if let Ok(config) = super::load_config(&ctx.config_path) {
        config.instance_dir
    } else {
        spacebot::config::Config::default_instance_dir()
    };

    // Ensure instance dir exists
    std::fs::create_dir_all(&instance_dir)?;

    match auth_cmd {
        AuthCommand::Login { console } => {
            let mode = if console {
                spacebot::auth::AuthMode::Console
            } else {
                spacebot::auth::AuthMode::Max
            };
            spacebot::auth::login_interactive(&instance_dir, mode).await?;
            Ok(())
        }
        AuthCommand::Status => {
            match spacebot::auth::load_credentials(&instance_dir)? {
                Some(creds) => {
                    let expires_in = creds.expires_at - chrono::Utc::now().timestamp_millis();
                    let expires_min = expires_in / 60_000;
                    if creds.is_expired() {
                        eprintln!("Anthropic OAuth: expired ({}m ago)", -expires_min);
                    } else {
                        eprintln!("Anthropic OAuth: valid (expires in {}m)", expires_min);
                    }
                    eprintln!(
                        "  access token: <redacted> ({} bytes)",
                        creds.access_token.len()
                    );
                    eprintln!(
                        "  refresh token: <redacted> ({} bytes)",
                        creds.refresh_token.len()
                    );
                    eprintln!(
                        "  credentials file: {}",
                        spacebot::auth::credentials_path(&instance_dir).display()
                    );
                }
                None => {
                    eprintln!("No OAuth credentials found.");
                    eprintln!("Run `spacebot auth login` to authenticate.");
                }
            }
            Ok(())
        }
        AuthCommand::Logout => {
            let path = spacebot::auth::credentials_path(&instance_dir);
            if path.exists() {
                std::fs::remove_file(&path)?;
                eprintln!("Credentials removed.");
            } else {
                eprintln!("No credentials found.");
            }
            Ok(())
        }
        AuthCommand::Refresh => {
            let creds = spacebot::auth::load_credentials(&instance_dir)?
                .context("no credentials found — run `spacebot auth login` first")?;
            eprintln!("Refreshing access token...");
            let new_creds = creds.refresh().await.context("refresh failed")?;
            spacebot::auth::save_credentials(&instance_dir, &new_creds)?;
            let expires_min =
                (new_creds.expires_at - chrono::Utc::now().timestamp_millis()) / 60_000;
            eprintln!("Token refreshed (expires in {}m)", expires_min);
            Ok(())
        }
    }
}
