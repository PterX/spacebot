//! `spacebot task` — task management over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::tasks::{TaskActionResponse, TaskListResponse, TaskResponse};

#[derive(Subcommand)]
pub enum TaskCommand {
    /// List tasks
    List {
        /// Filter by agent (matches owner or assigned)
        #[arg(short, long)]
        agent: Option<String>,
        /// Filter by owner agent
        #[arg(long)]
        owner: Option<String>,
        /// Filter by assigned agent
        #[arg(long)]
        assigned: Option<String>,
        /// Filter by status (pending_approval, backlog, ready, in_progress, done)
        #[arg(short, long)]
        status: Option<String>,
        /// Filter by priority (critical, high, medium, low)
        #[arg(short, long)]
        priority: Option<String>,
        /// Filter by creator
        #[arg(long)]
        created_by: Option<String>,
        /// Maximum number of tasks to return
        #[arg(short, long, default_value_t = 100)]
        limit: i64,
    },
    /// Show a task
    Get {
        /// Task number
        number: i64,
    },
    /// Create a task
    Create {
        /// Task title
        title: String,
        /// Agent that owns the task
        #[arg(short, long)]
        owner: String,
        /// Agent assigned to execute (defaults to the owner)
        #[arg(short, long)]
        assigned: Option<String>,
        /// Task description
        #[arg(long)]
        description: Option<String>,
        /// Priority (critical, high, medium, low)
        #[arg(short, long)]
        priority: Option<String>,
        /// Add a subtask (repeatable)
        #[arg(long = "subtask")]
        subtasks: Vec<String>,
        /// Creator recorded on the task (defaults to human)
        #[arg(long)]
        created_by: Option<String>,
    },
    /// Update a task
    Update {
        /// Task number
        number: i64,
        /// New title
        #[arg(long)]
        title: Option<String>,
        /// New description
        #[arg(long)]
        description: Option<String>,
        /// New status (pending_approval, backlog, ready, in_progress, done)
        #[arg(short, long)]
        status: Option<String>,
        /// New priority (critical, high, medium, low)
        #[arg(short, long)]
        priority: Option<String>,
        /// Reassign to a different agent
        #[arg(short, long)]
        assigned: Option<String>,
        /// Mark the subtask at this index complete (0-based)
        #[arg(long)]
        complete_subtask: Option<usize>,
    },
    /// Approve a task (move to ready)
    Approve {
        /// Task number
        number: i64,
        /// Approver recorded on the task
        #[arg(long)]
        approved_by: Option<String>,
    },
    /// Queue a task for execution (must already be approved)
    Execute {
        /// Task number
        number: i64,
        /// Approver recorded on the task
        #[arg(long)]
        approved_by: Option<String>,
    },
    /// Reassign a task to a different agent
    Assign {
        /// Task number
        number: i64,
        /// Agent to assign
        agent: String,
    },
    /// Delete a task
    Delete {
        /// Task number
        number: i64,
    },
}

