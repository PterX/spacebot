//! `spacebot channel` — channel management over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::channels::{CancelProcessResponse, ChannelsResponse};

#[derive(Subcommand)]
pub enum ChannelCommand {
    /// List channels across agents
    List {
        /// Include archived (inactive) channels
        #[arg(long)]
        all: bool,
        /// Filter by agent ID
        #[arg(short, long)]
        agent: Option<String>,
    },
    /// Show live status (active branches and workers) for channels
    Status {
        /// Only show status for this channel
        channel_id: Option<String>,
    },
    /// Show the message timeline for a channel
    Messages {
        /// Channel ID
        channel_id: String,
        /// Maximum number of messages to return (max 100)
        #[arg(short, long, default_value_t = 20)]
        limit: i64,
        /// Pagination cursor for fetching older messages
        #[arg(short, long)]
        before: Option<String>,
    },
    /// Archive a channel without deleting its history
    Archive {
        /// Agent ID that owns the channel
        agent_id: String,
        /// Channel ID
        channel_id: String,
    },
    /// Restore an archived channel
    Unarchive {
        /// Agent ID that owns the channel
        agent_id: String,
        /// Channel ID
        channel_id: String,
    },
    /// Delete a channel and its message history
    Delete {
        /// Agent ID that owns the channel
        agent_id: String,
        /// Channel ID
        channel_id: String,
    },
    /// Cancel a running worker or branch
    Cancel {
        /// Channel ID
        channel_id: String,
        /// Process type
        #[arg(value_parser = ["worker", "branch"])]
        process_type: String,
        /// Process ID
        process_id: String,
    },
}

pub async fn run(ctx: &super::Context, channel_cmd: ChannelCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match channel_cmd {
        ChannelCommand::List { all, agent } => {
            let mut params = Vec::new();
            if all {
                params.push("include_inactive=true".to_string());
            }
            if let Some(agent) = &agent {
                params.push(format!("agent_id={}", urlencoding::encode(agent)));
            }
            let path = if params.is_empty() {
                "channels".to_string()
            } else {
                format!("channels?{}", params.join("&"))
            };

            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ChannelsResponse = client::parse(value)?;
            if response.channels.is_empty() {
                eprintln!("No channels found.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .channels
                .iter()
                .map(|channel| {
                    vec![
                        channel.id.clone(),
                        channel.agent_id.clone(),
                        channel.platform.clone(),
                        channel.display_name.clone().unwrap_or_default(),
                        if channel.is_active { "yes" } else { "no" }.to_string(),
                        output::short_timestamp(&channel.last_activity_at),
                    ]
                })
                .collect();
            output::table(
                &["ID", "AGENT", "PLATFORM", "NAME", "ACTIVE", "LAST ACTIVITY"],
                &rows,
            );
            Ok(())
        }
        ChannelCommand::Status { channel_id } => {
            let value = client.get("channels/status").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let mut entries: Vec<(&String, &serde_json::Value)> = value
                .as_object()
                .map(|map| map.iter().collect())
                .unwrap_or_default();
            if let Some(filter) = &channel_id {
                entries.retain(|(id, _)| id.as_str() == filter.as_str());
                if entries.is_empty() {
                    eprintln!("No status for channel {filter}.");
                    return Ok(());
                }
            }
            if entries.is_empty() {
                eprintln!("No channel status available.");
                return Ok(());
            }
            entries.sort_by_key(|(id, _)| id.as_str());
            for (id, block) in entries {
                println!("{id}");
                let branches = block["active_branches"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let workers = block["active_workers"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for branch in &branches {
                    println!(
                        "  branch {}  {}",
                        branch["id"].as_str().unwrap_or(""),
                        branch["description"].as_str().unwrap_or(""),
                    );
                }
                for worker in &workers {
                    println!(
                        "  worker {}  [{}] {}",
                        worker["id"].as_str().unwrap_or(""),
                        worker["status"].as_str().unwrap_or(""),
                        worker["task"].as_str().unwrap_or(""),
                    );
                }
                if branches.is_empty() && workers.is_empty() {
                    println!("  idle");
                }
            }
            Ok(())
        }
        ChannelCommand::Messages {
            channel_id,
            limit,
            before,
        } => {
            let mut path = format!(
                "channels/messages?channel_id={}&limit={limit}",
                urlencoding::encode(&channel_id)
            );
            if let Some(before) = &before {
                path.push_str(&format!("&before={}", urlencoding::encode(before)));
            }

            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let items = value["items"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                eprintln!("No messages found for channel {channel_id}.");
                return Ok(());
            }
            for item in &items {
                let timestamp = output::short_timestamp(
                    item["created_at"]
                        .as_str()
                        .or(item["started_at"].as_str())
                        .unwrap_or(""),
                );
                match item["type"].as_str().unwrap_or("") {
                    "message" => {
                        let sender = item["sender_name"]
                            .as_str()
                            .or(item["role"].as_str())
                            .unwrap_or("");
                        println!(
                            "[{timestamp}] {sender}: {}",
                            item["content"].as_str().unwrap_or("")
                        );
                    }
                    "branch_run" => {
                        println!(
                            "[{timestamp}] branch {}: {}",
                            item["id"].as_str().unwrap_or(""),
                            item["description"].as_str().unwrap_or(""),
                        );
                    }
                    "worker_run" => {
                        println!(
                            "[{timestamp}] worker {} [{}]: {}",
                            item["id"].as_str().unwrap_or(""),
                            item["status"].as_str().unwrap_or(""),
                            item["task"].as_str().unwrap_or(""),
                        );
                    }
                    "tool_call_run" => {
                        println!(
                            "[{timestamp}] tool {} [{}]",
                            item["tool_name"].as_str().unwrap_or(""),
                            item["status"].as_str().unwrap_or(""),
                        );
                    }
                    other => println!("[{timestamp}] {other}"),
                }
            }
            if value["has_more"].as_bool().unwrap_or(false) {
                eprintln!("Older messages available — pass --before with a pagination cursor.");
            }
            Ok(())
        }
        ChannelCommand::Archive {
            agent_id,
            channel_id,
        } => set_archived(&client, ctx, &agent_id, &channel_id, true).await,
        ChannelCommand::Unarchive {
            agent_id,
            channel_id,
        } => set_archived(&client, ctx, &agent_id, &channel_id, false).await,
        ChannelCommand::Delete {
            agent_id,
            channel_id,
        } => {
            let value = client
                .delete(&format!(
                    "channels?agent_id={}&channel_id={}",
                    urlencoding::encode(&agent_id),
                    urlencoding::encode(&channel_id)
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            eprintln!("Channel {channel_id} deleted.");
            Ok(())
        }
        ChannelCommand::Cancel {
            channel_id,
            process_type,
            process_id,
        } => {
            let body = serde_json::json!({
                "channel_id": channel_id,
                "process_type": process_type,
                "process_id": process_id,
            });

            let value = client.post("channels/cancel-process", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: CancelProcessResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
    }
}

async fn set_archived(
    client: &ApiClient,
    ctx: &super::Context,
    agent_id: &str,
    channel_id: &str,
    archived: bool,
) -> anyhow::Result<()> {
    let body = serde_json::json!({
        "agent_id": agent_id,
        "channel_id": channel_id,
        "archived": archived,
    });

    let value = client.post("channels/archive", &body).await?;
    if ctx.json {
        output::json(&value);
        return Ok(());
    }
    if archived {
        eprintln!("Channel {channel_id} archived.");
    } else {
        eprintln!("Channel {channel_id} restored.");
    }
    Ok(())
}
