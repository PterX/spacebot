//! Per-channel settings persistence (SQLite).
//!
//! Stores `ConversationSettings` for platform channels (Discord, Slack, etc.).
//! Portal conversations store settings in `portal_conversations.settings` instead.

use super::settings::ConversationSettings;
use sqlx::{Row as _, SqlitePool};

#[derive(Debug, Clone)]
pub struct ChannelSettingsStore {
    pool: SqlitePool,
}

impl ChannelSettingsStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Get settings for a specific channel, if any have been persisted.
    pub async fn get(
        &self,
        agent_id: &str,
        conversation_id: &str,
    ) -> crate::error::Result<Option<ConversationSettings>> {
        let row = sqlx::query(
            "SELECT settings FROM channel_settings WHERE agent_id = ? AND conversation_id = ?",
        )
        .bind(agent_id)
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(row.and_then(|r| {
            r.try_get::<String, _>("settings").ok().and_then(|s| {
                if s.is_empty() || s == "{}" {
                    None
                } else {
                    serde_json::from_str(&s).ok()
                }
            })
        }))
    }

    /// Atomically set only the response mode. A single JSON-patch statement
    /// avoids the read-modify-write window in which a concurrent whole-row
    /// writer's changes could be overwritten with stale fields.
    pub async fn set_response_mode(
        &self,
        agent_id: &str,
        conversation_id: &str,
        mode: crate::conversation::settings::ResponseMode,
    ) -> crate::error::Result<()> {
        let mode_str = mode.as_setting_str()?;

        sqlx::query(
            "INSERT INTO channel_settings (agent_id, conversation_id, settings, updated_at) \
             VALUES (?, ?, json_set('{}', '$.response_mode', ?), CURRENT_TIMESTAMP) \
             ON CONFLICT (agent_id, conversation_id) \
             DO UPDATE SET settings = json_set(COALESCE(NULLIF(channel_settings.settings, ''), '{}'), '$.response_mode', ?), \
                 updated_at = CURRENT_TIMESTAMP",
        )
        .bind(agent_id)
        .bind(conversation_id)
        .bind(&mode_str)
        .bind(&mode_str)
        .execute(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(())
    }

    /// Insert or update settings for a channel.
    pub async fn upsert(
        &self,
        agent_id: &str,
        conversation_id: &str,
        settings: &ConversationSettings,
    ) -> crate::error::Result<()> {
        let settings_json = serde_json::to_string(settings).map_err(|e| anyhow::anyhow!(e))?;

        sqlx::query(
            "INSERT INTO channel_settings (agent_id, conversation_id, settings, updated_at) \
             VALUES (?, ?, ?, CURRENT_TIMESTAMP) \
             ON CONFLICT (agent_id, conversation_id) \
             DO UPDATE SET settings = excluded.settings, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(agent_id)
        .bind(conversation_id)
        .bind(&settings_json)
        .execute(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(())
    }
}
