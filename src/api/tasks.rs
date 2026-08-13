use super::state::ApiState;
use crate::notifications::{NewNotification, NotificationKind, NotificationSeverity};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub(super) struct TaskListQuery {
    /// Convenience filter: matches tasks where owner OR assigned equals this value.
    #[serde(default)]
    agent_id: Option<String>,
    /// Filter by owner agent. Optional.
    #[serde(default)]
    owner_agent_id: Option<String>,
    /// Filter by assigned agent. Optional.
    #[serde(default)]
    assigned_agent_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default = "default_task_limit")]
    limit: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct CreateTaskRequest {
    /// Agent that owns (created) this task.
    owner_agent_id: String,
    /// Agent assigned to execute. Defaults to `owner_agent_id`.
    #[serde(default)]
    assigned_agent_id: Option<String>,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    subtasks: Vec<crate::tasks::TaskSubtask>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    source_memory_id: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default)]
    worker_type: Option<crate::tasks::TaskWorkerType>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_mode: Option<crate::tasks::TaskWorktreeMode>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    required_skills: Vec<String>,
    #[serde(default)]
    depends_on: Vec<TaskDependencyRequest>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct TaskDependencyRequest {
    task: i64,
    #[serde(default)]
    kind: Option<crate::tasks::TaskDependencyKind>,
}

