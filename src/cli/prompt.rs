//! `spacebot prompt` — read captured LLM requests from the terminal.
//!
//! The inspector's copy button hands out a `prompt show <id>` line, so this is
//! where a reference pasted into a shell or an agent session resolves.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::llm::record::{PromptRecord, PromptRequestSummary};

#[derive(Subcommand)]
pub enum PromptCommand {
    /// List captured requests, newest first
    List {
        /// Only this channel's requests
        #[arg(short, long)]
        channel: Option<String>,
        /// Only this branch or worker's requests
        #[arg(short, long)]
        process: Option<String>,
        /// Every request produced by one conversation message
        #[arg(short, long)]
        message: Option<String>,
        #[arg(short, long, default_value_t = 20)]
        limit: i64,
    },
    /// Show one captured request
    Show {
        /// Request id, or any unambiguous prefix of one
        request_id: String,
        /// Print the system prompt only, with no block annotations
        #[arg(long)]
        raw: bool,
    },
    /// Compare the block maps of two captured requests
    ///
    /// Reports which blocks changed rather than which bytes did, so a prompt
    /// that grew between two turns names the block responsible.
    Diff { first: String, second: String },
    /// Turn request capture on or off
    Capture {
        /// `on` or `off`
        state: String,
        /// Days of records to keep
        #[arg(long)]
        retain: Option<i64>,
    },
}

pub async fn run(ctx: &super::Context, command: PromptCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match command {
        PromptCommand::List {
            channel,
            process,
            message,
            limit,
        } => list(ctx, &client, channel, process, message, limit).await,
        PromptCommand::Show { request_id, raw } => show(ctx, &client, &request_id, raw).await,
        PromptCommand::Diff { first, second } => diff(ctx, &client, &first, &second).await,
        PromptCommand::Capture { state, retain } => capture(ctx, &client, &state, retain).await,
    }
}