pub async fn run(ctx: &super::Context, task_cmd: TaskCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match task_cmd {
        TaskCommand::List {
            agent,
            owner,
            assigned,
            status,
            priority,
            created_by,
            limit,
        } => {
            let mut query = vec![format!("limit={limit}")];
            if let Some(agent) = &agent {
                query.push(format!("agent_id={}", urlencoding::encode(agent)));
            }
            if let Some(owner) = &owner {
                query.push(format!("owner_agent_id={}", urlencoding::encode(owner)));
            }
            if let Some(assigned) = &assigned {
                query.push(format!(
                    "assigned_agent_id={}",
                    urlencoding::encode(assigned)
                ));
            }
            if let Some(status) = &status {
                query.push(format!("status={}", urlencoding::encode(status)));
            }
            if let Some(priority) = &priority {
                query.push(format!("priority={}", urlencoding::encode(priority)));
            }
            if let Some(created_by) = &created_by {
                query.push(format!("created_by={}", urlencoding::encode(created_by)));
            }

            let value = client.get(&format!("tasks?{}", query.join("&"))).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskListResponse = client::parse(value)?;
            if response.tasks.is_empty() {
                eprintln!("No tasks found.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .tasks
                .iter()
                .map(|task| {
                    vec![
                        task.task_number.to_string(),
                        output::truncate(&task.title, 50),
                        output::enum_label(&task.status),
                        output::enum_label(&task.priority),
                        task.assigned_agent_id.clone(),
                        output::short_timestamp(&task.updated_at),
                    ]
                })
                .collect();
            output::table(
                &["#", "TITLE", "STATUS", "PRIORITY", "ASSIGNED", "UPDATED"],
                &rows,
            );
            Ok(())
        }
        TaskCommand::Get { number } => {
            let value = client.get(&format!("tasks/{number}")).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            let task = response.task;
            println!("Number:      #{}", task.task_number);
            println!("Title:       {}", task.title);
            println!("Status:      {}", output::enum_label(&task.status));
            println!("Priority:    {}", output::enum_label(&task.priority));
            println!("Owner:       {}", task.owner_agent_id);
            println!("Assigned:    {}", task.assigned_agent_id);
            println!("Created by:  {}", task.created_by);
            println!("Created:     {}", output::short_timestamp(&task.created_at));
            println!("Updated:     {}", output::short_timestamp(&task.updated_at));
            if let Some(worker_id) = &task.worker_id {
                println!("Worker:      {worker_id}");
            }
            if let Some(approved_at) = &task.approved_at {
                let approved_by = task.approved_by.as_deref().unwrap_or("unknown");
                println!(
                    "Approved:    {} by {approved_by}",
                    output::short_timestamp(approved_at)
                );
            }
            if let Some(completed_at) = &task.completed_at {
                println!("Completed:   {}", output::short_timestamp(completed_at));
            }
            if let Some(description) = &task.description {
                println!("Description: {description}");
            }
            if !task.subtasks.is_empty() {
                println!("Subtasks:");
                for subtask in &task.subtasks {
                    let marker = if subtask.completed { "x" } else { " " };
                    println!("  [{marker}] {}", subtask.title);
                }
            }
            Ok(())
        }
        TaskCommand::Create {
            title,
            owner,
            assigned,
            description,
            priority,
            subtasks,
            created_by,
        } => {
            let mut body = serde_json::json!({
                "owner_agent_id": owner,
                "title": title,
            });
            if let Some(assigned) = &assigned {
                body["assigned_agent_id"] = serde_json::json!(assigned);
            }
            if let Some(description) = &description {
                body["description"] = serde_json::json!(description);
            }
            if let Some(priority) = &priority {
                body["priority"] = serde_json::json!(priority);
            }
            if !subtasks.is_empty() {
                let subtasks: Vec<serde_json::Value> = subtasks
                    .iter()
                    .map(|title| serde_json::json!({ "title": title, "completed": false }))
                    .collect();
                body["subtasks"] = serde_json::json!(subtasks);
            }
            if let Some(created_by) = &created_by {
                body["created_by"] = serde_json::json!(created_by);
            }

            let value = client.post("tasks", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{} created ({}).",
                response.task.task_number,
                output::enum_label(&response.task.status)
            );
            Ok(())
        }
        TaskCommand::Update {
            number,
            title,
            description,
            status,
            priority,
            assigned,
            complete_subtask,
        } => {
            let mut body = serde_json::json!({});
            if let Some(title) = &title {
                body["title"] = serde_json::json!(title);
            }
            if let Some(description) = &description {
                body["description"] = serde_json::json!(description);
            }
            if let Some(status) = &status {
                body["status"] = serde_json::json!(status);
            }
            if let Some(priority) = &priority {
                body["priority"] = serde_json::json!(priority);
            }
            if let Some(assigned) = &assigned {
                body["assigned_agent_id"] = serde_json::json!(assigned);
            }
            if let Some(index) = complete_subtask {
                body["complete_subtask"] = serde_json::json!(index);
            }
            if body.as_object().is_none_or(|fields| fields.is_empty()) {
                anyhow::bail!("nothing to update — pass at least one field flag");
            }

            let value = client.put(&format!("tasks/{number}"), &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{} updated ({}).",
                response.task.task_number,
                output::enum_label(&response.task.status)
            );
            Ok(())
        }
        TaskCommand::Approve {
            number,
            approved_by,
        } => {
            let mut body = serde_json::json!({});
            if let Some(approved_by) = &approved_by {
                body["approved_by"] = serde_json::json!(approved_by);
            }

            let value = client
                .post(&format!("tasks/{number}/approve"), &body)
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{} approved ({}).",
                response.task.task_number,
                output::enum_label(&response.task.status)
            );
            Ok(())
        }
        TaskCommand::Execute {
            number,
            approved_by,
        } => {
            let mut body = serde_json::json!({});
            if let Some(approved_by) = &approved_by {
                body["approved_by"] = serde_json::json!(approved_by);
            }

            let value = client
                .post(&format!("tasks/{number}/execute"), &body)
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{} is {}.",
                response.task.task_number,
                output::enum_label(&response.task.status)
            );
            Ok(())
        }
        TaskCommand::Assign { number, agent } => {
            let body = serde_json::json!({ "assigned_agent_id": agent });
            let value = client
                .post(&format!("tasks/{number}/assign"), &body)
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{} assigned to {}.",
                response.task.task_number, response.task.assigned_agent_id
            );
            Ok(())
        }
        TaskCommand::Delete { number } => {
            let value = client.delete(&format!("tasks/{number}")).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: TaskActionResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
    }
}
