//! Router-side control plane for slash commands.
//!
//! Control commands execute deterministically against channel state — the
//! settings stores, process control, and runtime config — without creating
//! an inbound message or consuming an agent turn. Dispatching here, before
//! the channel's message queue, is what makes them busy-immune: a `/quiet`
//! lands even while the channel is mid-turn.
//!
//! The reply-text builders are shared with the channel's in-queue dispatch
//! path so both surfaces produce identical output.

use super::registry::{ControlAction, Surface};
use crate::ProcessType;
use crate::agent::channel_prompt::TemporalContext;
use crate::conversation::settings::{ResolvedConversationSettings, ResponseMode};

/// Confirmation reply for a response-mode change.
pub fn mode_confirmation(mode: ResponseMode) -> &'static str {
    match mode {
        ResponseMode::Active => "active mode enabled. i'll respond normally in this chat.",
        ResponseMode::Observe => {
            "observe mode enabled. i'll learn from this conversation but won't respond."
        }
        ResponseMode::MentionOnly => {
            "mention-only mode enabled. i'll only respond when @mentioned or replied to."
        }
    }
}

/// Human-readable label for a response mode in `/status` output.
pub fn mode_label(mode: ResponseMode) -> &'static str {
    match mode {
        ResponseMode::Active => "active",
        ResponseMode::Observe => "observe (learning, never responds)",
        ResponseMode::MentionOnly => "mention-only (@mention/reply only)",
    }
}

/// `/status` reply body.
#[allow(clippy::too_many_arguments)]
pub fn status_text(
    agent_id: &str,
    channel_id: &str,
    adapter: &str,
    mode: ResponseMode,
    channel_model: &str,
    branch_model: &str,
    now_line: &str,
) -> String {
    format!(
        "status\n\
         - agent: {agent_id}\n\
         - channel: {channel_id}\n\
         - adapter: {adapter}\n\
         - mode: {}\n\
         - channel model: {channel_model}\n\
         - branch model: {branch_model}\n\
         - time: {now_line}",
        mode_label(mode)
    )
}

/// Everything a control command needs, owned so execution can run in a
/// spawned task without borrowing the router loop.
pub struct ControlPlane {
    pub deps: crate::AgentDeps,
    pub conversation_id: String,
    /// Runtime adapter key of the surface the command arrived on.
    pub adapter: String,
    /// Settings defaults from the matched binding, if any.
    pub binding_settings: Option<crate::conversation::ConversationSettings>,
    pub surface: Surface,
    pub is_authority: bool,
    pub is_portal: bool,
}

impl ControlPlane {
    /// Execute a control action and return the reply text.
    pub async fn execute(&self, action: ControlAction) -> String {
        match action {
            ControlAction::Status => self.status().await,
            ControlAction::SetResponseMode(mode) => self.set_response_mode(mode).await,
            ControlAction::Help => {
                crate::commands::REGISTRY.help_text_for(Some(self.surface), self.is_authority)
            }
            ControlAction::AgentId => self.deps.agent_id.to_string(),
        }
    }

    /// Resolve the conversation's settings the same way channel creation
    /// does: per-conversation DB override > binding defaults > defaults.
    async fn resolved_settings(&self) -> ResolvedConversationSettings {
        let db_settings = if self.is_portal {
            let store =
                crate::conversation::PortalConversationStore::new(self.deps.sqlite_pool.clone());
            match store.get(&self.deps.agent_id, &self.conversation_id).await {
                Ok(conversation) => conversation.and_then(|conversation| conversation.settings),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        conversation_id = %self.conversation_id,
                        "failed to load portal conversation settings for control command"
                    );
                    None
                }
            }
        } else {
            let store =
                crate::conversation::ChannelSettingsStore::new(self.deps.sqlite_pool.clone());
            match store.get(&self.deps.agent_id, &self.conversation_id).await {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        conversation_id = %self.conversation_id,
                        "failed to load channel settings for control command"
                    );
                    None
                }
            }
        };
        ResolvedConversationSettings::resolve(
            db_settings.as_ref(),
            self.binding_settings.as_ref(),
            None,
        )
    }

    /// The live channel's control handle, when one is running for this
    /// conversation.
    async fn live_channel(&self) -> Option<crate::agent::channel::ChannelControlHandle> {
        let channel_id: crate::ChannelId = std::sync::Arc::from(self.conversation_id.as_str());
        self.deps
            .process_control_registry
            .channel_handle(&channel_id)
            .await
    }

    async fn status(&self) -> String {
        let temporal_context = TemporalContext::from_runtime(self.deps.runtime_config.as_ref());
        let routing = self.deps.runtime_config.routing.load();
        let resolved = self.resolved_settings().await;
        let channel_model = resolved
            .resolve_model("channel")
            .unwrap_or_else(|| routing.resolve(ProcessType::Channel, None));
        let branch_model = resolved
            .resolve_model("branch")
            .unwrap_or_else(|| routing.resolve(ProcessType::Branch, None));
        // A live channel's in-memory mode wins over the store: mode changes
        // persist asynchronously, so the cell is ahead of the DB briefly.
        let mode = match self.live_channel().await {
            Some(handle) => handle.response_mode(),
            None => resolved.response_mode,
        };
        status_text(
            &self.deps.agent_id,
            &self.conversation_id,
            &self.adapter,
            mode,
            channel_model,
            branch_model,
            &temporal_context.current_time_line(),
        )
    }

    async fn set_response_mode(&self, mode: ResponseMode) -> String {
        if self.is_portal {
            let store =
                crate::conversation::PortalConversationStore::new(self.deps.sqlite_pool.clone());
            let settings = match store.get(&self.deps.agent_id, &self.conversation_id).await {
                Ok(Some(conversation)) => conversation.settings,
                Ok(None) | Err(_) => None,
            };
            let mut settings = settings.unwrap_or_default();
            settings.response_mode = mode;
            if let Err(error) = store
                .update(
                    &self.deps.agent_id,
                    &self.conversation_id,
                    None,
                    None,
                    Some(settings),
                )
                .await
            {
                tracing::warn!(
                    %error,
                    conversation_id = %self.conversation_id,
                    ?mode,
                    "failed to persist response_mode to portal conversation"
                );
            }
        } else {
            let store =
                crate::conversation::ChannelSettingsStore::new(self.deps.sqlite_pool.clone());
            let mut settings = match store.get(&self.deps.agent_id, &self.conversation_id).await {
                Ok(Some(existing)) => existing,
                Ok(None) => crate::conversation::ConversationSettings::default(),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        conversation_id = %self.conversation_id,
                        ?mode,
                        "failed to load existing settings before persisting response_mode"
                    );
                    crate::conversation::ConversationSettings::default()
                }
            };
            settings.response_mode = mode;
            if let Err(error) = store
                .upsert(&self.deps.agent_id, &self.conversation_id, &settings)
                .await
            {
                tracing::warn!(
                    %error,
                    conversation_id = %self.conversation_id,
                    ?mode,
                    "failed to persist response_mode to channel_settings"
                );
            }
        }

        // Poke the live channel so the change applies mid-turn, not on the
        // next channel restart.
        if let Some(handle) = self.live_channel().await {
            handle.set_response_mode_live(mode);
        }

        mode_confirmation(mode).to_string()
    }
}
