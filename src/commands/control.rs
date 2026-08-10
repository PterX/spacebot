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

/// Short mode name for failure replies.
fn mode_name(mode: ResponseMode) -> &'static str {
    match mode {
        ResponseMode::Active => "active",
        ResponseMode::Observe => "observe",
        ResponseMode::MentionOnly => "mention-only",
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
        // On persistence failure the change is abandoned — no live update,
        // and the reply reports the failure instead of confirming a mode
        // that never persisted.
        if let Err(error) = persist_response_mode(
            &self.deps.sqlite_pool,
            &self.deps.agent_id,
            &self.conversation_id,
            self.is_portal,
            mode,
        )
        .await
        {
            tracing::warn!(
                %error,
                conversation_id = %self.conversation_id,
                ?mode,
                "failed to persist response mode"
            );
            return format!(
                "couldn't switch to {} mode — settings persistence failed, mode is unchanged",
                mode_name(mode)
            );
        }

        // Poke the live channel so the change applies mid-turn, not on the
        // next channel restart.
        if let Some(handle) = self.live_channel().await {
            handle.set_response_mode_live(mode);
        }

        mode_confirmation(mode).to_string()
    }
}

/// Persist the response mode through the stores' atomic field updates,
/// leaving every other settings field untouched. No settings read happens
/// here, so concurrent whole-row writers can't be clobbered with stale
/// fields, and an unreadable record can't be replaced with defaults.
pub(crate) async fn persist_response_mode(
    pool: &sqlx::SqlitePool,
    agent_id: &str,
    conversation_id: &str,
    is_portal: bool,
    mode: ResponseMode,
) -> anyhow::Result<()> {
    if is_portal {
        let store = crate::conversation::PortalConversationStore::new(pool.clone());
        let updated = store
            .set_response_mode(agent_id, conversation_id, mode)
            .await?;
        if !updated {
            anyhow::bail!("portal conversation {conversation_id} not found");
        }
    } else {
        let store = crate::conversation::ChannelSettingsStore::new(pool.clone());
        store
            .set_response_mode(agent_id, conversation_id, mode)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn memory_pool() -> sqlx::SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite should connect")
    }

    async fn create_channel_settings_table(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "CREATE TABLE channel_settings (
                agent_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                settings TEXT NOT NULL DEFAULT '{}',
                updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (agent_id, conversation_id)
            )",
        )
        .execute(pool)
        .await
        .expect("channel_settings table should create");
    }

    async fn create_portal_conversations_table(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "CREATE TABLE portal_conversations (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                title TEXT NOT NULL,
                title_source TEXT NOT NULL DEFAULT 'system',
                archived INTEGER NOT NULL DEFAULT 0,
                settings TEXT,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            )",
        )
        .execute(pool)
        .await
        .expect("portal_conversations table should create");
    }

    #[tokio::test]
    async fn channel_mode_change_preserves_other_settings() {
        let pool = memory_pool().await;
        create_channel_settings_table(&pool).await;

        let store = crate::conversation::ChannelSettingsStore::new(pool.clone());
        let existing = crate::conversation::ConversationSettings {
            model: Some("special-model".into()),
            ..Default::default()
        };
        store.upsert("agent", "conv", &existing).await.unwrap();

        persist_response_mode(&pool, "agent", "conv", false, ResponseMode::Observe)
            .await
            .unwrap();

        let loaded = store.get("agent", "conv").await.unwrap().unwrap();
        assert_eq!(loaded.response_mode, ResponseMode::Observe);
        assert_eq!(
            loaded.model.as_deref(),
            Some("special-model"),
            "a mode change must not clobber other persisted settings"
        );
    }

    #[tokio::test]
    async fn channel_store_failure_propagates_instead_of_writing_defaults() {
        // No channel_settings table: the settings read fails, and the
        // failure must surface instead of turning into a default upsert
        // and a false confirmation.
        let pool = memory_pool().await;
        let result =
            persist_response_mode(&pool, "agent", "conv", false, ResponseMode::Observe).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn portal_mode_change_preserves_other_settings() {
        let pool = memory_pool().await;
        create_portal_conversations_table(&pool).await;

        let store = crate::conversation::PortalConversationStore::new(pool.clone());
        store.ensure("agent", "session").await.unwrap();
        let existing = crate::conversation::ConversationSettings {
            model: Some("special-model".into()),
            ..Default::default()
        };
        store
            .update("agent", "session", None, None, Some(existing))
            .await
            .unwrap();

        persist_response_mode(&pool, "agent", "session", true, ResponseMode::MentionOnly)
            .await
            .unwrap();

        let loaded = store
            .get("agent", "session")
            .await
            .unwrap()
            .unwrap()
            .settings
            .unwrap();
        assert_eq!(loaded.response_mode, ResponseMode::MentionOnly);
        assert_eq!(loaded.model.as_deref(), Some("special-model"));
    }

    #[tokio::test]
    async fn portal_mode_change_initializes_null_settings() {
        let pool = memory_pool().await;
        create_portal_conversations_table(&pool).await;

        let store = crate::conversation::PortalConversationStore::new(pool.clone());
        store.ensure("agent", "session").await.unwrap();

        persist_response_mode(&pool, "agent", "session", true, ResponseMode::Observe)
            .await
            .unwrap();

        let loaded = store
            .get("agent", "session")
            .await
            .unwrap()
            .unwrap()
            .settings
            .unwrap();
        assert_eq!(loaded.response_mode, ResponseMode::Observe);
    }

    #[tokio::test]
    async fn channel_mode_change_handles_empty_string_settings() {
        let pool = memory_pool().await;
        create_channel_settings_table(&pool).await;

        sqlx::query(
            "INSERT INTO channel_settings (agent_id, conversation_id, settings) VALUES (?, ?, '')",
        )
        .bind("agent")
        .bind("conv")
        .execute(&pool)
        .await
        .unwrap();

        persist_response_mode(&pool, "agent", "conv", false, ResponseMode::MentionOnly)
            .await
            .unwrap();

        let store = crate::conversation::ChannelSettingsStore::new(pool.clone());
        let loaded = store.get("agent", "conv").await.unwrap().unwrap();
        assert_eq!(loaded.response_mode, ResponseMode::MentionOnly);
    }

    #[tokio::test]
    async fn portal_missing_conversation_fails_without_writing() {
        let pool = memory_pool().await;
        create_portal_conversations_table(&pool).await;

        let result =
            persist_response_mode(&pool, "agent", "missing", true, ResponseMode::Observe).await;
        assert!(result.is_err());

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM portal_conversations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "a failed mode change must not create records");
    }
}
