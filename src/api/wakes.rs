//! Wake definition endpoints and public webhook ingress.
//!
//! The authenticated surface lists, tunes, test-fires, and deletes wake
//! definitions through the per-agent stores in the wake registry. Webhook
//! ingress lives here too but is registered outside the protected router:
//! the per-wake bearer token in the path is the authority boundary, per
//! `docs/design-docs/wakes.md`.

use super::state::ApiState;
use crate::config::AutonomyLevel;
use crate::wakes::{WakeDef, WakeTrigger};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Id of the read-only virtual entry derived from the agent's live
/// `AutonomyConfig`. The autonomy dial governs it, so it has no `wake_defs`
/// row and rejects writes.
const INTERVAL_SURVEY_WAKE_ID: &str = "interval-survey";

/// Upper bound on webhook ingress bodies. The payload is persisted per
/// event row, so this caps row size. Enforced at the transport level via a
/// route-scoped `DefaultBodyLimit` where the ingress route is registered,
/// so oversized bodies are rejected before they are buffered.
pub(super) const MAX_WEBHOOK_BODY_BYTES: usize = 64 * 1024;

type ApiError = (StatusCode, Json<Value>);

fn error_response(status: StatusCode, message: &str) -> ApiError {
    (status, Json(json!({ "error": message })))
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct WakesQuery {
    agent_id: String,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct WakeItem {
    pub id: String,
    pub name: String,
    /// "schedule", "webhook", or "event".
    pub trigger_kind: String,
    /// Human-readable trigger summary: the cron expression, "every 15m",
    /// the event name, or "webhook".
    pub trigger_label: String,
    pub instructions: String,
    pub min_level: AutonomyLevel,
    pub enabled: bool,
    pub builtin: bool,
    /// Derived from live config rather than a `wake_defs` row; read-only.
    #[serde(rename = "virtual")]
    pub is_virtual: bool,
    pub last_fired_at: Option<String>,
    /// Public ingress path, webhook wakes only.
    pub webhook_url: Option<String>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct WakesResponse {
    pub wakes: Vec<WakeItem>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct WakeUpdateRequest {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    min_level: Option<AutonomyLevel>,
}

/// Look up an agent's deps in the wake registry.
async fn agent_deps(state: &ApiState, agent_id: &str) -> Option<crate::AgentDeps> {
    let key: crate::AgentId = Arc::from(agent_id);
    state.wake_registry.read().await.get(&key).cloned()
}

fn interval_label(secs: u64) -> String {
    if secs >= 3600 && secs.is_multiple_of(3600) {
        format!("every {}h", secs / 3600)
    } else if secs >= 60 && secs.is_multiple_of(60) {
        format!("every {}m", secs / 60)
    } else {
        format!("every {secs}s")
    }
}

fn trigger_label(trigger: &WakeTrigger) -> String {
    match trigger {
        WakeTrigger::Schedule {
            cron_expr: Some(expr),
            ..
        } => expr.clone(),
        WakeTrigger::Schedule {
            interval_secs: Some(secs),
            ..
        } => interval_label(*secs),
        WakeTrigger::Schedule { .. } => "schedule".to_string(),
        WakeTrigger::Webhook => "webhook".to_string(),
        WakeTrigger::Event { event } => event.as_str().to_string(),
    }
}

fn item_from_def(def: &WakeDef) -> WakeItem {
    WakeItem {
        id: def.id.clone(),
        name: def.name.clone(),
        trigger_kind: def.trigger.kind().to_string(),
        trigger_label: trigger_label(&def.trigger),
        instructions: def.instructions.clone(),
        min_level: def.min_level,
        enabled: def.enabled,
        builtin: def.builtin,
        is_virtual: false,
        last_fired_at: def.last_fired_at.clone(),
        webhook_url: def
            .webhook_token
            .as_deref()
            .map(|token| format!("/hooks/wakes/{token}")),
    }
}

/// The virtual interval-survey entry derived from the agent's live autonomy
/// config. Enabled tracks the dial: any level above `off` runs the survey.
fn interval_survey_item(config: &crate::config::AutonomyConfig) -> WakeItem {
    WakeItem {
        id: INTERVAL_SURVEY_WAKE_ID.to_string(),
        name: "Interval survey".to_string(),
        trigger_kind: "schedule".to_string(),
        trigger_label: interval_label(config.interval_secs),
        instructions: "Survey task and goal state on the configured interval.".to_string(),
        min_level: AutonomyLevel::Observe,
        enabled: config.level != AutonomyLevel::Off,
        builtin: true,
        is_virtual: true,
        last_fired_at: None,
        webhook_url: None,
    }
}

/// Ring the agent's wake doorbell after an enqueue. Best-effort: liveness
/// only, durability already comes from the persisted event row.
fn ring_doorbell(deps: &crate::AgentDeps) {
    if let Some(wake_tx) = &deps.wake_tx {
        crate::agent::wake::fire_wake(wake_tx, &deps.agent_id);
    }
}

/// List wake definitions for an agent, virtual interval survey first.
#[utoipa::path(
    get,
    path = "/agents/wakes",
    params(WakesQuery),
    responses(
        (status = 200, body = WakesResponse),
        (status = 404, description = "Agent not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "wakes",
)]
pub(super) async fn list_wakes(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<WakesQuery>,
) -> Result<Json<WakesResponse>, StatusCode> {
    let deps = agent_deps(&state, &query.agent_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let config = **deps.runtime_config.autonomy.load();

    let defs = deps.wake_def_store.list().await.map_err(|error| {
        tracing::warn!(%error, agent_id = %query.agent_id, "failed to list wake defs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut wakes = Vec::with_capacity(defs.len() + 1);
    wakes.push(interval_survey_item(&config));

    for mut def in defs {
        // Lazy minting keeps config-seeded webhook wakes usable without a
        // separate provisioning step. The insert-if-absent guard means a
        // concurrent list re-reads the winner's token instead of clobbering.
        if def.trigger == WakeTrigger::Webhook && def.webhook_token.is_none() {
            let token = uuid::Uuid::new_v4().to_string();
            let minted = deps
                .wake_def_store
                .set_webhook_token_if_absent(&def.id, &token)
                .await;
            def.webhook_token = match minted {
                Ok(true) => Some(token),
                Ok(false) => deps
                    .wake_def_store
                    .get(&def.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|row| row.webhook_token),
                Err(error) => {
                    tracing::warn!(%error, wake_id = %def.id, "failed to mint webhook token");
                    None
                }
            };
        }
        wakes.push(item_from_def(&def));
    }

    Ok(Json(WakesResponse { wakes }))
}

/// Tune a wake definition. Built-in rows accept enabled, instructions, and
/// min_level but keep their name; the virtual interval survey rejects all
/// writes.
#[utoipa::path(
    put,
    path = "/agents/wakes/{wake_id}",
    params(("wake_id" = String, Path, description = "Wake definition id"), WakesQuery),
    request_body = WakeUpdateRequest,
    responses(
        (status = 200, body = WakeItem),
        (status = 400, description = "Virtual wake, or a rename of a built-in wake"),
        (status = 404, description = "Agent or wake not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "wakes",
)]
pub(super) async fn update_wake(
    State(state): State<Arc<ApiState>>,
    Path(wake_id): Path<String>,
    Query(query): Query<WakesQuery>,
    Json(body): Json<WakeUpdateRequest>,
) -> Result<Json<WakeItem>, ApiError> {
    if wake_id == INTERVAL_SURVEY_WAKE_ID {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "the interval survey is governed by the autonomy dial",
        ));
    }

    let deps = agent_deps(&state, &query.agent_id)
        .await
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown agent"))?;

    let def = deps
        .wake_def_store
        .get(&wake_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to load wake def");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load wake")
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown wake"))?;

    if def.builtin && body.name.is_some() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "built-in wakes cannot be renamed",
        ));
    }

    deps.wake_def_store
        .update_tuning(
            &wake_id,
            body.name.as_deref(),
            body.instructions.as_deref(),
            body.min_level,
            body.enabled,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to update wake def");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update wake")
        })?;

    let updated = deps
        .wake_def_store
        .get(&wake_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to reload wake def");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load wake")
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown wake"))?;

    Ok(Json(item_from_def(&updated)))
}

/// Manually test-fire a wake through the authenticated API.
#[utoipa::path(
    post,
    path = "/agents/wakes/{wake_id}/fire",
    params(("wake_id" = String, Path, description = "Wake definition id"), WakesQuery),
    responses(
        (status = 202, description = "Wake event queued"),
        (status = 400, description = "Virtual wake"),
        (status = 404, description = "Agent or wake not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "wakes",
)]
pub(super) async fn fire_wake(
    State(state): State<Arc<ApiState>>,
    Path(wake_id): Path<String>,
    Query(query): Query<WakesQuery>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if wake_id == INTERVAL_SURVEY_WAKE_ID {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "the interval survey cannot be fired manually",
        ));
    }

    let deps = agent_deps(&state, &query.agent_id)
        .await
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown agent"))?;

    deps.wake_def_store
        .get(&wake_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to load wake def");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load wake")
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown wake"))?;

    // A unique dedupe key per fire: manual test fires must not coalesce
    // into each other or into pending organic events.
    let dedupe_key = format!("manual:{}", uuid::Uuid::new_v4());
    deps.wake_event_store
        .enqueue(&wake_id, &dedupe_key, &json!({ "fired_by": "api" }))
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to enqueue manual wake event");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to enqueue wake")
        })?;

    if let Err(error) = deps.wake_def_store.touch_last_fired(&wake_id).await {
        tracing::warn!(%error, %wake_id, "failed to touch wake last_fired_at");
    }
    ring_doorbell(&deps);

    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "queued" }))))
}

