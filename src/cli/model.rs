//! `spacebot model` — model catalog access over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::models::ModelsResponse;

#[derive(Subcommand)]
pub enum ModelCommand {
    /// List models available to the configured providers
    List {
        /// Filter by provider ID (anthropic, openrouter, ...)
        #[arg(short, long)]
        provider: Option<String>,
        /// Filter by capability (input_audio, voice_transcription)
        #[arg(short, long)]
        capability: Option<String>,
    },
    /// Clear the model catalog cache and refetch it
    Refresh,
}

pub async fn run(ctx: &super::Context, model_cmd: ModelCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match model_cmd {
        ModelCommand::List {
            provider,
            capability,
        } => {
            let mut query = Vec::new();
            if let Some(provider) = &provider {
                query.push(format!("provider={}", urlencoding::encode(provider)));
            }
            if let Some(capability) = &capability {
                query.push(format!("capability={}", urlencoding::encode(capability)));
            }
            let path = if query.is_empty() {
                "models".to_string()
            } else {
                format!("models?{}", query.join("&"))
            };

            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ModelsResponse = client::parse(value)?;
            if response.models.is_empty() {
                eprintln!("No models available — configure a provider first.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .models
                .iter()
                .map(|model| {
                    vec![
                        model.id.clone(),
                        model.name.clone(),
                        model
                            .context_window
                            .map(|window| window.to_string())
                            .unwrap_or_else(|| "-".into()),
                        yes_no(model.tool_call),
                        yes_no(model.reasoning),
                        yes_no(model.input_audio),
                    ]
                })
                .collect();
            output::table(
                &["ID", "NAME", "CONTEXT", "TOOLS", "REASONING", "AUDIO"],
                &rows,
            );
            Ok(())
        }
        ModelCommand::Refresh => {
            let value = client
                .post("models/refresh", &serde_json::json!({}))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ModelsResponse = client::parse(value)?;
            eprintln!(
                "Model catalog refreshed ({} models).",
                response.models.len()
            );
            Ok(())
        }
    }
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}
