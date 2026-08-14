//! Task tracking data model and storage.

pub mod comments;
pub mod migration;
pub mod revisions;
pub mod store;

pub use comments::{
    CreateTaskCommentInput, MAX_COMMENT_BODY_BYTES, MAX_COMMENT_PAGE, MIN_COMMENT_BODY_CHARS,
    TaskComment, normalize_comment_body,
};
pub use revisions::{
    MAX_EDIT_SUMMARY_CHARS, MAX_REVISION_PAGE, TaskAuthorKind, TaskFieldChange,
    TaskMutationContext, TaskMutationSource, TaskRevision, TaskRevisionDependency,
    TaskRevisionDiff, TaskRevisionSnapshot, TaskRevisionSummary,
};
pub use store::{
    CreateTaskInput, ExecutionDefaults, ExecutionPlan, Patch, Task, TaskDependencyEdge,
    TaskDependencyKind, TaskListFilter, TaskPriority, TaskStatus, TaskStore, TaskSubtask,
    TaskUpdateResult, TaskWorkerType, TaskWorktreeMode, UpdateTaskInput, WorkerTaskUpdateResult,
};
