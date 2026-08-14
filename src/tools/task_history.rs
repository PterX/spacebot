//! Read and restore a task's specification history.
//!
//! Four actions on one tool because they are one train of thought: list what
//! changed, read the version you suspect, diff it against now, put it back.

use crate::AgentId;
use crate::tasks::{TaskAuthorKind, TaskMutationContext, TaskMutationSource, TaskStore};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Revisions returned by a `list` with no explicit limit.
const DEFAULT_HISTORY_LIMIT: i64 = 20;

#[derive(Clone)]
pub struct TaskHistoryTool {
    task_store: Arc<TaskStore>,
    agent_id: AgentId,
    api_state: Option<Arc<crate::api::ApiState>>,
}

impl TaskHistoryTool {
    pub fn new(task_store: Arc<TaskStore>, agent_id: AgentId) -> Self {
        Self {
            task_store,
            agent_id,
            api_state: None,
        }
    }

    pub fn with_api_state(mut self, api_state: Arc<crate::api::ApiState>) -> Self {
        self.api_state = Some(api_state);
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("task_history failed: {0}")]
pub struct TaskHistoryError(String);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskHistoryArgs {
    pub task_number: i32,
    /// `list` (default), `get`, `diff`, or `restore`.
    pub action: Option<String>,
    /// The revision to read, diff from, or restore.
    pub revision: Option<i64>,
    /// Revision to diff against. Defaults to the task's current revision.
    pub to_revision: Option<i64>,
    /// Revisions returned by `list`.
    pub limit: Option<i64>,
    /// Why this restore is being made. Required for `restore`.
    pub edit_summary: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskHistoryOutput {
    pub success: bool,
    pub task_number: i64,
    /// The task's revision after this call.
    pub current_revision: i64,
    pub result: serde_json::Value,
    pub message: String,
}

impl Tool for TaskHistoryTool {
    const NAME: &'static str = "task_history";

