//! Goal listing tool. Available to channels (read-only) and branches.

use crate::goals::{Goal, GoalListFilter, GoalStatus, GoalStore, GoalTaskCounts};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct GoalListTool {
    goal_store: Arc<GoalStore>,
}

impl std::fmt::Debug for GoalListTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalListTool").finish()
    }
}

impl GoalListTool {
    pub fn new(goal_store: Arc<GoalStore>) -> Self {
        Self { goal_store }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("goal_list failed: {0}")]
pub struct GoalListError(String);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GoalListArgs {
    pub status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i32,
}

fn default_limit() -> i32 {
    50
}

#[derive(Debug, Serialize)]
pub struct GoalListEntry {
    #[serde(flatten)]
    pub goal: Goal,
    pub task_counts: GoalTaskCounts,
}

#[derive(Debug, Serialize)]
pub struct GoalListOutput {
    pub success: bool,
    pub count: usize,
    pub goals: Vec<GoalListEntry>,
}

impl Tool for GoalListTool {
    const NAME: &'static str = "goal_list";

    type Error = GoalListError;
    type Args = GoalListArgs;
    type Output = GoalListOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/goal_list").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": GoalStatus::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        "description": "Optional status filter"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of goals to return"
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let status = match args.status.as_deref() {
            None => None,
            Some(value) => Some(
                GoalStatus::parse(value)
                    .ok_or_else(|| GoalListError(format!("invalid status filter: {value}")))?,
            ),
        };
        let limit = i64::from(args.limit).clamp(1, 500);

        let goals = self
            .goal_store
            .list(GoalListFilter {
                status,
                limit: Some(limit),
            })
            .await
            .map_err(|error| GoalListError(format!("{error}")))?;

        let mut entries = Vec::with_capacity(goals.len());
        for goal in goals {
            let task_counts = self
                .goal_store
                .linked_task_counts(&goal.id)
                .await
                .map_err(|error| GoalListError(format!("{error}")))?;
            entries.push(GoalListEntry { goal, task_counts });
        }

        Ok(GoalListOutput {
            success: true,
            count: entries.len(),
            goals: entries,
        })
    }
}
