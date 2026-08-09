//! `spacebot memory` — agent memory inspection over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::memories::MemoriesListResponse;
use spacebot::memory::types::Memory;

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// List memories for an agent
    List {
        /// Agent ID
        #[arg(short, long)]
        agent: String,
        /// Maximum number of results (default 50, max 200)
        #[arg(short, long)]
        limit: Option<i64>,
        /// Number of results to skip for pagination
        #[arg(long)]
        offset: Option<usize>,
        /// Filter by memory type (fact, preference, decision, identity,
        /// event, observation, goal, todo)
        #[arg(short = 't', long)]
        memory_type: Option<String>,
        /// Sort order: recent, importance, most_accessed
        #[arg(short, long)]
        sort: Option<String>,
    },
    /// Search memories with hybrid search (vector + FTS + graph)
    Search {
        /// Search query
        query: String,
        /// Agent ID
        #[arg(short, long)]
        agent: String,
        /// Maximum number of results (default 20, max 100)
        #[arg(short, long)]
        limit: Option<usize>,
        /// Filter by memory type
        #[arg(short = 't', long)]
        memory_type: Option<String>,
    },
}

pub async fn run(ctx: &super::Context, memory_cmd: MemoryCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match memory_cmd {
        MemoryCommand::List {
            agent,
            limit,
            offset,
            memory_type,
            sort,
        } => {
            let mut path = format!("agents/memories?agent_id={}", urlencoding::encode(&agent));
            if let Some(limit) = limit {
                path.push_str(&format!("&limit={limit}"));
            }
            if let Some(offset) = offset {
                path.push_str(&format!("&offset={offset}"));
            }
            if let Some(memory_type) = &memory_type {
                path.push_str(&format!(
                    "&memory_type={}",
                    urlencoding::encode(memory_type)
                ));
            }
            if let Some(sort) = &sort {
                path.push_str(&format!("&sort={}", urlencoding::encode(sort)));
            }

            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: MemoriesListResponse = client::parse(value)?;
            if response.memories.is_empty() {
                eprintln!("No memories found.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .memories
                .iter()
                .map(|memory| {
                    vec![
                        memory.id.clone(),
                        output::enum_label(&memory.memory_type),
                        format!("{:.2}", memory.importance),
                        memory.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                        summary(&memory.content),
                    ]
                })
                .collect();
            output::table(&["ID", "TYPE", "IMPORTANCE", "UPDATED", "CONTENT"], &rows);
            eprintln!(
                "{} of {} memories.",
                response.memories.len(),
                response.total
            );
            Ok(())
        }
        MemoryCommand::Search {
            query,
            agent,
            limit,
            memory_type,
        } => {
            let mut path = format!(
                "agents/memories/search?agent_id={}&q={}",
                urlencoding::encode(&agent),
                urlencoding::encode(&query),
            );
            if let Some(limit) = limit {
                path.push_str(&format!("&limit={limit}"));
            }
            if let Some(memory_type) = &memory_type {
                path.push_str(&format!(
                    "&memory_type={}",
                    urlencoding::encode(memory_type)
                ));
            }

            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            // The search result wrapper is serialize-only on the server, so
            // parse each embedded memory typed and read the score raw.
            let results = value["results"].as_array().cloned().unwrap_or_default();
            if results.is_empty() {
                eprintln!("No matching memories.");
                return Ok(());
            }
            let mut rows = Vec::with_capacity(results.len());
            for result in &results {
                let memory: Memory = client::parse(result["memory"].clone())?;
                rows.push(vec![
                    format!("{:.3}", result["score"].as_f64().unwrap_or(0.0)),
                    memory.id.clone(),
                    output::enum_label(&memory.memory_type),
                    summary(&memory.content),
                ]);
            }
            output::table(&["SCORE", "ID", "TYPE", "CONTENT"], &rows);
            Ok(())
        }
    }
}

/// First line of the content, truncated for table display.
fn summary(content: &str) -> String {
    const MAX_CHARS: usize = 80;
    let line = content.lines().next().unwrap_or("");
    let mut out: String = line.chars().take(MAX_CHARS).collect();
    if content.lines().nth(1).is_some() || line.chars().count() > MAX_CHARS {
        out.push_str("...");
    }
    out
}