    type Error = TaskHistoryError;
    type Args = TaskHistoryArgs;
    type Output = TaskHistoryOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/task_history").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_number": { "type": "integer", "description": "Task number reference (#N)" },
                    "action": {
                        "type": "string",
                        "enum": ["list", "get", "diff", "restore"],
                        "description": "\"list\" (default) summarises revisions newest first; \"get\" reads one revision whole; \"diff\" compares two; \"restore\" puts the task back to one by appending a new revision"
                    },
                    "revision": { "type": "integer", "description": "Revision to read, diff from, or restore" },
                    "to_revision": { "type": "integer", "description": "Revision to diff against; defaults to current" },
                    "limit": { "type": "integer", "description": "How many revisions \"list\" returns" },
                    "edit_summary": { "type": "string", "description": "Why you are restoring. Required for \"restore\"" }
                },
                "required": ["task_number"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let task_number = i64::from(args.task_number);
        let action = args.action.as_deref().unwrap_or("list");

        let task = self
            .task_store
            .get_by_number(task_number)
            .await
            .map_err(|error| TaskHistoryError(format!("{error}")))?
            .ok_or_else(|| TaskHistoryError(format!("task #{task_number} not found")))?;

        let revision_arg = |name: &str| {
            args.revision
                .ok_or_else(|| TaskHistoryError(format!("\"{name}\" needs a revision number")))
        };

        match action {
            "list" => {
                let revisions = self
                    .task_store
                    .list_revisions(task_number, args.limit.unwrap_or(DEFAULT_HISTORY_LIMIT))
                    .await
                    .map_err(|error| TaskHistoryError(format!("{error}")))?;
                let count = revisions.len();
                Ok(TaskHistoryOutput {
                    success: true,
                    task_number,
                    current_revision: task.revision,
                    result: serde_json::json!({ "revisions": revisions }),
                    message: format!(
                        "Task #{task_number} is at revision {}; {count} shown",
                        task.revision
                    ),
                })
            }
            "get" => {
                let revision = revision_arg("get")?;
                let found = self
                    .task_store
                    .get_revision(task_number, revision)
                    .await
                    .map_err(|error| TaskHistoryError(format!("{error}")))?
                    .ok_or_else(|| {
                        TaskHistoryError(format!("task #{task_number} has no revision {revision}"))
                    })?;
                Ok(TaskHistoryOutput {
                    success: true,
                    task_number,
                    current_revision: task.revision,
                    result: serde_json::to_value(&found).unwrap_or_default(),
                    message: format!("Revision {revision} of task #{task_number}"),
                })
            }
            "diff" => {
                let from = revision_arg("diff")?;
                let diff = self
                    .task_store
                    .diff_revisions(task_number, from, args.to_revision)
                    .await
                    .map_err(|error| TaskHistoryError(format!("{error}")))?;
                let changed = diff.changes.len();
                let to = diff.to;
                Ok(TaskHistoryOutput {
                    success: true,
                    task_number,
                    current_revision: task.revision,
                    result: serde_json::to_value(&diff).unwrap_or_default(),
                    message: if changed == 0 {
                        format!("Revisions {from} and {to} are materially identical")
                    } else {
                        format!("{changed} field(s) differ between revisions {from} and {to}")
                    },
                })
            }
            "restore" => {
                let revision = revision_arg("restore")?;
                let Some(edit_summary) = args.edit_summary else {
                    return Err(TaskHistoryError(
                        "restore needs an edit_summary saying why".to_string(),
                    ));
                };

                let context = TaskMutationContext::new(
                    TaskAuthorKind::Agent,
                    Some(self.agent_id.to_string()),
                    TaskMutationSource::Restore,
                )
                .with_summary(Some(edit_summary))
                .expecting(Some(task.revision));

                let update = self
                    .task_store
                    .restore_revision(task_number, revision, context)
                    .await
                    .map_err(|error| TaskHistoryError(format!("{error}")))?;

                if let Some(api_state) = &self.api_state {
                    api_state
                        .event_tx
                        .send(crate::api::ApiEvent::TaskUpdated {
                            agent_id: update.task.effective_agent_id().to_string(),
                            task_number,
                            status: update.task.status.to_string(),
                            action: "updated".to_string(),
                        })
                        .ok();
                    if let Some(new_revision) = update.new_revision {
                        api_state
                            .event_tx
                            .send(crate::api::ApiEvent::TaskRevised {
                                agent_id: update.task.effective_agent_id().to_string(),
                                task_number,
                                revision: new_revision,
                                restored_from: Some(revision),
                            })
                            .ok();
                    }
                }

                let message = match update.new_revision {
                    Some(new_revision) => format!(
                        "Restored revision {revision} of task #{task_number} as revision {new_revision}"
                    ),
                    None => format!(
                        "Task #{task_number} already matched revision {revision}; nothing changed"
                    ),
                };
                Ok(TaskHistoryOutput {
                    success: true,
                    task_number,
                    current_revision: update.task.revision,
                    result: serde_json::json!({ "restored_from": revision }),
                    message,
                })
            }
            other => Err(TaskHistoryError(format!(
                "unknown action \"{other}\"; use list, get, diff, or restore"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::store::{CreateTaskInput, UpdateTaskInput, setup_test_store};
    use crate::tasks::{TaskPriority, TaskStatus};

    async fn tool_with_task() -> (TaskHistoryTool, i64) {
        let store = Arc::new(setup_test_store().await);
        let task = store
            .create(CreateTaskInput {
                owner_agent_id: "main".to_string(),
                assigned_agent_id: Some("main".to_string()),
                title: "spec".to_string(),
                description: Some("first draft".to_string()),
                status: TaskStatus::Backlog,
                created_by: "human".to_string(),
                context: TaskMutationContext::new(
                    TaskAuthorKind::User,
                    Some("jamie".to_string()),
                    TaskMutationSource::Cli,
                ),
                ..Default::default()
            })
            .await
            .expect("task should be created");

        store
            .update_with_status_transition(
                task.task_number,
                UpdateTaskInput {
                    description: Some(Some("agent rewrote this".to_string())),
                    priority: Some(TaskPriority::Critical),
                    context: TaskMutationContext::new(
                        TaskAuthorKind::Agent,
                        Some("main".to_string()),
                        TaskMutationSource::Tool,
                    ),
                    ..Default::default()
                },
            )
            .await
            .expect("update should succeed");

        let tool = TaskHistoryTool::new(store, std::sync::Arc::from("main"));
        (tool, task.task_number)
    }

    fn args(task_number: i64, action: &str) -> TaskHistoryArgs {
        TaskHistoryArgs {
            task_number: task_number as i32,
            action: Some(action.to_string()),
            revision: None,
            to_revision: None,
            limit: None,
            edit_summary: None,
        }
    }

    #[tokio::test]
    async fn list_reports_every_revision_newest_first() {
        let (tool, number) = tool_with_task().await;

        let output = tool
            .call(args(number, "list"))
            .await
            .expect("list should succeed");

        assert_eq!(output.current_revision, 2);
        let revisions = output.result["revisions"]
            .as_array()
            .expect("revisions array");
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0]["revision"], 2);
        assert_eq!(revisions[0]["author_type"], "agent");
    }

    #[tokio::test]
    async fn diff_defaults_to_the_current_revision() {
        let (tool, number) = tool_with_task().await;

        let output = tool
            .call(TaskHistoryArgs {
                revision: Some(1),
                ..args(number, "diff")
            })
            .await
            .expect("diff should succeed");

        assert_eq!(output.result["to"], 2);
        let fields: Vec<&str> = output.result["changes"]
            .as_array()
            .expect("changes array")
            .iter()
            .map(|change| change["field"].as_str().expect("field name"))
            .collect();
        assert_eq!(fields, vec!["description", "priority"]);
    }

    #[tokio::test]
    async fn restore_requires_a_summary_and_appends_a_revision() {
        let (tool, number) = tool_with_task().await;

        let missing_summary = tool
            .call(TaskHistoryArgs {
                revision: Some(1),
                ..args(number, "restore")
            })
            .await;
        assert!(missing_summary.is_err(), "restore must demand a reason");

        let output = tool
            .call(TaskHistoryArgs {
                revision: Some(1),
                edit_summary: Some("The agent overwrote the spec".to_string()),
                ..args(number, "restore")
            })
            .await
            .expect("restore should succeed");

        assert_eq!(output.current_revision, 3);
        assert_eq!(output.result["restored_from"], 1);

        let task = tool
            .task_store
            .get_by_number(number)
            .await
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.description.as_deref(), Some("first draft"));
        assert_eq!(task.priority, TaskPriority::Medium);
    }

    #[tokio::test]
    async fn unknown_actions_are_rejected() {
        let (tool, number) = tool_with_task().await;
        assert!(tool.call(args(number, "rewind")).await.is_err());
    }
}
