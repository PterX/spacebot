//! Goal CRUD endpoints.
//!
//! Goals are user-defined objectives, instance-scoped like tasks. Completion
//! is user-initiated: the user closes a goal here (or in the UI) by setting
//! `status: "completed"` via `PUT /goals/{id}`.

use super::state::ApiState;
use crate::goals::{
    CreateGoalInput, Goal, GoalListFilter, GoalStatus, GoalStore, GoalTaskCounts, UpdateGoalInput,
};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct GoalListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default = "default_goal_limit")]
    limit: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct CreateGoalRequest {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    /// Optional deadline as YYYY-MM-DD.
    #[serde(default)]
    due_date: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct UpdateGoalRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// New status. Completion happens here: set `"completed"` to close a goal.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    /// New deadline as YYYY-MM-DD, or empty string to clear.
    #[serde(default)]
    due_date: Option<String>,
    /// Replacement progress notes, or empty string to clear.
    #[serde(default)]
    notes: Option<String>,
    /// Object patch deep-merged into the current metadata.
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct GoalWithCounts {
    #[serde(flatten)]
    pub goal: Goal,
    pub task_counts: GoalTaskCounts,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct GoalListResponse {
    pub goals: Vec<GoalWithCounts>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct GoalResponse {
    pub goal: GoalWithCounts,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_goal_limit() -> i64 {
    100
}

/// Extract the goal store, returning 503 if not yet initialized.
fn get_goal_store(state: &ApiState) -> Result<Arc<GoalStore>, StatusCode> {
    state
        .goal_store
        .load()
        .as_ref()
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

fn parse_status(value: Option<&str>) -> Result<Option<GoalStatus>, StatusCode> {
    match value {
        None => Ok(None),
        Some(value) => Ok(Some(
            GoalStatus::parse(value).ok_or(StatusCode::BAD_REQUEST)?,
        )),
    }
}

fn parse_priority(value: Option<&str>) -> Result<Option<crate::tasks::TaskPriority>, StatusCode> {
    match value {
        None => Ok(None),
        Some(value) => Ok(Some(
            crate::tasks::TaskPriority::parse(value).ok_or(StatusCode::BAD_REQUEST)?,
        )),
    }
}

async fn with_counts(store: &GoalStore, goal: Goal) -> Result<GoalWithCounts, StatusCode> {
    let task_counts = store.linked_task_counts(&goal.id).await.map_err(|error| {
        tracing::warn!(%error, goal_id = %goal.id, "failed to count linked tasks");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(GoalWithCounts { goal, task_counts })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /goals` — list goals with optional status filter.
#[utoipa::path(
    get,
    path = "/goals",
    params(GoalListQuery),
    responses(
        (status = 200, body = GoalListResponse),
        (status = 400, description = "Invalid status filter"),
        (status = 503, description = "Goal store not initialized"),
    ),
    tag = "goals",
)]
pub(super) async fn list_goals(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<GoalListQuery>,
) -> Result<Json<GoalListResponse>, StatusCode> {
    let store = get_goal_store(&state)?;
    let status = parse_status(query.status.as_deref())?;

    let goals = store
        .list(GoalListFilter {
            status,
            limit: Some(query.limit.clamp(1, 500)),
        })
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to list goals");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut entries = Vec::with_capacity(goals.len());
    for goal in goals {
        entries.push(with_counts(&store, goal).await?);
    }

    Ok(Json(GoalListResponse { goals: entries }))
}

/// `GET /goals/{id}` — get a goal by id.
#[utoipa::path(
    get,
    path = "/goals/{id}",
    params(
        ("id" = String, Path, description = "Goal id"),
    ),
    responses(
        (status = 200, body = GoalResponse),
        (status = 404, description = "Goal not found"),
        (status = 503, description = "Goal store not initialized"),
    ),
    tag = "goals",
)]
pub(super) async fn get_goal(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<GoalResponse>, StatusCode> {
    let store = get_goal_store(&state)?;

    let goal = store
        .get(&id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, goal_id = %id, "failed to get goal");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(GoalResponse {
        goal: with_counts(&store, goal).await?,
    }))
}

/// `POST /goals` — create a goal. New goals start active.
#[utoipa::path(
    post,
    path = "/goals",
    request_body = CreateGoalRequest,
    responses(
        (status = 200, body = GoalResponse),
        (status = 400, description = "Invalid request"),
        (status = 503, description = "Goal store not initialized"),
    ),
    tag = "goals",
)]
pub(super) async fn create_goal(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateGoalRequest>,
) -> Result<Json<GoalResponse>, StatusCode> {
    let store = get_goal_store(&state)?;
    let priority =
        parse_priority(request.priority.as_deref())?.unwrap_or(crate::tasks::TaskPriority::Medium);

    let goal = store
        .create(CreateGoalInput {
            title: request.title,
            description: request.description,
            priority,
            due_date: request.due_date,
            metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
        })
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to create goal");
            StatusCode::BAD_REQUEST
        })?;

    Ok(Json(GoalResponse {
        goal: with_counts(&store, goal).await?,
    }))
}

/// `PUT /goals/{id}` — update a goal. Status changes follow the goal
/// lifecycle (active ↔ paused, active → completed, active → abandoned);
/// user-initiated completion happens here via `status: "completed"`.
#[utoipa::path(
    put,
    path = "/goals/{id}",
    params(
        ("id" = String, Path, description = "Goal id"),
    ),
    request_body = UpdateGoalRequest,
    responses(
        (status = 200, body = GoalResponse),
        (status = 400, description = "Invalid request or status transition"),
        (status = 404, description = "Goal not found"),
        (status = 503, description = "Goal store not initialized"),
    ),
    tag = "goals",
)]
pub(super) async fn update_goal(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateGoalRequest>,
) -> Result<Json<GoalResponse>, StatusCode> {
    let store = get_goal_store(&state)?;
    let status = parse_status(request.status.as_deref())?;
    let priority = parse_priority(request.priority.as_deref())?;

    let goal = store
        .update(
            &id,
            UpdateGoalInput {
                title: request.title,
                description: request.description,
                status,
                priority,
                due_date: request.due_date,
                notes: request.notes,
                metadata: request.metadata,
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, goal_id = %id, "failed to update goal");
            StatusCode::BAD_REQUEST
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(GoalResponse {
        goal: with_counts(&store, goal).await?,
    }))
}
