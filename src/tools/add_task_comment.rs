//! Append a comment to a task's discussion thread.
//!
//! Branches and the cortex may comment on any task. A worker may only comment
//! on the task it is bound to — the same scoping `task_update` applies.

use crate::tasks::{CreateTaskCommentInput, TaskAuthorKind, TaskStore};
use crate::{AgentId, WorkerId};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub enum TaskCommentScope {
    Branch,
    Worker(WorkerId),
}

#[derive(Clone)]
pub struct AddTaskCommentTool {
    task_store: Arc<TaskStore>,
    agent_id: AgentId,
    scope: TaskCommentScope,
    api_state: Option<Arc<crate::api::ApiState>>,
}

impl AddTaskCommentTool {
    pub fn for_branch(task_store: Arc<TaskStore>, agent_id: AgentId) -> Self {
        Self {
            task_store,
            agent_id,
            scope: TaskCommentScope::Branch,
            api_state: None,
        }
    }

    pub fn for_worker(task_store: Arc<TaskStore>, agent_id: AgentId, worker_id: WorkerId) -> Self {
        Self {
            task_store,
            agent_id,
            scope: TaskCommentScope::Worker(worker_id),
            api_state: None,
        }
    }

    pub fn with_api_state(mut self, api_state: Arc<crate::api::ApiState>) -> Self {
        self.api_state = Some(api_state);
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("add_task_comment failed: {0}")]
pub struct AddTaskCommentError(String);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddTaskCommentArgs {
    pub task_number: i32,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct AddTaskCommentOutput {
    pub success: bool,
    pub task_number: i64,
    pub comment_id: String,
    pub message: String,
}

impl Tool for AddTaskCommentTool {
    const NAME: &'static str = "add_task_comment";

    type Error = AddTaskCommentError;
    type Args = AddTaskCommentArgs;
    type Output = AddTaskCommentOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/add_task_comment").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_number": { "type": "integer", "description": "Task number reference (#N)" },
                    "body": {
                        "type": "string",
                        "description": format!(
                            "What you found or decided, in {}-{} characters. Comments are permanent and cannot be edited.",
                            crate::tasks::MIN_COMMENT_BODY_CHARS,
                            crate::tasks::MAX_COMMENT_BODY_BYTES,
                        ),
                    }
                },
                "required": ["task_number", "body"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let task_number = i64::from(args.task_number);

        let task = self
            .task_store
            .get_by_number(task_number)
            .await
            .map_err(|error| AddTaskCommentError(format!("{error}")))?
            .ok_or_else(|| AddTaskCommentError(format!("task #{task_number} not found")))?;

        let (author_type, author_id) = match &self.scope {
            TaskCommentScope::Branch => (TaskAuthorKind::Agent, self.agent_id.to_string()),
            TaskCommentScope::Worker(worker_id) => {
                if task.worker_id.as_deref() != Some(&worker_id.to_string()) {
                    return Err(AddTaskCommentError(format!(
                        "worker {worker_id} can only comment on the task it is bound to"
                    )));
                }
                (TaskAuthorKind::Worker, worker_id.to_string())
            }
        };
        let worker_id = match &self.scope {
            TaskCommentScope::Branch => None,
            TaskCommentScope::Worker(worker_id) => Some(worker_id.to_string()),
        };

        let comment = self
            .task_store
            .add_comment(CreateTaskCommentInput {
                task_number,
                author_type,
                author_id: Some(author_id),
                body: args.body,
                worker_id,
                metadata: serde_json::json!({}),
            })
            .await
            .map_err(|error| AddTaskCommentError(format!("{error}")))?;

        if let Some(api_state) = &self.api_state {
            api_state
                .event_tx
                .send(crate::api::ApiEvent::TaskCommented {
                    agent_id: task.effective_agent_id().to_string(),
                    task_number,
                    comment_id: comment.id.clone(),
                    seq: comment.seq,
                    author_type: comment.author_type.to_string(),
                })
                .ok();
        }

        Ok(AddTaskCommentOutput {
            success: true,
            task_number,
            comment_id: comment.id,
            message: format!("Commented on task #{task_number}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TaskStatus;
    use crate::tasks::store::{CreateTaskInput, UpdateTaskInput, setup_test_store};

    async fn store_with_task() -> (Arc<crate::tasks::TaskStore>, i64) {
        let store = Arc::new(setup_test_store().await);
        let task = store
            .create(CreateTaskInput {
                owner_agent_id: "main".to_string(),
                assigned_agent_id: Some("main".to_string()),
                title: "discussed".to_string(),
                status: TaskStatus::Backlog,
                created_by: "human".to_string(),
                ..Default::default()
            })
            .await
            .expect("task should be created");
        (store, task.task_number)
    }

    #[tokio::test]
    async fn a_branch_comment_is_attributed_to_its_agent() {
        let (store, number) = store_with_task().await;
        let tool = AddTaskCommentTool::for_branch(store.clone(), Arc::from("main"));

        tool.call(AddTaskCommentArgs {
            task_number: number as i32,
            body: "Scoped this to notes/ after reading the issue.".to_string(),
        })
        .await
        .expect("comment should be written");

        let comments = store
            .list_comments(number, 10, None)
            .await
            .expect("comments should load");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author_type, TaskAuthorKind::Agent);
        assert_eq!(comments[0].author_id.as_deref(), Some("main"));
        assert_eq!(comments[0].worker_id, None);
    }

    #[tokio::test]
    async fn a_worker_can_only_comment_on_the_task_it_is_bound_to() {
        let (store, number) = store_with_task().await;
        let worker_id: crate::WorkerId = uuid::Uuid::new_v4();
        let tool = AddTaskCommentTool::for_worker(store.clone(), Arc::from("main"), worker_id);

        // Unbound: the write is refused rather than silently attributed.
        assert!(
            tool.call(AddTaskCommentArgs {
                task_number: number as i32,
                body: "findings from an unbound worker".to_string(),
            })
            .await
            .is_err()
        );

        store
            .update_with_status_transition(
                number,
                UpdateTaskInput {
                    worker_id: Some(worker_id.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("binding should succeed");

        tool.call(AddTaskCommentArgs {
            task_number: number as i32,
            body: "The migration needed a backfill; added one.".to_string(),
        })
        .await
        .expect("bound worker should be able to comment");

        let comments = store
            .list_comments(number, 10, None)
            .await
            .expect("comments should load");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author_type, TaskAuthorKind::Worker);
        assert_eq!(
            comments[0].worker_id.as_deref(),
            Some(worker_id.to_string().as_str())
        );
    }

    #[tokio::test]
    async fn commenting_on_a_missing_task_fails() {
        let (store, _) = store_with_task().await;
        let tool = AddTaskCommentTool::for_branch(store, Arc::from("main"));

        assert!(
            tool.call(AddTaskCommentArgs {
                task_number: 999,
                body: "into the void".to_string(),
            })
            .await
            .is_err()
        );
    }
}
