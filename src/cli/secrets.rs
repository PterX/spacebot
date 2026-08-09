//! `spacebot secrets` — secret store management over the control API.

use super::client::{self, ApiClient};
use super::output;
use anyhow::Context as _;
use clap::Subcommand;
use spacebot::api::secrets::{
    DeleteSecretResponse, EncryptResponse, MigrateResponse, PutSecretResponse, SecretInfoResponse,
    SecretListResponse,
};

#[derive(Subcommand)]
pub enum SecretsCommand {
    /// Show store state and secret counts
    Status,
    /// List all secrets (name + category)
    List,
    /// Add or update a secret
    Set {
        /// Secret name (e.g. GH_TOKEN)
        name: String,
        /// Secret category (system or tool)
        #[arg(long)]
        category: Option<String>,
        /// Read value from stdin instead of interactive prompt
        #[arg(long)]
        stdin: bool,
    },
    /// Delete a secret
    Delete {
        /// Secret name
        name: String,
    },
    /// Show secret metadata and config references
    Info {
        /// Secret name
        name: String,
    },
    /// Auto-migrate plaintext keys from config.toml
    Migrate,
    /// Enable encryption (generate master key, encrypt all secrets)
    Encrypt,
    /// Unlock encrypted store
    Unlock {
        /// Read master key from stdin instead of interactive prompt
        #[arg(long)]
        stdin: bool,
    },
    /// Lock encrypted store (clear key from memory)
    Lock,
    /// Rotate master key (encrypted mode only)
    Rotate,
    /// Export all secrets to a backup file
    Export {
        /// Output file path
        #[arg(short, long)]
        output: std::path::PathBuf,
    },
    /// Import secrets from a backup file
    Import {
        /// Input file path
        #[arg(short, long)]
        input: std::path::PathBuf,
        /// Overwrite existing secrets with same name
        #[arg(long)]
        overwrite: bool,
    },
}

