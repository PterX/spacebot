//! `spacebot notification` — notification inbox over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::notifications::{NotificationsResponse, UnreadCountResponse};

#[derive(Subcommand)]
pub enum NotificationCommand {
    /// List notifications
    List {
        /// Only unread notifications
        #[arg(short, long)]
        unread: bool,
        /// Filter by agent id
        #[arg(short, long)]
        agent: Option<String>,
        /// Filter by kind (task_approval, worker_failed, cortex_observation)
        #[arg(short, long)]
        kind: Option<String>,
        /// Maximum number of notifications to return
        #[arg(short, long)]
        limit: Option<i64>,
        /// Number of notifications to skip
        #[arg(long)]
        offset: Option<i64>,
    },
    /// Count unread notifications
    Count,
    /// Mark a notification as read
    Read {
        /// Notification id
        #[arg(required_unless_present = "all")]
        id: Option<String>,
        /// Mark all notifications as read
        #[arg(long, conflicts_with = "id")]
        all: bool,
    },
    /// Dismiss a notification
    Dismiss {
        /// Notification id
        #[arg(required_unless_present = "read")]
        id: Option<String>,
        /// Dismiss all already-read notifications
        #[arg(long, conflicts_with = "id")]
        read: bool,
    },
}

pub async fn run(ctx: &super::Context, cmd: NotificationCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match cmd {
        NotificationCommand::List {
            unread,
            agent,
            kind,
            limit,
            offset,
        } => {
            let mut params: Vec<String> = Vec::new();
            if unread {
                params.push("filter=unread".to_string());
            }
            if let Some(agent) = &agent {
                params.push(format!("agent_id={}", urlencoding::encode(agent)));
            }
            if let Some(kind) = &kind {
                params.push(format!("kind={}", urlencoding::encode(kind)));
            }
            if let Some(limit) = limit {
                params.push(format!("limit={limit}"));
            }
            if let Some(offset) = offset {
                params.push(format!("offset={offset}"));
            }
            let path = if params.is_empty() {
                "notifications".to_string()
            } else {
                format!("notifications?{}", params.join("&"))
            };

            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: NotificationsResponse = client::parse(value)?;
            if response.notifications.is_empty() {
                eprintln!("No notifications.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .notifications
                .iter()
                .map(|notification| {
                    let status = if notification.read_at.is_some() {
                        "read"
                    } else {
                        "unread"
                    };
                    vec![
                        notification.id.clone(),
                        notification.kind.clone(),
                        notification.severity.clone(),
                        notification.agent_id.clone().unwrap_or_else(|| "-".into()),
                        notification.title.clone(),
                        status.to_string(),
                        output::short_timestamp(&notification.created_at),
                    ]
                })
                .collect();
            output::table(
                &[
                    "ID", "KIND", "SEVERITY", "AGENT", "TITLE", "STATUS", "CREATED",
                ],
                &rows,
            );
            Ok(())
        }
        NotificationCommand::Count => {
            let value = client.get("notifications/unread_count").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: UnreadCountResponse = client::parse(value)?;
            println!("{}", response.count);
            Ok(())
        }
        NotificationCommand::Read { id, all } => {
            let value = match (&id, all) {
                (_, true) => {
                    client
                        .post("notifications/read_all", &serde_json::json!({}))
                        .await?
                }
                (Some(id), false) => {
                    client
                        .post(&format!("notifications/{id}/read"), &serde_json::json!({}))
                        .await?
                }
                (None, false) => unreachable!("clap requires id unless --all"),
            };
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            if all {
                eprintln!("All notifications marked as read.");
            } else {
                eprintln!("Marked as read.");
            }
            Ok(())
        }
        NotificationCommand::Dismiss { id, read } => {
            let value = match (&id, read) {
                (_, true) => {
                    client
                        .post("notifications/dismiss_read", &serde_json::json!({}))
                        .await?
                }
                (Some(id), false) => {
                    client
                        .post(
                            &format!("notifications/{id}/dismiss"),
                            &serde_json::json!({}),
                        )
                        .await?
                }
                (None, false) => unreachable!("clap requires id unless --read"),
            };
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            if read {
                eprintln!("Read notifications dismissed.");
            } else {
                eprintln!("Dismissed.");
            }
            Ok(())
        }
    }
}
