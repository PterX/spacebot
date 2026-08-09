//! `spacebot activity` — instance-wide activity over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Args;
use spacebot::api::activity::ActivityResponse;

#[derive(Args)]
pub struct ActivityArgs {
    /// ISO 8601 lower bound (default: 30 days ago)
    #[arg(long)]
    since: Option<String>,
    /// ISO 8601 upper bound
    #[arg(long)]
    until: Option<String>,
}

pub async fn run(ctx: &super::Context, args: ActivityArgs) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    let mut params: Vec<String> = Vec::new();
    if let Some(since) = &args.since {
        params.push(format!("since={}", urlencoding::encode(since)));
    }
    if let Some(until) = &args.until {
        params.push(format!("until={}", urlencoding::encode(until)));
    }
    let path = if params.is_empty() {
        "activity".to_string()
    } else {
        format!("activity?{}", params.join("&"))
    };

    let value = client.get(&path).await?;
    if ctx.json {
        output::json(&value);
        return Ok(());
    }
    let response: ActivityResponse = client::parse(value)?;
    if response.daily.is_empty() {
        eprintln!("No activity in the selected range.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = response
        .daily
        .iter()
        .map(|day| {
            vec![
                day.date.clone(),
                day.messages.to_string(),
                day.branches.to_string(),
                day.workers.to_string(),
                day.cortex.to_string(),
                day.cron.to_string(),
                day.active_channels.to_string(),
                day.tokens.input.to_string(),
                day.tokens.output.to_string(),
                format!("${:.4}", day.tokens.cost_usd),
            ]
        })
        .collect();
    output::table(
        &[
            "DATE", "MESSAGES", "BRANCHES", "WORKERS", "CORTEX", "CRON", "CHANNELS", "INPUT",
            "OUTPUT", "COST",
        ],
        &rows,
    );

    let totals = &response.totals;
    println!();
    println!("Messages:        {}", totals.messages);
    println!("Branches:        {}", totals.branches);
    println!("Workers:         {}", totals.workers);
    println!("Cortex events:   {}", totals.cortex);
    println!("Cron runs:       {}", totals.cron);
    println!("Active channels: {}", totals.active_channels);
    println!("Input tokens:    {}", totals.tokens.input);
    println!("Output tokens:   {}", totals.tokens.output);
    println!("Cache read:      {}", totals.tokens.cache_read);
    println!("Reasoning:       {}", totals.tokens.reasoning);
    println!("Token cost:      ${:.4}", totals.tokens.cost_usd);

    if !totals.tokens.by_process.is_empty() {
        let mut processes: Vec<_> = totals.tokens.by_process.iter().collect();
        processes.sort_by(|a, b| a.0.cmp(b.0));
        println!("By process:");
        for (process, tokens) in processes {
            println!(
                "  {process}: in {} / out {} (${:.4})",
                tokens.input, tokens.output, tokens.cost_usd
            );
        }
    }

    Ok(())
}