pub async fn run(ctx: &super::Context, secrets_cmd: SecretsCommand) -> anyhow::Result<()> {
    // Bootstrap the secrets store so `secret:` references in config resolve
    // when the client loads config for the API base URL and token.
    crate::bootstrap_secrets_store(&ctx.config_path);

    let client = ApiClient::from_context(ctx)?;

    match secrets_cmd {
        SecretsCommand::Status => {
            let value = client.get("secrets/status").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            println!(
                "State:          {}",
                value["state"].as_str().unwrap_or("unknown")
            );
            println!(
                "Encrypted:      {}",
                value["encrypted"].as_bool().unwrap_or(false)
            );
            println!(
                "Total secrets:  {}",
                value["secret_count"].as_u64().unwrap_or(0)
            );
            println!(
                "  System:       {}",
                value["system_count"].as_u64().unwrap_or(0)
            );
            println!(
                "  Tool:         {}",
                value["tool_count"].as_u64().unwrap_or(0)
            );
            if value["platform_managed"].as_bool().unwrap_or(false) {
                println!("  Managed by:   platform");
            }
            Ok(())
        }
        SecretsCommand::List => {
            let value = client.get("secrets").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: SecretListResponse = client::parse(value)?;
            if response.secrets.is_empty() {
                eprintln!("No secrets stored.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .secrets
                .iter()
                .map(|secret| {
                    vec![
                        secret.name.clone(),
                        output::enum_label(&secret.category),
                        secret.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                    ]
                })
                .collect();
            output::table(&["NAME", "CATEGORY", "UPDATED"], &rows);
            Ok(())
        }
        SecretsCommand::Set {
            name,
            category,
            stdin,
        } => {
            let value = if stdin {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                buf.trim_end().to_string()
            } else {
                dialoguer::Password::new()
                    .with_prompt("Enter value")
                    .interact()
                    .context("failed to read secret value")?
            };

            if value.is_empty() {
                anyhow::bail!("secret value cannot be empty");
            }

            let mut body = serde_json::json!({ "value": value });
            if let Some(cat) = &category {
                body["category"] = serde_json::json!(cat);
            }

            let value = client
                .put(&format!("secrets/{}", client::encode_path(&name)), &body)
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: PutSecretResponse = client::parse(value)?;
            eprintln!(
                "Secret {} saved ({}).",
                result.name,
                output::enum_label(&result.category)
            );
            if result.reload_required {
                eprintln!("Note: Reload config or restart for the new value to take effect.");
            }
            Ok(())
        }
        SecretsCommand::Delete { name } => {
            let value = client
                .delete(&format!("secrets/{}", client::encode_path(&name)))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: DeleteSecretResponse = client::parse(value)?;
            eprintln!("Deleted {}.", result.deleted);
            if let Some(warning) = result.warning {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        SecretsCommand::Info { name } => {
            let value = client
                .get(&format!("secrets/{}/info", client::encode_path(&name)))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let info: SecretInfoResponse = client::parse(value)?;
            println!("Name:     {}", info.name);
            println!("Category: {}", output::enum_label(&info.category));
            println!("Created:  {}", info.created_at.format("%Y-%m-%d %H:%M:%S"));
            println!("Updated:  {}", info.updated_at.format("%Y-%m-%d %H:%M:%S"));
            Ok(())
        }
        SecretsCommand::Migrate => {
            let value = client
                .post("secrets/migrate", &serde_json::json!({}))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: MigrateResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            for item in result.migrated {
                eprintln!(
                    "  {} -> {} ({})",
                    item.config_key,
                    item.secret_name,
                    output::enum_label(&item.category),
                );
            }
            Ok(())
        }
        SecretsCommand::Encrypt => {
            let value = client
                .post("secrets/encrypt", &serde_json::json!({}))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: EncryptResponse = client::parse(value)?;
            eprintln!();
            eprintln!("Encryption enabled.");
            eprintln!();
            eprintln!("Master key: {}", result.master_key);
            eprintln!();
            eprintln!("IMPORTANT: Save this master key. You will need it to unlock");
            eprintln!("the secret store after a reboot (Linux) or if the Keychain");
            eprintln!("is reset (macOS). This is the only time the key will be shown.");
            Ok(())
        }
        SecretsCommand::Unlock { stdin } => {
            let master_key = if stdin {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                buf.trim().to_string()
            } else {
                dialoguer::Password::new()
                    .with_prompt("Enter master key")
                    .interact()
                    .context("failed to read master key")?
            };

            let value = client
                .post(
                    "secrets/unlock",
                    &serde_json::json!({ "master_key": master_key }),
                )
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            eprintln!("{}", value["message"].as_str().unwrap_or("Unlocked."));
            Ok(())
        }
        SecretsCommand::Lock => {
            let value = client.post("secrets/lock", &serde_json::json!({})).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            eprintln!("{}", value["message"].as_str().unwrap_or("Locked."));
            Ok(())
        }
        SecretsCommand::Rotate => {
            eprintln!("WARNING: This will invalidate your current master key.");
            eprintln!("You will need to save the new key for future unlocks.");
            eprint!("Continue? [y/N]: ");
            let mut confirm = String::new();
            std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut confirm)?;
            if !confirm.trim().eq_ignore_ascii_case("y") {
                eprintln!("Cancelled.");
                return Ok(());
            }

            let value = client
                .post("secrets/rotate", &serde_json::json!({}))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            eprintln!();
            eprintln!(
                "New master key: {}",
                value["master_key"].as_str().unwrap_or("")
            );
            eprintln!();
            eprintln!("IMPORTANT: Save this new key. Your old key no longer works.");
            Ok(())
        }
        SecretsCommand::Export {
            output: output_path,
        } => {
            let value = client
                .post("secrets/export", &serde_json::json!({}))
                .await?;
            if ctx.json {
                output::json(&value);
            }
            let count = value["count"].as_u64().unwrap_or(0);
            let content =
                serde_json::to_string_pretty(&value).context("failed to serialize export data")?;
            // Write with restrictive permissions — this file contains
            // plaintext secrets. mode() only applies on creation, so an
            // existing file is re-chmodded before the secrets are written.
            #[cfg(unix)]
            {
                use std::io::Write as _;
                use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&output_path)
                    .with_context(|| format!("failed to create {}", output_path.display()))?;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("failed to restrict {}", output_path.display()))?;
                file.write_all(content.as_bytes())
                    .with_context(|| format!("failed to write {}", output_path.display()))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::write(&output_path, content)
                    .with_context(|| format!("failed to write {}", output_path.display()))?;
            }
            eprintln!("Exported {count} secrets to {}", output_path.display());

            if let Some(warning) = value["warning"].as_str() {
                eprintln!();
                eprintln!("WARNING: {warning}");
            }
            Ok(())
        }
        SecretsCommand::Import { input, overwrite } => {
            let content = std::fs::read_to_string(&input)
                .with_context(|| format!("failed to read {}", input.display()))?;
            let mut import_data: serde_json::Value =
                serde_json::from_str(&content).context("failed to parse backup file as JSON")?;

            import_data["overwrite"] = serde_json::json!(overwrite);

            let value = client.post("secrets/import", &import_data).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            eprintln!(
                "{}",
                value["message"].as_str().unwrap_or("Import complete.")
            );
            if let Some(skipped) = value["skipped"].as_array()
                && !skipped.is_empty()
            {
                for name in skipped {
                    eprintln!(
                        "  {} -- kept existing (use --overwrite to replace)",
                        name.as_str().unwrap_or("")
                    );
                }
            }
            Ok(())
        }
    }
}
