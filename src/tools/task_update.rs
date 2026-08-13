//! Task update tool for branch and worker processes.

use crate::tasks::{
    TaskPriority, TaskStatus, TaskStore, TaskSubtask, UpdateTaskInput, WorkerTaskUpdateResult,
};
use crate::{AgentId, WorkerId};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum TaskUpdateScope {
    Branch,
    Worker(WorkerId),
}

#[derive(Debug, Clone)]
pub struct TaskUpdateTool {
    task_store: Arc<TaskStore>,
    // Retained for future authorization checks on global task updates.
    #[allow(dead_code)]
    agent_id: AgentId,
    scope: TaskUpdateScope,
    working_memory: Option<Arc<crate::memory::WorkingMemoryStore>>,
}

impl TaskUpdateTool {
    pub fn for_branch(task_store: Arc<TaskStore>, agent_id: AgentId) -> Self {
        Self {
            task_store,
            agent_id,
            scope: TaskUpdateScope::Branch,
            working_memory: None,
        }
    }

    pub fn for_worker(task_store: Arc<TaskStore>, agent_id: AgentId, worker_id: WorkerId) -> Self {
        Self {
            task_store,
            agent_id,
            scope: TaskUpdateScope::Worker(worker_id),
            working_memory: None,
        }
    }

    pub fn with_working_memory(mut self, store: Arc<crate::memory::WorkingMemoryStore>) -> Self {
        self.working_memory = Some(store);
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("task_update failed: {0}")]
pub struct TaskUpdateError(String);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskUpdateArgs {
    pub task_number: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub subtasks: Option<Vec<TaskSubtask>>,
    pub metadata: Option<serde_json::Value>,
    pub complete_subtask: Option<i32>,
    pub worker_id: Option<String>,
    pub approved_by: Option<String>,
    pub worker_type: Option<String>,
    pub project_id: Option<String>,
    pub repo_id: Option<String>,
    pub worktree_mode: Option<String>,
    pub worktree_id: Option<String>,
    pub required_skills: Option<Vec<String>>,
    /// Full replacement of the task's dependency edges; omit to leave
    /// unchanged.
    pub depends_on: Option<Vec<crate::tools::task_create::TaskDependencyArg>>,
}

#[derive(Debug, Serialize)]
pub struct TaskUpdateOutput {
    pub success: bool,
    pub task_number: i64,
    pub status: String,
    pub message: String,
}

impl Tool for TaskUpdateTool {
    const NAME: &'static str = "task_update";

    type Error = TaskUpdateError;
    type Args = TaskUpdateArgs;
    type Output = TaskUpdateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let is_worker = matches!(self.scope, TaskUpdateScope::Worker(_));