async fn list(
    ctx: &super::Context,
    client: &ApiClient,
    channel: Option<String>,
    process: Option<String>,
    message: Option<String>,
    limit: i64,
) -> anyhow::Result<()> {
    let mut params = vec![format!("limit={limit}")];
    if let Some(channel) = &channel {
        params.push(format!("channel_id={}", urlencoding::encode(channel)));
    }
    if let Some(process) = &process {
        params.push(format!("process_id={}", urlencoding::encode(process)));
    }
    if let Some(message) = &message {
        params.push(format!("message_id={}", urlencoding::encode(message)));
    }

    let value = client.get(&format!("prompts?{}", params.join("&"))).await?;
    if ctx.json {
        output::json(&value);
        return Ok(());
    }

    let capture_on = value
        .get("capture_enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let requests: Vec<PromptRequestSummary> =
        client::parse(value.get("requests").cloned().unwrap_or_default())?;

    if requests.is_empty() {
        eprintln!(
            "{}",
            if capture_on {
                "No captured requests match."
            } else {
                "Prompt capture is off. Turn it on with `spacebot prompt capture on`."
            }
        );
        return Ok(());
    }

    let rows: Vec<Vec<String>> = requests
        .iter()
        .map(|request| {
            vec![
                request.request_id.chars().take(8).collect(),
                output::short_timestamp(&request.started_at.to_rfc3339()),
                request.process_kind.clone(),
                request.trigger.clone().unwrap_or_default(),
                format!("{}", request.system_chars / 4),
                request.history_length.to_string(),
                request.tool_count.to_string(),
                request.model.clone(),
            ]
        })
        .collect();

    output::table(
        &[
            "ID", "WHEN", "PROCESS", "TRIGGER", "~SYSTOK", "MSGS", "TOOLS", "MODEL",
        ],
        &rows,
    );
    Ok(())
}

async fn show(
    ctx: &super::Context,
    client: &ApiClient,
    request_id: &str,
    raw: bool,
) -> anyhow::Result<()> {
    let value = client
        .get(&format!(
            "prompts/get?request_id={}",
            urlencoding::encode(request_id)
        ))
        .await?;
    if ctx.json {
        output::json(&value);
        return Ok(());
    }

    let record: PromptRecord = client::parse(value)?;

    if raw {
        println!("{}", record.system.text);
        return Ok(());
    }

    println!("request  {}", record.request_id);
    println!(
        "process  {} {}{}",
        record.process.kind,
        record.process.id.as_deref().unwrap_or("-"),
        record
            .process
            .process_type
            .as_deref()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default()
    );
    println!("model    {}", record.model.name);
    println!("trigger  {}", record.trigger.kind);
    if let Some(parent) = &record.trigger.parent {
        println!("parent   {parent}");
    }
    println!(
        "usage    {} in / {} out{}",
        record.usage.input_tokens,
        record.usage.output_tokens,
        if record.usage.cost_usd > 0.0 {
            format!(" · ${:.4}", record.usage.cost_usd)
        } else {
            String::new()
        }
    );
    println!();

    if record.system.blocks.is_empty() {
        println!("--- SYSTEM PROMPT ({} chars) ---", record.system.text.len());
        println!("{}", record.system.text);
    } else {
        let total = record.system.text.len().max(1);
        for block in &record.system.blocks {
            let bytes = block.end - block.start;
            println!(
                "--- {} [{}·{}] {} ch · ~{} tok · {:.1}% ---",
                block.id,
                output::enum_label(&block.layer),
                output::enum_label(&block.stability),
                block.chars,
                block.tokens,
                100.0 * bytes as f64 / total as f64,
            );
            println!("{}", &record.system.text[block.start..block.end]);
        }
    }

    if !record.tools.is_empty() {
        println!("\n--- TOOLS ({}) ---", record.tools.len());
        for tool in &record.tools {
            println!("{:<28} {} ch", tool.name, tool.chars);
        }
    }

    println!("\n--- MESSAGES ({}) ---", record.history_length);
    println!(
        "{}",
        serde_json::to_string_pretty(&record.messages).unwrap_or_default()
    );

    if let Some(text) = &record.response.text {
        println!("\n--- RESPONSE ---");
        println!("{text}");
    }
    if !record.response.tool_calls.is_empty() {
        println!("\ntool calls: {}", record.response.tool_calls.join(", "));
    }
    if let Some(error) = &record.response.error {
        println!("\nerror: {error}");
    }

    Ok(())
}

async fn diff(
    ctx: &super::Context,
    client: &ApiClient,
    first: &str,
    second: &str,
) -> anyhow::Result<()> {
    let left: PromptRecord = client::parse(
        client
            .get(&format!(
                "prompts/get?request_id={}",
                urlencoding::encode(first)
            ))
            .await?,
    )?;
    let right: PromptRecord = client::parse(
        client
            .get(&format!(
                "prompts/get?request_id={}",
                urlencoding::encode(second)
            ))
            .await?,
    )?;

    let slice = |record: &PromptRecord, start: usize, end: usize| {
        record.system.text[start..end].to_string()
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut index = 0usize;
    loop {
        let a = left.system.blocks.get(index);
        let b = right.system.blocks.get(index);
        match (a, b) {
            (None, None) => break,
            (Some(a), Some(b)) if a.id == b.id => {
                let before = slice(&left, a.start, a.end);
                let after = slice(&right, b.start, b.end);
                if before != after {
                    rows.push(vec![
                        a.id.clone(),
                        "changed".to_string(),
                        format!("{} → {}", a.chars, b.chars),
                    ]);
                }
            }
            (Some(a), Some(b)) => rows.push(vec![
                format!("{} / {}", a.id, b.id),
                "reordered".to_string(),
                format!("{} → {}", a.chars, b.chars),
            ]),
            (Some(a), None) => {
                rows.push(vec![a.id.clone(), "removed".to_string(), "-".to_string()])
            }
            (None, Some(b)) => rows.push(vec![b.id.clone(), "added".to_string(), "-".to_string()]),
        }
        index += 1;
    }

    if ctx.json {
        output::json(&serde_json::json!({"changes": rows}));
        return Ok(());
    }

    if rows.is_empty() {
        println!("Block maps are identical.");
        return Ok(());
    }

    output::table(&["BLOCK", "CHANGE", "CHARS"], &rows);
    Ok(())
}

async fn capture(
    ctx: &super::Context,
    client: &ApiClient,
    state: &str,
    retain: Option<i64>,
) -> anyhow::Result<()> {
    let enabled = match state {
        "on" | "true" | "enable" => true,
        "off" | "false" | "disable" => false,
        other => anyhow::bail!("expected `on` or `off`, got `{other}`"),
    };

    let mut body = serde_json::json!({"enabled": enabled});
    if let Some(days) = retain {
        body["retention_days"] = serde_json::json!(days);
    }

    let value = client.post("prompts/capture", &body).await?;
    if ctx.json {
        output::json(&value);
        return Ok(());
    }

    println!(
        "Prompt capture {}{}",
        if enabled { "on" } else { "off" },
        value
            .get("retention_days")
            .and_then(|days| days.as_i64())
            .map(|days| format!(", keeping {days} days"))
            .unwrap_or_default()
    );
    Ok(())
}
