//! Home channel tool: points the instance's proactive outreach at this chat.
//!
//! Registered on user channels only, and resolves the conversation it was
//! called from rather than taking a target argument — the intent ("make this
//! your home") is expressible in a sentence, so the model calls this directly.
//! `/sethome` is a second entry point over the same handler.

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tool that sets the calling conversation as the home channel.
#[derive(Clone)]
pub struct SetHomeChannelTool {
    deps: crate::AgentDeps,
    conversation_id: String,
    is_portal: bool,
}

impl std::fmt::Debug for SetHomeChannelTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetHomeChannelTool")
            .field("conversation_id", &self.conversation_id)
            .field("is_portal", &self.is_portal)
            .finish_non_exhaustive()
    }
}

impl SetHomeChannelTool {
    pub fn new(
        deps: crate::AgentDeps,
        conversation_id: impl Into<String>,
        is_portal: bool,
    ) -> Self {
        Self {
            deps,
            conversation_id: conversation_id.into(),
            is_portal,
        }
    }
}

/// Error type for the home channel tool.
#[derive(Debug, thiserror::Error)]
#[error("Setting the home channel failed: {0}")]
pub struct SetHomeChannelError(String);

/// The tool takes no arguments — it resolves the calling conversation.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetHomeChannelArgs {}

/// Output from the home channel tool.
#[derive(Debug, Serialize)]
pub struct SetHomeChannelOutput {
    pub message: String,
}

impl Tool for SetHomeChannelTool {
    const NAME: &'static str = "set_home_channel";

    type Error = SetHomeChannelError;
    type Args = SetHomeChannelArgs;
    type Output = SetHomeChannelOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/set_home_channel").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let message = crate::commands::control::set_home_channel(
            &self.deps,
            &self.conversation_id,
            self.is_portal,
        )
        .await;

        Ok(SetHomeChannelOutput { message })
    }
}