        // Workers only see subtask/metadata fields; branches/cortex see everything.
        let parameters = if is_worker {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task_number": { "type": "integer", "description": "Task number reference (#N)" },
                    "subtasks": {
                        "type": "array",
                        "description": "Optional full replacement of subtask list",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "completed": { "type": "boolean" }
                            },
                            "required": ["title", "completed"]
                        }
                    },
                    "metadata": { "type": "object", "description": "Metadata object deep-merged with current metadata" },
                    "complete_subtask": { "type": "integer", "description": "Subtask index to mark complete" }
                },
                "required": ["task_number"]
            })
        } else {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task_number": { "type": "integer", "description": "Task number reference (#N)" },
                    "title": { "type": "string", "description": "Optional new title" },
                    "description": { "type": "string", "description": "Optional new description" },
                    "status": {
                        "type": "string",
                        "enum": crate::tasks::TaskStatus::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        "description": "Optional new status"
                    },
                    "priority": {
                        "type": "string",
                        "enum": crate::tasks::TaskPriority::ALL.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                        "description": "Optional new priority"
                    },
                    "subtasks": {
                        "type": "array",
                        "description": "Optional full replacement of subtask list",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "completed": { "type": "boolean" }
                            },
                            "required": ["title", "completed"]
                        }
                    },
                    "metadata": { "type": "object", "description": "Metadata object deep-merged with current metadata" },
                    "complete_subtask": { "type": "integer", "description": "Subtask index to mark complete" },
                    "worker_id": { "type": "string", "description": "Optional worker ID to bind to this task" },
                    "approved_by": { "type": "string", "description": "Optional approver identifier" },
                    "worker_type": {
                        "type": "string",
                        "enum": crate::tasks::TaskWorkerType::ALL.iter().map(|w| w.to_string()).collect::<Vec<_>>(),
                        "description": "Execution plan: which worker kind runs this task"
                    },
                    "project_id": { "type": "string", "description": "Execution plan: project ID the work belongs to" },
                    "repo_id": { "type": "string", "description": "Execution plan: repo ID within the project" },
                    "worktree_mode": {
                        "type": "string",
                        "enum": crate::tasks::TaskWorktreeMode::ALL.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
                        "description": "Execution plan: root, existing, or create"
                    },
                    "worktree_id": { "type": "string", "description": "Execution plan: existing worktree ID" },
                    "required_skills": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Full replacement of the task's required skills list"
                    },
                    "depends_on": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "task": { "type": "integer", "description": "Task number this task depends on" },
                                "kind": {
                                    "type": "string",
                                    "enum": crate::tasks::TaskDependencyKind::ALL.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
                                    "description": "\"gate\" (default) waits for done; \"stack\" waits only for the dependency's branch"
                                }
                            },
                            "required": ["task"]
                        },
                        "description": "Full replacement of this task's dependency edges"
                    }
                },
                "required": ["task_number"]
            })
        };

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/task_update").to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let task_number = i64::from(args.task_number);
        if matches!(self.scope, TaskUpdateScope::Worker(_))
            && (args.title.is_some()
                || args.description.is_some()
                || args.status.is_some()
                || args.priority.is_some()
                || args.worker_id.is_some()
                || args.approved_by.is_some()
                || args.worker_type.is_some()
                || args.project_id.is_some()
                || args.repo_id.is_some()
                || args.worktree_mode.is_some()
                || args.worktree_id.is_some()
                || args.required_skills.is_some()
                || args.depends_on.is_some())
        {
            return Err(TaskUpdateError(
                "workers can only update subtasks and metadata".to_string(),
            ));
        }

        let status = match args.status.as_deref() {
            None => None,
            Some(value) => Some(
                TaskStatus::parse(value)
                    .ok_or_else(|| TaskUpdateError(format!("invalid status: {value}")))?,
            ),
        };
        let priority = match args.priority.as_deref() {
            None => None,
            Some(value) => Some(
                TaskPriority::parse(value)
                    .ok_or_else(|| TaskUpdateError(format!("invalid priority: {value}")))?,
            ),
        };
        let complete_subtask = match args.complete_subtask {
            None => None,
            Some(value) => Some(
                usize::try_from(value)
                    .map_err(|_| TaskUpdateError(format!("invalid subtask index: {value}")))?,
            ),
        };

        let worker_type = match args.worker_type.as_deref() {
            None => None,
            Some(value) => Some(
                crate::tasks::TaskWorkerType::parse(value)
                    .ok_or_else(|| TaskUpdateError(format!("invalid worker_type: {value}")))?,
            ),
        };
        let worktree_mode = match args.worktree_mode.as_deref() {
            None => None,
            Some(value) => Some(
                crate::tasks::TaskWorktreeMode::parse(value)
                    .ok_or_else(|| TaskUpdateError(format!("invalid worktree_mode: {value}")))?,
            ),
        };

        let input = UpdateTaskInput {
            title: args.title,
            description: args.description,
            status,
            priority,
            subtasks: args.subtasks,
            metadata: args.metadata,
            worker_id: args.worker_id,
            clear_worker_id: false,
            approved_by: args.approved_by,
            complete_subtask,
            assigned_agent_id: None,
            worker_type,
            project_id: args.project_id,
            repo_id: args.repo_id,
            worktree_mode,
            worktree_id: args.worktree_id,
            required_skills: args.required_skills,
        };

        let edges = args
            .depends_on
            .as_deref()
            .map(crate::tools::task_create::parse_dependency_args)
            .transpose()
            .map_err(TaskUpdateError)?;

        let update_result = match &self.scope {
            TaskUpdateScope::Branch => self
                .task_store
                .update_with_dependencies_and_status_transition(
                    task_number,
                    input,
                    edges.as_deref(),
                )
                .await
                .map_err(|error| TaskUpdateError(format!("{error}")))?
                .ok_or_else(|| TaskUpdateError(format!("task #{} not found", task_number)))?,
            TaskUpdateScope::Worker(worker_id) => match self
                .task_store
                .update_worker_task(&worker_id.to_string(), task_number, input)
                .await
                .map_err(|error| TaskUpdateError(format!("{error}")))?
            {
                WorkerTaskUpdateResult::Updated(result) => *result,
                WorkerTaskUpdateResult::NotAssigned => {
                    return Err(TaskUpdateError(
                        "worker is not assigned to a task".to_string(),
                    ));
                }
                WorkerTaskUpdateResult::WrongTask {
                    assigned_task_number,
                } => {
                    return Err(TaskUpdateError(format!(
                        "worker {} can only update task #{}",
                        worker_id, assigned_task_number
                    )));
                }
            },
        };
        let previous_status = update_result.previous_status;
        let updated = update_result.task;

        if let Some(working_memory) = &self.working_memory {
            let transitioned_to_done =
                previous_status != TaskStatus::Done && updated.status == TaskStatus::Done;
            let (event_type, summary, importance) = if transitioned_to_done {
                (
                    crate::memory::WorkingMemoryEventType::Outcome,
                    format!("Task #{} completed", updated.task_number),
                    0.7,
                )
            } else {
                (
                    crate::memory::WorkingMemoryEventType::TaskUpdate,
                    format!(
                        "Task #{} updated to {}",
                        updated.task_number, updated.status
                    ),
                    0.4,
                )
            };
            working_memory
                .emit(event_type, summary)
                .importance(importance)
                .record();
        }

        Ok(TaskUpdateOutput {
            success: true,
            task_number: updated.task_number,
            status: updated.status.to_string(),
            message: format!("Updated task #{}", updated.task_number),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::memory::working::WorkingMemoryEvent;
    use crate::memory::{WorkingMemoryEventType, WorkingMemoryStore};
    use crate::tasks::store::setup_test_store;
    use chrono_tz::Tz;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::time::Duration;

    async fn setup_working_memory() -> Arc<WorkingMemoryStore> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite connect");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        WorkingMemoryStore::new(pool, Tz::UTC)
    }

    async fn wait_for_single_event(store: &WorkingMemoryStore) -> WorkingMemoryEvent {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let events = store
                    .get_recent_events(10, 0.0)
                    .await
                    .expect("working memory query");
                if let Some(event) = events.into_iter().next() {
                    break event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for working memory event")
    }

    #[tokio::test]
    async fn task_update_emits_outcome_for_done_status() {
        let task_store = Arc::new(setup_test_store().await);
        let working_memory = setup_working_memory().await;

        let created = task_store
            .create(crate::tasks::CreateTaskInput {
                owner_agent_id: "agent-test".to_string(),
                assigned_agent_id: Some("agent-test".to_string()),
                title: "Review PR 2".to_string(),
                description: None,
                status: TaskStatus::InProgress,
                priority: TaskPriority::Medium,
                subtasks: Vec::new(),
                metadata: serde_json::json!({}),
                source_memory_id: None,
                created_by: "branch".to_string(),
                ..Default::default()
            })
            .await
            .expect("task should be created");

        let tool = TaskUpdateTool::for_branch(task_store, AgentId::from("agent-test"))
            .with_working_memory(working_memory.clone());

        let output = tool
            .call(TaskUpdateArgs {
                task_number: created.task_number as i32,
                title: None,
                description: None,
                status: Some("done".to_string()),
                priority: None,
                subtasks: None,
                metadata: None,
                complete_subtask: None,
                worker_id: None,
                approved_by: None,
                worker_type: None,
                project_id: None,
                repo_id: None,
                worktree_mode: None,
                worktree_id: None,
                required_skills: None,
                depends_on: None,
            })
            .await
            .expect("task update should succeed");

        assert_eq!(output.status, "done");

        let event = wait_for_single_event(&working_memory).await;
        assert_eq!(event.event_type, WorkingMemoryEventType::Outcome);
        assert_eq!(
            event.summary,
            format!("Task #{} completed", created.task_number)
        );
    }

    #[tokio::test]
    async fn task_update_keeps_task_update_event_when_task_was_already_done() {
        let task_store = Arc::new(setup_test_store().await);
        let working_memory = setup_working_memory().await;

        let created = task_store
            .create(crate::tasks::CreateTaskInput {
                owner_agent_id: "agent-test".to_string(),
                assigned_agent_id: Some("agent-test".to_string()),
                title: "Review merged changes".to_string(),
                description: None,
                status: TaskStatus::Done,
                priority: TaskPriority::Medium,
                subtasks: Vec::new(),
                metadata: serde_json::json!({}),
                source_memory_id: None,
                created_by: "branch".to_string(),
                ..Default::default()
            })
            .await
            .expect("task should be created");

        let tool = TaskUpdateTool::for_branch(task_store, AgentId::from("agent-test"))
            .with_working_memory(working_memory.clone());

        let output = tool
            .call(TaskUpdateArgs {
                task_number: created.task_number as i32,
                title: Some("Review merged changes carefully".to_string()),
                description: None,
                status: None,
                priority: None,
                subtasks: None,
                metadata: None,
                complete_subtask: None,
                worker_id: None,
                approved_by: None,
                worker_type: None,
                project_id: None,
                repo_id: None,
                worktree_mode: None,
                worktree_id: None,
                required_skills: None,
                depends_on: None,
            })
            .await
            .expect("task update should succeed");

        assert_eq!(output.status, "done");

        let event = wait_for_single_event(&working_memory).await;
        assert_eq!(event.event_type, WorkingMemoryEventType::TaskUpdate);
        assert_eq!(
            event.summary,
            format!("Task #{} updated to done", created.task_number)
        );
    }

    #[tokio::test]
    async fn worker_scope_checks_assignment_before_global_task_lookup() {
        let task_store = Arc::new(setup_test_store().await);
        let assigned = task_store
            .create(crate::tasks::CreateTaskInput {
                owner_agent_id: "agent-test".to_string(),
                assigned_agent_id: Some("agent-test".to_string()),
                title: "Assigned task".to_string(),
                description: None,
                status: TaskStatus::InProgress,
                priority: TaskPriority::Medium,
                subtasks: Vec::new(),
                metadata: serde_json::json!({}),
                source_memory_id: None,
                created_by: "branch".to_string(),
                ..Default::default()
            })
            .await
            .expect("assigned task should be created");
        let other = task_store
            .create(crate::tasks::CreateTaskInput {
                owner_agent_id: "agent-test".to_string(),
                assigned_agent_id: Some("agent-test".to_string()),
                title: "Other task".to_string(),
                description: None,
                status: TaskStatus::InProgress,
                priority: TaskPriority::Medium,
                subtasks: Vec::new(),
                metadata: serde_json::json!({}),
                source_memory_id: None,
                created_by: "branch".to_string(),
                ..Default::default()
            })
            .await
            .expect("other task should be created");
        let worker_id = WorkerId::new_v4();
        task_store
            .update(
                assigned.task_number,
                crate::tasks::UpdateTaskInput {
                    worker_id: Some(worker_id.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("worker assignment should update");

        let tool = TaskUpdateTool::for_worker(task_store, AgentId::from("agent-test"), worker_id);

        let existing_foreign = tool
            .call(TaskUpdateArgs {
                task_number: other.task_number as i32,
                title: None,
                description: None,
                status: None,
                priority: None,
                subtasks: None,
                metadata: Some(serde_json::json!({"progress": "checked"})),
                complete_subtask: None,
                worker_id: None,
                approved_by: None,
                worker_type: None,
                project_id: None,
                repo_id: None,
                worktree_mode: None,
                worktree_id: None,
                required_skills: None,
                depends_on: None,
            })
            .await
            .expect_err("foreign task should be rejected");
        let missing = tool
            .call(TaskUpdateArgs {
                task_number: 999,
                title: None,
                description: None,
                status: None,
                priority: None,
                subtasks: None,
                metadata: Some(serde_json::json!({"progress": "checked"})),
                complete_subtask: None,
                worker_id: None,
                approved_by: None,
                worker_type: None,
                project_id: None,
                repo_id: None,
                worktree_mode: None,
                worktree_id: None,
                required_skills: None,
                depends_on: None,
            })
            .await
            .expect_err("missing task should be rejected the same way");

        assert_eq!(existing_foreign.to_string(), missing.to_string());
        assert_eq!(
            existing_foreign.to_string(),
            format!(
                "task_update failed: worker {} can only update task #{}",
                worker_id, assigned.task_number
            )
        );
    }
}