fn dependency_edges(
    requests: &[TaskDependencyRequest],
) -> Vec<(i64, crate::tasks::TaskDependencyKind)> {
    requests
        .iter()
        .map(|r| {
            (
                r.task,
                r.kind.unwrap_or(crate::tasks::TaskDependencyKind::Gate),
            )
        })
        .collect()
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct UpdateTaskRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    assigned_agent_id: Option<String>,
    #[serde(default)]
    subtasks: Option<Vec<crate::tasks::TaskSubtask>>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    complete_subtask: Option<usize>,
    #[serde(default)]
    worker_id: Option<String>,
    #[serde(default)]
    approved_by: Option<String>,
    #[serde(default)]
    worker_type: Option<crate::tasks::TaskWorkerType>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    worktree_mode: Option<crate::tasks::TaskWorktreeMode>,
    #[serde(default)]
    worktree_id: Option<String>,
    #[serde(default)]
    required_skills: Option<Vec<String>>,
    /// Full replacement of dependency edges; omit to leave unchanged.
    #[serde(default)]
    depends_on: Option<Vec<TaskDependencyRequest>>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct ApproveRequest {
    #[serde(default)]
    approved_by: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct AssignRequest {
    assigned_agent_id: String,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskListResponse {
    pub tasks: Vec<crate::tasks::Task>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskResponse {
    pub task: crate::tasks::Task,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TaskActionResponse {
    pub success: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_task_limit() -> i64 {
    100
}

/// Extract the global task store, returning 503 if not yet initialized.
fn get_task_store(state: &ApiState) -> Result<Arc<crate::tasks::TaskStore>, StatusCode> {
    state
        .task_store
        .load()
        .as_ref()
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

fn parse_status(value: Option<&str>) -> Result<Option<crate::tasks::TaskStatus>, StatusCode> {
    match value {
        None => Ok(None),
        Some(value) => Ok(Some(
            crate::tasks::TaskStatus::parse(value).ok_or(StatusCode::BAD_REQUEST)?,
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

fn emit_task_event(state: &ApiState, task: &crate::tasks::Task, action: &str) {
    state
        .event_tx
        .send(super::state::ApiEvent::TaskUpdated {
            agent_id: task.effective_agent_id().to_string(),
            task_number: task.task_number,
            status: task.status.to_string(),
            action: action.to_string(),
        })
        .ok();
}

/// Emit a task_approval notification when a task enters the pending_approval state.
fn maybe_emit_approval_notification(state: &ApiState, task: &crate::tasks::Task) {
    if task.status != crate::tasks::TaskStatus::PendingApproval {
        return;
    }
    state.emit_notification(NewNotification {
        kind: NotificationKind::TaskApproval,
        severity: NotificationSeverity::Info,
        title: task.title.clone(),
        body: task.description.clone(),
        agent_id: Some(task.effective_agent_id().to_string()),
        related_entity_type: Some("task".to_string()),
        related_entity_id: Some(task.task_number.to_string()),
        action_url: Some(format!("/tasks/{}", task.task_number)),
        metadata: None,
    });
}

/// Post-mutation fan-out shared by the task handlers: SSE event, approval
/// notification, and — when the mutation transitioned the task onto Ready —
/// a task-approved system event routed to the owning agent's wake queue.
async fn finish_task_mutation(
    state: &ApiState,
    task: &crate::tasks::Task,
    action: &str,
    previous_status: Option<crate::tasks::TaskStatus>,
) {
    emit_task_event(state, task, action);
    maybe_emit_approval_notification(state, task);

    let landed_on_ready = task.status == crate::tasks::TaskStatus::Ready
        && previous_status.is_some_and(|previous| previous != crate::tasks::TaskStatus::Ready);
    if !landed_on_ready {
        return;
    }

    let key: crate::AgentId = Arc::from(task.effective_agent_id());
    let deps = state.wake_registry.read().await.get(&key).cloned();
    let Some(deps) = deps else {
        return;
    };

    let mut payload = serde_json::json!({
        "task_number": task.task_number,
        "title": task.title,
        "action": action,
    });
    if let Some(approved_by) = &task.approved_by {
        payload["approved_by"] = serde_json::Value::from(approved_by.clone());
    }
    crate::wakes::emit_system_event(
        &deps,
        crate::wakes::SystemEvent::TaskApproved,
        &format!("task:{}", task.task_number),
        &payload,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /tasks` — list tasks with optional filters.
#[utoipa::path(
    get,
    path = "/tasks",
    params(TaskListQuery),
    responses(
        (status = 200, body = TaskListResponse),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn list_tasks(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<TaskListQuery>,
) -> Result<Json<TaskListResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let status = parse_status(query.status.as_deref())?;
    let priority = parse_priority(query.priority.as_deref())?;

    let tasks = store
        .list(crate::tasks::TaskListFilter {
            agent_id: query.agent_id,
            owner_agent_id: query.owner_agent_id,
            assigned_agent_id: query.assigned_agent_id,
            status,
            priority,
            created_by: query.created_by,
            limit: Some(query.limit.clamp(1, 500)),
        })
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to list tasks");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(TaskListResponse { tasks }))
}

/// `GET /tasks/{number}` — get a task by globally unique number.
#[utoipa::path(
    get,
    path = "/tasks/{number}",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn get_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let task = store
        .get_by_number(number)
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to get task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(TaskResponse { task }))
}

/// `POST /tasks` — create a task.
#[utoipa::path(
    post,
    path = "/tasks",
    request_body = CreateTaskRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 400, description = "Invalid request"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn create_task(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let status = crate::tasks::TaskStatus::PendingApproval;
    let priority =
        parse_priority(request.priority.as_deref())?.unwrap_or(crate::tasks::TaskPriority::Medium);

    let assigned = request
        .assigned_agent_id
        .unwrap_or_else(|| request.owner_agent_id.clone());

    let task = store
        .create_with_dependencies(
            crate::tasks::CreateTaskInput {
                owner_agent_id: request.owner_agent_id,
                assigned_agent_id: Some(assigned),
                title: request.title,
                description: request.description,
                status,
                priority,
                subtasks: request.subtasks,
                metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
                source_memory_id: request.source_memory_id,
                created_by: request.created_by.unwrap_or_else(|| "human".to_string()),
                worker_type: request.worker_type,
                project_id: request.project_id,
                repo_id: request.repo_id,
                worktree_mode: request.worktree_mode,
                worktree_id: request.worktree_id,
                required_skills: request.required_skills,
            },
            &dependency_edges(&request.depends_on),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to create task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    finish_task_mutation(&state, &task, "created", None).await;
    Ok(Json(TaskResponse { task }))
}

/// `PUT /tasks/{number}` — update a task.
#[utoipa::path(
    put,
    path = "/tasks/{number}",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = UpdateTaskRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn update_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let status = parse_status(request.status.as_deref())?;
    let priority = parse_priority(request.priority.as_deref())?;

    let edges = request.depends_on.as_deref().map(dependency_edges);

    let update = store
        .update_with_dependencies_and_status_override(
            number,
            crate::tasks::UpdateTaskInput {
                title: request.title,
                description: request.description,
                status,
                priority,
                assigned_agent_id: request.assigned_agent_id,
                subtasks: request.subtasks,
                metadata: request.metadata,
                worker_id: request.worker_id,
                clear_worker_id: false,
                approved_by: request.approved_by,
                complete_subtask: request.complete_subtask,
                worker_type: request.worker_type,
                project_id: request.project_id,
                repo_id: request.repo_id,
                worktree_mode: request.worktree_mode,
                worktree_id: request.worktree_id,
                required_skills: request.required_skills,
            },
            edges.as_deref(),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to update task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    Ok(Json(TaskResponse { task: update.task }))
}

/// `DELETE /tasks/{number}` — delete a task.
#[utoipa::path(
    delete,
    path = "/tasks/{number}",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    responses(
        (status = 200, body = TaskActionResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn delete_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
) -> Result<Json<TaskActionResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    // Fetch before delete so we can emit an event with the correct agent_id.
    let task = store
        .get_by_number(number)
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to get task for deletion");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let deleted = store.delete(number).await.map_err(|error| {
        tracing::warn!(%error, task_number = number, "failed to delete task");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !deleted {
        return Err(StatusCode::NOT_FOUND);
    }

    state
        .event_tx
        .send(super::state::ApiEvent::TaskUpdated {
            agent_id: task.effective_agent_id().to_string(),
            task_number: number,
            status: "deleted".to_string(),
            action: "deleted".to_string(),
        })
        .ok();

    Ok(Json(TaskActionResponse {
        success: true,
        message: format!("Task #{number} deleted"),
    }))
}

/// `POST /tasks/{number}/approve` — approve a task (move to ready).
#[utoipa::path(
    post,
    path = "/tasks/{number}/approve",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = ApproveRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn approve_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<ApproveRequest>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let update = store
        .update_with_status_transition(
            number,
            crate::tasks::UpdateTaskInput {
                status: Some(crate::tasks::TaskStatus::Ready),
                approved_by: request.approved_by,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to approve task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    // Auto-dismiss any pending task_approval notification for this task.
    if let Some(store) = state.notification_store.load().as_ref().clone()
        && let Err(error) = store
            .dismiss_by_entity("task_approval", "task", &number.to_string())
            .await
    {
        tracing::warn!(%error, task_number = number, "failed to auto-dismiss approval notification");
    }
    Ok(Json(TaskResponse { task: update.task }))
}

/// `POST /tasks/{number}/execute` — move a task to ready for execution.
/// Tasks already in `ready` or `in_progress` are returned as-is.
#[utoipa::path(
    post,
    path = "/tasks/{number}/execute",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = ApproveRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 409, description = "Task pending approval"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn execute_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<ApproveRequest>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let current = store
        .get_by_number(number)
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to get task for execution");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if matches!(
        current.status,
        crate::tasks::TaskStatus::Ready | crate::tasks::TaskStatus::InProgress
    ) {
        return Ok(Json(TaskResponse { task: current }));
    }

    // Reject pending_approval tasks — they must be approved first.
    if current.status == crate::tasks::TaskStatus::PendingApproval {
        return Err(StatusCode::CONFLICT);
    }

    let update = store
        .update_with_status_transition(
            number,
            crate::tasks::UpdateTaskInput {
                status: Some(crate::tasks::TaskStatus::Ready),
                approved_by: request.approved_by,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to execute task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    finish_task_mutation(
        &state,
        &update.task,
        "updated",
        Some(update.previous_status),
    )
    .await;
    Ok(Json(TaskResponse { task: update.task }))
}

/// `POST /tasks/{number}/assign` — reassign a task to a different agent.
#[utoipa::path(
    post,
    path = "/tasks/{number}/assign",
    params(
        ("number" = i64, Path, description = "Task number"),
    ),
    request_body = AssignRequest,
    responses(
        (status = 200, body = TaskResponse),
        (status = 404, description = "Task not found"),
        (status = 503, description = "Task store not initialized"),
    ),
    tag = "tasks",
)]
pub(super) async fn assign_task(
    State(state): State<Arc<ApiState>>,
    Path(number): Path<i64>,
    Json(request): Json<AssignRequest>,
) -> Result<Json<TaskResponse>, StatusCode> {
    let store = get_task_store(&state)?;

    let task = store
        .update(
            number,
            crate::tasks::UpdateTaskInput {
                assigned_agent_id: Some(request.assigned_agent_id),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_number = number, "failed to assign task");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    finish_task_mutation(&state, &task, "updated", None).await;
    Ok(Json(TaskResponse { task }))
}