/// Delete a user-owned wake definition.
#[utoipa::path(
    delete,
    path = "/agents/wakes/{wake_id}",
    params(("wake_id" = String, Path, description = "Wake definition id"), WakesQuery),
    responses(
        (status = 200, description = "Wake deleted"),
        (status = 400, description = "Virtual or built-in wake"),
        (status = 404, description = "Agent or wake not found"),
        (status = 409, description = "Config-owned wake"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "wakes",
)]
pub(super) async fn delete_wake(
    State(state): State<Arc<ApiState>>,
    Path(wake_id): Path<String>,
    Query(query): Query<WakesQuery>,
) -> Result<Json<Value>, ApiError> {
    if wake_id == INTERVAL_SURVEY_WAKE_ID {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "the interval survey is governed by the autonomy dial",
        ));
    }

    let deps = agent_deps(&state, &query.agent_id)
        .await
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown agent"))?;

    let def = deps
        .wake_def_store
        .get(&wake_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to load wake def");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load wake")
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "unknown wake"))?;

    if def.builtin {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "built-in wakes can be disabled but not deleted",
        ));
    }
    if def.config_owned {
        return Err(error_response(
            StatusCode::CONFLICT,
            "this wake is owned by a [[wakes]] entry; remove it from config.toml instead",
        ));
    }

    deps.wake_def_store
        .delete(&wake_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %wake_id, "failed to delete wake def");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete wake")
        })?;

    Ok(Json(json!({ "success": true })))
}

