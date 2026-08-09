//! Goal creation tool for branch processes.

use crate::goals::{CreateGoalInput, GoalStore};
use crate::tasks::TaskPriority;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct GoalCreateTool {
    goal_store: Arc<GoalStore>,
    working_memory: Option<Arc<crate::memory::WorkingMemoryStore>>,
}

impl std::fmt::Debug for GoalCreateTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalCreateTool").finish()
    }
}

impl GoalCreateTool {
    pub fn new(goal_store: Arc<GoalStore>) -> Self {
        Self {
            goal_store,
            working_memory: None,
        }
    }

    pub fn with_working_memory(mut self, store: Arc<crate::memory::WorkingMemoryStore>) -> Self {
        self.working_memory = Some(store);
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("goal_create failed: {0}")]
pub struct GoalCreateError(String);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GoalCreateArgs {
    pub title: String,
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

fn default_priority() -> String {
    "medium".to_string()
}

#[derive(Debug, Serialize)]
pub struct GoalCreateOutput {
    pub success: bool,
    pub goal_id: String,
    pub status: String,
    pub message: String,
}

impl Tool for GoalCreateTool {
    const NAME: &'static str = "goal_create";

    type Error = GoalCreateError;
    type Args = GoalCreateArgs;
    type Output = GoalCreateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/goal_create").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "One-line goal label" },
                    "description": { "type": "string", "description": "Context and acceptance criteria — what does success look like?" },
                    "priority": {
                        "type": "string",
                        "enum": TaskPriority::ALL.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                        "description": "Goal priority"
                    },
                    "due_date": { "type": "string", "description": "Optional deadline as YYYY-MM-DD" },
                    "metadata": { "type": "object", "description": "Optional metadata object for external references" }
                },
                "required": ["title"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let priority = TaskPriority::parse(&args.priority)
            .ok_or_else(|| GoalCreateError(format!("invalid priority: {}", args.priority)))?;

        let goal = self
            .goal_store
            .create(CreateGoalInput {
                title: args.title,
                description: args.description,
                priority,
                due_date: args.due_date,
                metadata: args.metadata.unwrap_or_else(|| serde_json::json!({})),
            })
            .await
            .map_err(|error| GoalCreateError(format!("{error}")))?;

        if let Some(working_memory) = &self.working_memory {
            working_memory
                .emit(
                    crate::memory::WorkingMemoryEventType::Decision,
                    format!("Goal created: {} (priority: {})", goal.title, goal.priority),
                )
                .importance(0.5)
                .record();
        }

        Ok(GoalCreateOutput {
            success: true,
            goal_id: goal.id,
            status: goal.status.to_string(),
            message: format!("Created goal: {}", goal.title),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::goals::store::setup_test_store;

    #[tokio::test]
    async fn goal_create_returns_active_goal() {
        let goal_store = Arc::new(setup_test_store().await);
        let tool = GoalCreateTool::new(goal_store.clone());

        let output = tool
            .call(GoalCreateArgs {
                title: "Migrate auth to Clerk".to_string(),
                description: Some("All endpoints on Clerk middleware".to_string()),
                priority: "high".to_string(),
                due_date: Some("2026-05-01".to_string()),
                metadata: None,
            })
            .await
            .expect("goal create should succeed");

        assert_eq!(output.status, "active");

        let goal = goal_store
            .get(&output.goal_id)
            .await
            .expect("get should succeed")
            .expect("goal should exist");
        assert_eq!(goal.priority, TaskPriority::High);
        assert_eq!(goal.due_date.as_deref(), Some("2026-05-01"));
    }

    #[tokio::test]
    async fn goal_create_rejects_invalid_priority() {
        let goal_store = Arc::new(setup_test_store().await);
        let tool = GoalCreateTool::new(goal_store);

        let error = tool
            .call(GoalCreateArgs {
                title: "bad".to_string(),
                description: None,
                priority: "urgent".to_string(),
                due_date: None,
                metadata: None,
            })
            .await
            .expect_err("invalid priority must fail");

        assert!(error.to_string().contains("invalid priority"));
    }
}
