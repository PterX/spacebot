//! `spacebot task` — task management over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::tasks::{
    TaskActionResponse, TaskCommentListResponse, TaskCommentResponse, TaskHistoryResponse,
    TaskListResponse, TaskResponse, TaskRevisionResponse,
};
use spacebot::tasks::TaskRevisionDiff;

/// Marks every write from this binary so revision history records where a
/// change came from.
const CLI_SOURCE: &str = "cli";

/// Render a snapshot field value as one line, or as an indented block when it
/// spans several.
fn render_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "-".to_string(),
        serde_json::Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn print_block(label: &str, value: &serde_json::Value) {
    let rendered = render_value(value);
    if rendered.contains('\n') {
        println!("{label}:");
        for line in rendered.lines() {
            println!("    {line}");
        }
    } else {
        println!("{label}: {rendered}");
    }
}

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
        /// Why this edit was made, recorded on the revision
        #[arg(long)]
        summary: Option<String>,
        /// Fail if the task has moved past this revision
        #[arg(long)]
        expect: Option<i64>,
    },
    /// Add a comment to a task's discussion
    Comment {
        /// Task number
        number: i64,
        /// Comment body
        body: String,
        /// Author recorded on the comment
        #[arg(long)]
        author: Option<String>,
    },
    /// List a task's comments
    Comments {
        /// Task number
        number: i64,
        /// Maximum number of comments to return
        #[arg(short, long, default_value_t = 50)]
        limit: i64,
        /// Resume after this comment sequence number
        #[arg(long)]
        after: Option<i64>,
    },
    /// List a task's revision history
    History {
        /// Task number
        number: i64,
        /// Maximum number of revisions to return
        #[arg(short, long, default_value_t = 50)]
        limit: i64,
    },
    /// Show one historical revision of a task
    Revision {
        /// Task number
        number: i64,
        /// Revision number
        revision: i64,
    },
    /// Show what changed between two revisions
    Diff {
        /// Task number
        number: i64,
        /// Revision to diff from
        from: i64,
        /// Revision to diff to (defaults to current)
        to: Option<i64>,
    },
    /// Restore a task to a historical revision
    Restore {
        /// Task number
        number: i64,
        /// Revision to restore
        revision: i64,
        /// Why the restore is being made
        #[arg(long)]
        summary: Option<String>,
        /// Revision the restore is written against (defaults to current)
        #[arg(long)]
        expect: Option<i64>,
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
                        task.assigned_agent_id
                            .clone()
                            .unwrap_or_else(|| "-".to_string()),
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
            println!(
                "Assigned:    {}",
                task.assigned_agent_id.as_deref().unwrap_or("-")
            );
            println!("Created by:  {}", task.created_by);
            println!("Revision:    {}", task.revision);
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
                "source": CLI_SOURCE,
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
            summary,
            expect,
        } => {
            let mut body = serde_json::json!({ "source": CLI_SOURCE });
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
            if title.is_none()
                && description.is_none()
                && status.is_none()
                && priority.is_none()
                && assigned.is_none()
                && complete_subtask.is_none()
            {
                anyhow::bail!("nothing to update — pass at least one field flag");
            }
            if let Some(summary) = &summary {
                body["edit_summary"] = serde_json::json!(summary);
            }
            if let Some(expect) = expect {
                body["expected_revision"] = serde_json::json!(expect);
            }

            let value = client.put(&format!("tasks/{number}"), &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{} updated ({}, revision {}).",
                response.task.task_number,
                output::enum_label(&response.task.status),
                response.task.revision
            );
            Ok(())
        }
        TaskCommand::Comment {
            number,
            body,
            author,
        } => {
            let mut request = serde_json::json!({ "body": body });
            if let Some(author) = &author {
                request["author_id"] = serde_json::json!(author);
            }

            let value = client
                .post(&format!("tasks/{number}/comments"), &request)
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskCommentResponse = client::parse(value)?;
            eprintln!("Commented on task #{number} (#{}).", response.comment.seq);
            Ok(())
        }
        TaskCommand::Comments {
            number,
            limit,
            after,
        } => {
            let mut query = vec![format!("limit={limit}")];
            if let Some(after) = after {
                query.push(format!("after={after}"));
            }

            let value = client
                .get(&format!("tasks/{number}/comments?{}", query.join("&")))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskCommentListResponse = client::parse(value)?;
            if response.comments.is_empty() {
                eprintln!("No comments on task #{number}.");
                return Ok(());
            }
            for comment in &response.comments {
                println!(
                    "#{} {} {} · {}",
                    comment.seq,
                    comment.author_type,
                    comment.author_id.as_deref().unwrap_or("-"),
                    output::short_timestamp(&comment.created_at)
                );
                for line in comment.body.lines() {
                    println!("    {line}");
                }
                println!();
            }
            eprintln!(
                "{} of {} comment(s) shown.",
                response.comments.len(),
                response.total
            );
            Ok(())
        }
        TaskCommand::History { number, limit } => {
            let value = client
                .get(&format!("tasks/{number}/revisions?limit={limit}"))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskHistoryResponse = client::parse(value)?;
            if response.revisions.is_empty() {
                eprintln!("Task #{number} has no revision history.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .revisions
                .iter()
                .map(|revision| {
                    vec![
                        revision.revision.to_string(),
                        revision.author_type.to_string(),
                        revision
                            .author_id
                            .clone()
                            .unwrap_or_else(|| "-".to_string()),
                        revision.source.to_string(),
                        match revision.restored_from {
                            Some(from) => format!("restore of {from}"),
                            None => revision
                                .edit_summary
                                .clone()
                                .unwrap_or_else(|| "-".to_string()),
                        },
                        output::short_timestamp(&revision.created_at),
                    ]
                })
                .collect();
            output::table(&["REV", "AUTHOR", "ID", "SOURCE", "SUMMARY", "WHEN"], &rows);
            eprintln!("Task #{number} is at revision {}.", response.current);
            Ok(())
        }
        TaskCommand::Revision { number, revision } => {
            let value = client
                .get(&format!("tasks/{number}/revisions/{revision}"))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskRevisionResponse = client::parse(value)?;
            let summary = &response.revision.summary;
            let snapshot = &response.revision.snapshot;
            println!("Revision:    {}", summary.revision);
            println!(
                "Author:      {} {}",
                summary.author_type,
                summary.author_id.as_deref().unwrap_or("-")
            );
            println!("Source:      {}", summary.source);
            if let Some(from) = summary.restored_from {
                println!("Restored:    revision {from}");
            }
            if let Some(edit_summary) = &summary.edit_summary {
                println!("Summary:     {edit_summary}");
            }
            println!(
                "When:        {}",
                output::short_timestamp(&summary.created_at)
            );
            println!();
            println!("Title:       {}", snapshot.title);
            println!("Status:      {}", output::enum_label(&snapshot.status));
            println!("Priority:    {}", output::enum_label(&snapshot.priority));
            println!(
                "Assigned:    {}",
                snapshot.assigned_agent_id.as_deref().unwrap_or("-")
            );
            if let Some(description) = &snapshot.description {
                println!("Description: {description}");
            }
            if !snapshot.subtasks.is_empty() {
                println!("Subtasks:");
                for subtask in &snapshot.subtasks {
                    let marker = if subtask.completed { "x" } else { " " };
                    println!("  [{marker}] {}", subtask.title);
                }
            }
            if !snapshot.depends_on.is_empty() {
                let edges: Vec<String> = snapshot
                    .depends_on
                    .iter()
                    .map(|edge| format!("#{} ({})", edge.task, edge.kind))
                    .collect();
                println!("Depends on:  {}", edges.join(", "));
            }
            Ok(())
        }
        TaskCommand::Diff { number, from, to } => {
            let mut query = vec![format!("from={from}")];
            if let Some(to) = to {
                query.push(format!("to={to}"));
            }

            let value = client
                .get(&format!(
                    "tasks/{number}/revisions/diff?{}",
                    query.join("&")
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let diff: TaskRevisionDiff = client::parse(value)?;
            if diff.changes.is_empty() {
                eprintln!(
                    "Revisions {} and {} of task #{number} are materially identical.",
                    diff.from, diff.to
                );
                return Ok(());
            }
            eprintln!("Task #{number}: revision {} → {}", diff.from, diff.to);
            for change in &diff.changes {
                println!();
                println!("{}", change.field);
                print_block("  before", &change.before);
                print_block("  after", &change.after);
            }
            Ok(())
        }
        TaskCommand::Restore {
            number,
            revision,
            summary,
            expect,
        } => {
            let expected = match expect {
                Some(expected) => expected,
                None => {
                    let value = client.get(&format!("tasks/{number}")).await?;
                    let response: TaskResponse = client::parse(value)?;
                    response.task.revision
                }
            };

            let mut request = serde_json::json!({
                "expected_revision": expected,
                "source": CLI_SOURCE,
            });
            if let Some(summary) = &summary {
                request["edit_summary"] = serde_json::json!(summary);
            }

            let value = client
                .post(
                    &format!("tasks/{number}/revisions/{revision}/restore"),
                    &request,
                )
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: TaskResponse = client::parse(value)?;
            eprintln!(
                "Task #{number} restored to revision {revision}, now at revision {}.",
                response.task.revision
            );
            Ok(())
        }
        TaskCommand::Approve {
            number,
            approved_by,
        } => {
            let mut body = serde_json::json!({ "source": CLI_SOURCE });
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
            let mut body = serde_json::json!({ "source": CLI_SOURCE });
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
            let body = serde_json::json!({
                "assigned_agent_id": agent,
                "source": CLI_SOURCE,
            });
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
                response.task.task_number,
                response.task.assigned_agent_id.as_deref().unwrap_or("-")
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
