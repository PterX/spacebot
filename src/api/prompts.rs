//! Prompt record API — the captured requests behind the inspector.

use super::state::ApiState;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Resolve an agent's record store, defaulting to the only agent when the
/// caller did not name one.
fn record_store(
    state: &ApiState,
    agent_id: Option<&str>,
) -> Result<Arc<crate::llm::PromptRecordStore>, StatusCode> {
    let configs = state.runtime_configs.load();

    let runtime_config = match agent_id {
        Some(agent_id) => configs.get(agent_id),
        None => configs.values().next(),
    }
    .ok_or(StatusCode::NOT_FOUND)?;

    runtime_config
        .prompt_records
        .load()
        .as_ref()
        .clone()
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct ListQuery {
    agent_id: Option<String>,
    /// Restrict to one channel's requests.
    channel_id: Option<String>,
    /// Restrict to one branch or worker's requests.
    process_id: Option<String>,
    /// Every request produced by one conversation message.
    message_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    100
}

/// List captured requests, newest first.
#[utoipa::path(
    get,
    path = "/prompts",
    params(ListQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 404, description = "No record store for this agent"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "prompts",
)]
pub(super) async fn list_prompt_requests(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = record_store(&state, query.agent_id.as_deref())?;

    let rows = match query.message_id {
        Some(ref message_id) => store.for_message(message_id).await,
        None => {
            store
                .list(
                    query.channel_id.as_deref(),
                    query.process_id.as_deref(),
                    query.limit.clamp(1, 1000),
                )
                .await
        }
    }
    .map_err(|error| {
        tracing::warn!(%error, "failed to list prompt requests");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({
        "capture_enabled": store.is_enabled(),
        "requests": rows,
    })))
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct GetQuery {
    agent_id: Option<String>,
    /// Full request id, or any unambiguous prefix of one.
    request_id: String,
}

/// Fetch one captured request in full: system prompt, block map, tool
/// definitions, message history, response and usage.
#[utoipa::path(
    get,
    path = "/prompts/get",
    params(GetQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 404, description = "No such request"),
        (status = 409, description = "Ambiguous request id prefix"),
    ),
    tag = "prompts",
)]
pub(super) async fn get_prompt_request(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<GetQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let store = record_store(&state, query.agent_id.as_deref())?;

    match store.get(&query.request_id).await {
        Ok(Some(record)) => Ok(Json(
            serde_json::to_value(&record).unwrap_or(serde_json::Value::Null),
        )),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        // An ambiguous prefix is the caller's to fix; a failed query, unreadable
        // payload or unparseable record is the store failing and must not be
        // reported as a caller mistake.
        Err(crate::llm::record::LookupError::Ambiguous(id)) => {
            tracing::debug!(request_id = %id, "prompt record id is ambiguous");
            Err(StatusCode::CONFLICT)
        }
        Err(error) => {
            tracing::warn!(%error, "failed to load prompt record");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct CaptureSettings {
    pub enabled: bool,
    pub retention_days: i64,
}

/// Read the instance-wide capture setting.
#[utoipa::path(
    get,
    path = "/prompts/capture",
    params(("agent_id" = Option<String>, Query, description = "Agent to read")),
    responses((status = 200, body = CaptureSettings), (status = 404, description = "Agent not found")),
    tag = "prompts",
)]
pub(super) async fn get_prompt_debug_capture(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<AgentQuery>,
) -> Result<Json<CaptureSettings>, StatusCode> {
    let settings = settings_store(&state, query.agent_id.as_deref())?;

    Ok(Json(CaptureSettings {
        enabled: settings.prompt_debug_capture(),
        retention_days: settings.prompt_debug_retention_days(),
    }))
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct AgentQuery {
    agent_id: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct CaptureBody {
    agent_id: Option<String>,
    enabled: bool,
    /// Days of records to keep. Omitted leaves the current retention alone.
    retention_days: Option<i64>,
}

/// Turn request capture on or off for the whole instance.
#[utoipa::path(
    post,
    path = "/prompts/capture",
    request_body = CaptureBody,
    responses(
        (status = 200, body = CaptureSettings),
        (status = 404, description = "Agent not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "prompts",
)]
pub(super) async fn set_prompt_debug_capture(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<CaptureBody>,
) -> Result<Json<CaptureSettings>, StatusCode> {
    let retention = body.retention_days.filter(|days| *days > 0);

    // Capture is an instance-wide switch, so every agent is written and every
    // agent's live flag is set. Persisting one agent while flipping the flag
    // for all of them would look correct until a restart, then silently revert
    // for the agents that were never written.
    let configs = state.runtime_configs.load();
    if configs.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    for (agent_id, runtime_config) in configs.iter() {
        let Some(settings) = runtime_config.settings.load().as_ref().clone() else {
            tracing::warn!(agent_id, "no settings store; prompt capture not persisted");
            continue;
        };

        settings
            .set_prompt_debug_capture(body.enabled)
            .map_err(|error| {
                tracing::warn!(%error, agent_id, "failed to persist prompt capture setting");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        if let Some(days) = retention {
            settings
                .set_prompt_debug_retention_days(days)
                .map_err(|error| {
                    tracing::warn!(%error, agent_id, "failed to persist prompt retention setting");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        }

        // The store holds the flag the model layer reads, so it has to be told
        // too — persisting alone would not take effect until a restart.
        if let Some(store) = runtime_config.prompt_records.load().as_ref() {
            store.set_enabled(body.enabled);
        }
    }

    let settings = settings_store(&state, body.agent_id.as_deref())?;
    Ok(Json(CaptureSettings {
        enabled: body.enabled,
        retention_days: settings.prompt_debug_retention_days(),
    }))
}

fn settings_store(
    state: &ApiState,
    agent_id: Option<&str>,
) -> Result<Arc<crate::settings::SettingsStore>, StatusCode> {
    let configs = state.runtime_configs.load();

    let runtime_config = match agent_id {
        Some(agent_id) => configs.get(agent_id),
        None => configs.values().next(),
    }
    .ok_or(StatusCode::NOT_FOUND)?;

    runtime_config
        .settings
        .load()
        .as_ref()
        .clone()
        .ok_or(StatusCode::NOT_FOUND)
}
