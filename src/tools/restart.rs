//! Restart tool: lets the model reboot the spacebot daemon in place.
//!
//! The restart fires after the lifecycle grace delay so this tool's result
//! (and any final reply) can flush first. When the tool carries channel
//! context, the restarted daemon announces itself back in that channel via
//! the persisted [`PendingRestart`] marker.

use crate::lifecycle::{LifecycleHandle, PendingRestart, SHUTDOWN_GRACE};
use crate::messaging::target::BroadcastTarget;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::path::PathBuf;

/// Tool that requests an in-place daemon restart.
#[derive(Clone)]
pub struct RestartTool {
    lifecycle: LifecycleHandle,
    instance_dir: PathBuf,
    agent_id: String,
    source: &'static str,
    conversation_id: Option<String>,
    delivery: Option<BroadcastTarget>,
}

impl std::fmt::Debug for RestartTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestartTool")
            .field("agent_id", &self.agent_id)
            .field("source", &self.source)
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl RestartTool {
    pub fn new(
        lifecycle: LifecycleHandle,
        instance_dir: PathBuf,
        agent_id: impl Into<String>,
        source: &'static str,
    ) -> Self {
        Self {
            lifecycle,
            instance_dir,
            agent_id: agent_id.into(),
            source,
            conversation_id: None,
            delivery: None,
        }
    }

    /// Attach the originating channel so the restarted daemon can announce
    /// itself back there. Without this the restart completes silently.
    pub fn with_channel_context(
        mut self,
        conversation_id: impl Into<String>,
        delivery: Option<BroadcastTarget>,
    ) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self.delivery = delivery;
        self
    }
}

/// Error type for the restart tool.
#[derive(Debug, thiserror::Error)]
#[error("Restart failed: {0}")]
pub struct RestartError(String);

/// Arguments for the restart tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestartArgs {
    /// Why the daemon is being restarted. Audit-logged and included in the
    /// post-restart announcement.
    pub reason: String,
}

/// Output from the restart tool.
#[derive(Debug, Serialize)]
pub struct RestartOutput {
    pub status: &'static str,
    pub message: String,
}

impl Tool for RestartTool {
    const NAME: &'static str = "restart";

    type Error = RestartError;
    type Args = RestartArgs;
    type Output = RestartOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/restart").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Why the daemon is being restarted. Audit-logged and included in the post-restart announcement."
                    }
                },
                "required": ["reason"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let reason = args.reason.trim();
        if reason.is_empty() {
            return Err(RestartError("a non-empty reason is required".to_string()));
        }

        if !self.lifecycle.request_restart(reason) {
            return Ok(RestartOutput {
                status: "already_pending",
                message: "A restart or shutdown is already pending.".to_string(),
            });
        }

        let pending = PendingRestart {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone().unwrap_or_default(),
            adapter: self.delivery.as_ref().map(|d| d.adapter.clone()),
            target: self.delivery.as_ref().map(|d| d.target.clone()),
            reason: reason.to_string(),
            requested_at: chrono::Utc::now(),
            source: self.source.to_string(),
        };
        if let Err(error) = pending.write(&self.instance_dir) {
            tracing::warn!(%error, "failed to persist restart context; restart proceeds without announcement");
        }

        tracing::info!(
            agent_id = %self.agent_id,
            source = self.source,
            reason,
            "daemon restart requested by tool"
        );

        Ok(RestartOutput {
            status: "restarting",
            message: format!(
                "Restart scheduled in {}s. All in-flight workers and branches will be \
                 terminated; the daemon re-execs in place and comes back with the same \
                 configuration.",
                SHUTDOWN_GRACE.as_secs()
            ),
        })
    }
}
