//! `spacebot provider` — LLM provider management over the control API.

use super::client::{self, ApiClient};
use super::output;
use anyhow::Context as _;
use clap::Subcommand;
use spacebot::api::providers::{
    ProviderConfigResponse, ProviderModelTestResponse, ProviderUpdateResponse,
};

#[derive(Subcommand)]
pub enum ProviderCommand {
    /// List providers and whether each is configured
    List,
    /// Configure a provider credential and apply a model to default routing
    Set {
        /// Provider name (anthropic, openai, openrouter, azure, ...)
        provider: String,
        /// Model routing string (e.g. anthropic/claude-sonnet-4-5)
        #[arg(short, long)]
        model: String,
        /// Read the API key from stdin instead of interactive prompt.
        /// For ollama the credential is the base URL; for azure an empty
        /// key keeps the stored one.
        #[arg(long)]
        stdin: bool,
        /// Azure resource base URL (…openai.azure.com)
        #[arg(long)]
        base_url: Option<String>,
        /// Azure API version (YYYY-MM-DD or YYYY-MM-DD-preview)
        #[arg(long)]
        api_version: Option<String>,
        /// Azure deployment name
        #[arg(long)]
        deployment: Option<String>,
    },
    /// Show a provider's stored configuration (credentials excluded)
    Get {
        /// Provider name
        provider: String,
    },
    /// Test a provider/model pair with a live completion
    Test {
        /// Provider name
        provider: String,
        /// Model routing string to test
        #[arg(short, long)]
        model: String,
        /// Read the API key from stdin instead of interactive prompt.
        /// For azure an empty key reuses the stored one.
        #[arg(long)]
        stdin: bool,
        /// Azure resource base URL (…openai.azure.com)
        #[arg(long)]
        base_url: Option<String>,
        /// Azure API version (YYYY-MM-DD or YYYY-MM-DD-preview)
        #[arg(long)]
        api_version: Option<String>,
        /// Azure deployment name
        #[arg(long)]
        deployment: Option<String>,
    },
    /// Remove a provider's credentials
    Delete {
        /// Provider name
        provider: String,
    },
}

pub async fn run(ctx: &super::Context, provider_cmd: ProviderCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match provider_cmd {
        ProviderCommand::List => {
            let value = client.get("providers").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            // The response is a struct with one bool field per provider;
            // iterating the JSON object keeps the table in sync as providers
            // are added. JSON keys use underscores, provider IDs hyphens.
            let providers = value["providers"]
                .as_object()
                .context("unexpected API response shape")?;
            let rows: Vec<Vec<String>> = providers
                .iter()
                .map(|(name, configured)| {
                    vec![
                        name.replace('_', "-"),
                        if configured.as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                    ]
                })
                .collect();
            output::table(&["PROVIDER", "CONFIGURED"], &rows);
            if !value["has_any"].as_bool().unwrap_or(false) {
                eprintln!();
                eprintln!("No providers configured.");
            }
            Ok(())
        }
        ProviderCommand::Set {
            provider,
            model,
            stdin,
            base_url,
            api_version,
            deployment,
        } => {
            let api_key = read_api_key(stdin)?;
            let mut body = serde_json::json!({
                "provider": provider,
                "api_key": api_key,
                "model": model,
            });
            set_optional(&mut body, "base_url", &base_url);
            set_optional(&mut body, "api_version", &api_version);
            set_optional(&mut body, "deployment", &deployment);

            let value = client.post("providers", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: ProviderUpdateResponse = client::parse(value)?;
            if !result.success {
                anyhow::bail!("{}", result.message);
            }
            eprintln!("{}", result.message);
            Ok(())
        }
        ProviderCommand::Get { provider } => {
            let path = format!("providers/{}/config", urlencoding::encode(provider.trim()));
            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let config: ProviderConfigResponse = client::parse(value)?;
            if !config.success {
                anyhow::bail!("{}", config.message);
            }
            if config.base_url.is_none()
                && config.api_version.is_none()
                && config.deployment.is_none()
            {
                println!("{}", config.message);
                return Ok(());
            }
            if let Some(base_url) = &config.base_url {
                println!("Base URL:     {base_url}");
            }
            if let Some(api_version) = &config.api_version {
                println!("API version:  {api_version}");
            }
            if let Some(deployment) = &config.deployment {
                println!("Deployment:   {deployment}");
            }
            Ok(())
        }
        ProviderCommand::Test {
            provider,
            model,
            stdin,
            base_url,
            api_version,
            deployment,
        } => {
            let api_key = read_api_key(stdin)?;
            let mut body = serde_json::json!({
                "provider": provider,
                "api_key": api_key,
                "model": model,
            });
            set_optional(&mut body, "base_url", &base_url);
            set_optional(&mut body, "api_version", &api_version);
            set_optional(&mut body, "deployment", &deployment);

            eprintln!("Testing {model}...");
            let value = client.post("providers/test-model", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: ProviderModelTestResponse = client::parse(value)?;
            if !result.success {
                anyhow::bail!("{}", result.message);
            }
            eprintln!("{}", result.message);
            if let Some(sample) = &result.sample {
                println!("{sample}");
            }
            Ok(())
        }
        ProviderCommand::Delete { provider } => {
            let path = format!("providers/{}", urlencoding::encode(provider.trim()));
            let value = client.delete(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: ProviderUpdateResponse = client::parse(value)?;
            if !result.success {
                anyhow::bail!("{}", result.message);
            }
            eprintln!("{}", result.message);
            Ok(())
        }
    }
}

/// Read a provider credential from stdin or an interactive prompt. Empty
/// values are passed through — the API accepts them for Azure key reuse and
/// rejects them elsewhere.
fn read_api_key(stdin: bool) -> anyhow::Result<String> {
    if stdin {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        Ok(buf.trim_end().to_string())
    } else {
        dialoguer::Password::new()
            .with_prompt("Enter API key")
            .allow_empty_password(true)
            .interact()
            .context("failed to read API key")
    }
}

fn set_optional(body: &mut serde_json::Value, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        body[key] = serde_json::json!(value);
    }
}
