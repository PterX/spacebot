//! Instance-wide chronicle history and daily briefs.

use super::state::ApiState;
use crate::conversation::ChronicleStore;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use std::sync::Arc;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AgentDailyBrief {
    pub agent_id: String,
    pub day: String,
    pub summary: String,
    pub event_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChronicleHistoryItem {
    pub id: String,
    pub agent_id: String,
    pub channel_id: String,
    pub title: String,
    pub summary: String,
    pub message_count: i64,
    pub covers_from: DateTime<Utc>,
    pub covers_to: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChronicleHistoryResponse {
    pub daily_briefs: Vec<AgentDailyBrief>,
    pub checkpoints: Vec<ChronicleHistoryItem>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub(super) struct ChronicleHistoryQuery {
    /// Maximum number of chronicle checkpoints to return.
    #[serde(default)]
    limit: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/chronicle",
    params(ChronicleHistoryQuery),
    responses(
        (status = 200, body = ChronicleHistoryResponse),
        (status = 500, description = "Internal server error"),
    ),
    tag = "chronicle",
)]
pub(super) async fn get_chronicle_history(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<ChronicleHistoryQuery>,
) -> Result<Json<ChronicleHistoryResponse>, StatusCode> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let pools = state.agent_pools.load();
    let mut daily_briefs = Vec::new();
    let mut checkpoints = Vec::new();

    for (agent_id, pool) in pools.iter() {
        let brief = sqlx::query_as::<_, (String, String, i64, DateTime<Utc>)>(
            "SELECT day, summary, event_count, created_at \
             FROM working_memory_daily_summaries ORDER BY day DESC LIMIT 1",
        )
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            tracing::error!(%error, %agent_id, "failed to load daily brief");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        if let Some((day, summary, event_count, created_at)) = brief {
            daily_briefs.push(AgentDailyBrief {
                agent_id: agent_id.clone(),
                day,
                summary,
                event_count,
                created_at,
            });
        }

        let agent_checkpoints = ChronicleStore::new(pool.clone())
            .list_level_zero_recent(limit as i64)
            .await
            .map_err(|error| {
                tracing::error!(%error, %agent_id, "failed to load chronicle history");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        checkpoints.extend(
            agent_checkpoints
                .into_iter()
                .map(|checkpoint| ChronicleHistoryItem {
                    id: checkpoint.id,
                    agent_id: agent_id.clone(),
                    channel_id: checkpoint.channel_id,
                    title: checkpoint.title,
                    summary: checkpoint.summary,
                    message_count: checkpoint.message_count,
                    covers_from: checkpoint.covers_from_at,
                    covers_to: checkpoint.covers_to_at,
                    created_at: checkpoint.created_at,
                }),
        );
    }

    daily_briefs.sort_by(|left, right| {
        right
            .day
            .cmp(&left.day)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.created_at));
    checkpoints.truncate(limit);

    Ok(Json(ChronicleHistoryResponse {
        daily_briefs,
        checkpoints,
    }))
}