/// Public webhook ingress: `POST /hooks/wakes/{token}`.
///
/// Registered outside the protected router — the per-wake bearer token in
/// the path is the authority boundary. The body is stored as event payload
/// data and never interpreted; runs consume it, per the design doc's rule
/// that webhook payloads never carry authority or instructions.
pub(super) async fn webhook_ingress(
    State(state): State<Arc<ApiState>>,
    Path(token): Path<String>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    // Defense in depth: the route-scoped body limit at registration rejects
    // oversized bodies with 413 before they are buffered here.
    if body.len() > MAX_WEBHOOK_BODY_BYTES {
        return Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "webhook payload exceeds 64 KiB",
        ));
    }
    // Any valid JSON is accepted as the payload; empty or non-JSON bodies
    // degrade to an empty object.
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));

    // Snapshot the registry so the per-agent store scans run without
    // holding the lock.
    let registry: Vec<crate::AgentDeps> =
        state.wake_registry.read().await.values().cloned().collect();

    // A failing lookup on one agent must not mask a match on another, so
    // scan every agent and only report the failure when nothing matched.
    let mut lookup_failed = false;
    for deps in registry {
        let def = match deps.wake_def_store.find_by_webhook_token(&token).await {
            Ok(Some(def)) => def,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, agent_id = %deps.agent_id, "webhook token lookup failed");
                lookup_failed = true;
                continue;
            }
        };

        if !def.enabled {
            return Err(error_response(StatusCode::CONFLICT, "wake is disabled"));
        }

        // The empty dedupe key coalesces a delivery burst into one pending
        // event with a delivery count — the doc's flood behavior.
        deps.wake_event_store
            .enqueue(&def.id, "", &payload)
            .await
            .map_err(|error| {
                tracing::warn!(%error, wake_id = %def.id, "failed to enqueue webhook wake event");
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to enqueue wake")
            })?;

        if let Err(error) = deps.wake_def_store.touch_last_fired(&def.id).await {
            tracing::warn!(%error, wake_id = %def.id, "failed to touch wake last_fired_at");
        }
        ring_doorbell(&deps);

        return Ok((StatusCode::ACCEPTED, Json(json!({ "status": "queued" }))));
    }

    if lookup_failed {
        return Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "wake lookup failed",
        ));
    }
    Err(error_response(StatusCode::NOT_FOUND, "unknown token"))
}
