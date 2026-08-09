//! Autonomy status and run-history endpoints for the control interface.
//!
//! Read-only surface over the per-agent autonomy stores threaded through
//! `AgentDeps` (via the wake registry) and the hot-reloaded `RuntimeConfig`.
//! Level and tuning writes ride the agent-config path in `api::config`.

use super::state::ApiState;
use crate::config::AutonomyLevel;
use crate::wakes::{AutonomyRun, AutonomyRunStatus};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How many recent runs to inspect when deriving a status snapshot. Enough to
/// find the running row and the most recent finished run.
const STATUS_RUN_WINDOW: u32 = 10;

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct AutonomyStatusQuery {
    agent_id: String,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct AutonomyRunsQuery {
    /// Agent to list runs for. Omit to aggregate runs across all agents.
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default = "default_runs_limit")]
    limit: u32,
}

fn default_runs_limit() -> u32 {
    20
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct AutonomyCurrentRun {
    pub started_at: String,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct AutonomyStatusResponse {
    pub agent_id: String,
    pub level: AutonomyLevel,
    pub interval_secs: u64,
    pub active_hours: Option<(u8, u8)>,
    pub max_tasks_per_run: u32,
    /// When the most recent finished run started.
    pub last_run_at: Option<String>,
    /// Summary of the most recent finished run.
    pub last_run_summary: Option<String>,
    /// Interval anchor: last run start + interval, clamped to now when
    /// overdue. `null` when the level is `off`.
    pub next_run_at: Option<String>,
    /// The in-flight run, when one is active.
    pub current_run: Option<AutonomyCurrentRun>,
    pub pending_wake_events: i64,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct AutonomyFleetResponse {
    pub agents: Vec<AutonomyStatusResponse>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct AutonomyRunEntry {
    pub agent_id: String,
    #[serde(flatten)]
    pub run: AutonomyRun,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct AutonomyRunsResponse {
    pub runs: Vec<AutonomyRunEntry>,
}

/// Look up an agent's deps in the wake registry.
async fn agent_deps(state: &ApiState, agent_id: &str) -> Option<crate::AgentDeps> {
    let key: crate::AgentId = Arc::from(agent_id);
    state.wake_registry.read().await.get(&key).cloned()
}

/// Build a status snapshot for one agent from its stores and live config.
async fn build_status(
    agent_id: &str,
    deps: &crate::AgentDeps,
) -> crate::error::Result<AutonomyStatusResponse> {
    let config = **deps.runtime_config.autonomy.load();
    let recent = deps.autonomy_run_store.recent(STATUS_RUN_WINDOW).await?;
    let pending_wake_events = deps.wake_event_store.pending_count().await?;

    let current_run = recent
        .iter()
        .find(|run| run.status == AutonomyRunStatus::Running)
        .map(|run| AutonomyCurrentRun {
            started_at: run.started_at.clone(),
        });
    let last_finished = recent
        .iter()
        .find(|run| run.status != AutonomyRunStatus::Running);

    let next_run_at = if config.level == AutonomyLevel::Off {
        None
    } else {
        let now = chrono::Utc::now();
        // The newest run (running or finished) anchors the interval, matching
        // the driver's `last_run_started_at` semantics. A never-run agent is
        // immediately due.
        let next = recent
            .first()
            .and_then(|run| crate::wakes::parse_run_timestamp(&run.started_at))
            .map(|started| {
                (started + chrono::Duration::seconds(config.interval_secs as i64)).max(now)
            })
            .unwrap_or(now);
        Some(next.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    };

    Ok(AutonomyStatusResponse {
        agent_id: agent_id.to_string(),
        level: config.level,
        interval_secs: config.interval_secs,
        active_hours: config.active_hours,
        max_tasks_per_run: config.max_tasks_per_run,
        last_run_at: last_finished.map(|run| run.started_at.clone()),
        last_run_summary: last_finished.and_then(|run| run.summary.clone()),
        next_run_at,
        current_run,
        pending_wake_events,
    })
}

/// Get the autonomy status for a single agent.
#[utoipa::path(
    get,
    path = "/agents/autonomy",
    params(AutonomyStatusQuery),
    responses(
        (status = 200, body = AutonomyStatusResponse),
        (status = 404, description = "Agent not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "autonomy",
)]
pub(super) async fn autonomy_status(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<AutonomyStatusQuery>,
) -> Result<Json<AutonomyStatusResponse>, StatusCode> {
    let deps = agent_deps(&state, &query.agent_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let status = build_status(&query.agent_id, &deps)
        .await
        .map_err(|error| {
            tracing::warn!(%error, agent_id = %query.agent_id, "failed to build autonomy status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(status))
}

/// Get the autonomy status for every agent, in agent-list order.
#[utoipa::path(
    get,
    path = "/agents/autonomy/fleet",
    responses(
        (status = 200, body = AutonomyFleetResponse),
        (status = 500, description = "Internal server error"),
    ),
    tag = "autonomy",
)]
pub(super) async fn autonomy_fleet(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<AutonomyFleetResponse>, StatusCode> {
    let agent_ids: Vec<String> = state
        .agent_configs
        .load()
        .iter()
        .map(|info| info.id.clone())
        .collect();

    let mut agents = Vec::with_capacity(agent_ids.len());
    for agent_id in agent_ids {
        // Agents mid-removal may be absent from the registry; skip them
        // rather than failing the whole fleet snapshot.
        let Some(deps) = agent_deps(&state, &agent_id).await else {
            continue;
        };
        let status = build_status(&agent_id, &deps).await.map_err(|error| {
            tracing::warn!(%error, %agent_id, "failed to build autonomy status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        agents.push(status);
    }

    Ok(Json(AutonomyFleetResponse { agents }))
}

/// List recent autonomy runs, newest first. Scoped to one agent when
/// `agent_id` is given, aggregated across all agents otherwise.
#[utoipa::path(
    get,
    path = "/agents/autonomy/runs",
    params(AutonomyRunsQuery),
    responses(
        (status = 200, body = AutonomyRunsResponse),
        (status = 404, description = "Agent not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "autonomy",
)]
pub(super) async fn autonomy_runs(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<AutonomyRunsQuery>,
) -> Result<Json<AutonomyRunsResponse>, StatusCode> {
    let limit = query.limit.clamp(1, 100);

    let mut runs: Vec<AutonomyRunEntry> = Vec::new();
    match &query.agent_id {
        Some(agent_id) => {
            let deps = agent_deps(&state, agent_id)
                .await
                .ok_or(StatusCode::NOT_FOUND)?;
            let agent_runs = deps
                .autonomy_run_store
                .recent(limit)
                .await
                .map_err(|error| {
                    tracing::warn!(%error, %agent_id, "failed to list autonomy runs");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            runs.extend(agent_runs.into_iter().map(|run| AutonomyRunEntry {
                agent_id: agent_id.clone(),
                run,
            }));
        }
        None => {
            let registry: Vec<(String, crate::AgentDeps)> = state
                .wake_registry
                .read()
                .await
                .iter()
                .map(|(id, deps)| (id.to_string(), deps.clone()))
                .collect();

            for (agent_id, deps) in registry {
                let agent_runs = deps
                    .autonomy_run_store
                    .recent(limit)
                    .await
                    .map_err(|error| {
                        tracing::warn!(%error, %agent_id, "failed to list autonomy runs");
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;
                runs.extend(agent_runs.into_iter().map(|run| AutonomyRunEntry {
                    agent_id: agent_id.clone(),
                    run,
                }));
            }
            runs.sort_by(|a, b| {
                b.run
                    .started_at
                    .cmp(&a.run.started_at)
                    .then_with(|| b.run.id.cmp(&a.run.id))
            });
            runs.truncate(limit as usize);
        }
    }

    Ok(Json(AutonomyRunsResponse { runs }))
}
