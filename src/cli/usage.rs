//! `spacebot usage` — aggregated token usage over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Args;
use spacebot::api::usage::{UsageResponse, UsageTotals};

#[derive(Args)]
pub struct UsageArgs {
    /// Aggregate usage for one conversation instead of the instance
    #[arg(long, value_name = "ID", conflicts_with_all = ["since", "until", "group_by"])]
    conversation: Option<String>,
    /// Filter to one agent
    #[arg(short, long)]
    agent: Option<String>,
    /// ISO 8601 lower bound (default: 30 days ago)
    #[arg(long)]
    since: Option<String>,
    /// ISO 8601 upper bound
    #[arg(long)]
    until: Option<String>,
    /// Group totals by day, agent, or model (comma-separated)
    #[arg(short, long, value_name = "GROUPS")]
    group_by: Option<String>,
}

pub async fn run(ctx: &super::Context, args: UsageArgs) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    if let Some(conversation_id) = &args.conversation {
        let mut path = format!("usage/conversation/{conversation_id}");
        if let Some(agent) = &args.agent {
            path.push_str(&format!("?agent_id={}", urlencoding::encode(agent)));
        }
        let value = client.get(&path).await?;
        if ctx.json {
            output::json(&value);
            return Ok(());
        }
        let totals: UsageTotals = client::parse(value)?;
        print_totals(&totals);
        return Ok(());
    }

    let mut params: Vec<String> = Vec::new();
    if let Some(agent) = &args.agent {
        params.push(format!("agent_id={}", urlencoding::encode(agent)));
    }
    if let Some(since) = &args.since {
        params.push(format!("since={}", urlencoding::encode(since)));
    }
    if let Some(until) = &args.until {
        params.push(format!("until={}", urlencoding::encode(until)));
    }
    if let Some(group_by) = &args.group_by {
        params.push(format!("group_by={}", urlencoding::encode(group_by)));
    }
    let path = if params.is_empty() {
        "usage".to_string()
    } else {
        format!("usage?{}", params.join("&"))
    };

    let value = client.get(&path).await?;
    if ctx.json {
        output::json(&value);
        return Ok(());
    }
    let response: UsageResponse = client::parse(value)?;
    print_totals(&response.total);

    if !response.by_day.is_empty() {
        println!();
        let rows: Vec<Vec<String>> = response
            .by_day
            .iter()
            .map(|day| {
                vec![
                    day.date.clone(),
                    day.input_tokens.to_string(),
                    day.output_tokens.to_string(),
                    day.request_count.to_string(),
                    format_cost(day.estimated_cost_usd),
                ]
            })
            .collect();
        output::table(&["DATE", "INPUT", "OUTPUT", "REQUESTS", "COST"], &rows);
    }

    if !response.by_agent.is_empty() {
        println!();
        let rows: Vec<Vec<String>> = response
            .by_agent
            .iter()
            .map(|agent| {
                vec![
                    agent.agent_id.clone(),
                    agent.input_tokens.to_string(),
                    agent.output_tokens.to_string(),
                    agent.request_count.to_string(),
                    format_cost(agent.estimated_cost_usd),
                ]
            })
            .collect();
        output::table(&["AGENT", "INPUT", "OUTPUT", "REQUESTS", "COST"], &rows);
    }

    if !response.by_model.is_empty() {
        println!();
        let rows: Vec<Vec<String>> = response
            .by_model
            .iter()
            .map(|model| {
                vec![
                    model.model.clone(),
                    model.input_tokens.to_string(),
                    model.output_tokens.to_string(),
                    model.request_count.to_string(),
                    format_cost(model.estimated_cost_usd),
                ]
            })
            .collect();
        output::table(&["MODEL", "INPUT", "OUTPUT", "REQUESTS", "COST"], &rows);
    }

    Ok(())
}

fn print_totals(totals: &UsageTotals) {
    println!("Input tokens:       {}", totals.input_tokens);
    println!("Output tokens:      {}", totals.output_tokens);
    println!("Cache read tokens:  {}", totals.cache_read_tokens);
    println!("Cache write tokens: {}", totals.cache_write_tokens);
    println!("Reasoning tokens:   {}", totals.reasoning_tokens);
    println!("Requests:           {}", totals.request_count);
    match totals.estimated_cost_usd {
        Some(cost) => println!("Estimated cost:     ${cost:.4} ({})", totals.cost_status),
        None => println!("Estimated cost:     unknown"),
    }
}

fn format_cost(cost: Option<f64>) -> String {
    cost.map(|c| format!("${c:.4}"))
        .unwrap_or_else(|| "-".to_string())
}
