//! User-defined goals: persistent objectives that orient agent work.
//!
//! Goals are the "why" behind task work — high-level, always visible, and
//! closed by the user rather than auto-completed. See
//! `docs/design-docs/goals.md` for the full design.

pub mod store;

pub use store::{
    CreateGoalInput, Goal, GoalListFilter, GoalStatus, GoalStore, GoalTaskCounts, UpdateGoalInput,
    can_transition,
};

use crate::error::Result;

/// Maximum goals rendered into the channel system prompt. Keeps the short
/// format within its ~200 token budget.
const ACTIVE_GOALS_PROMPT_LIMIT: i64 = 10;

/// Render the compact active-goals list for channel system prompts.
///
/// Format per goal: `- [HIGH] title (due: date) — notes-first-line`, falling
/// back to a linked-task summary when no notes are set. Returns an empty
/// string when there are no active goals so callers can skip injection.
pub async fn render_active_goals(store: &GoalStore) -> Result<String> {
    let goals = store
        .list(GoalListFilter {
            status: Some(GoalStatus::Active),
            limit: Some(ACTIVE_GOALS_PROMPT_LIMIT),
        })
        .await?;

    if goals.is_empty() {
        return Ok(String::new());
    }

    let mut lines = vec!["## Active Goals".to_string()];
    for goal in goals {
        let mut line = format!(
            "- [{}] {}",
            goal.priority.as_str().to_uppercase(),
            goal.title
        );
        if let Some(due_date) = &goal.due_date {
            line.push_str(&format!(" (due: {due_date})"));
        }

        let summary = match goal.notes.as_deref().and_then(first_non_empty_line) {
            Some(notes_line) => notes_line.to_string(),
            None => store.linked_task_counts(&goal.id).await?.summary(),
        };
        line.push_str(&format!(" — {summary}"));
        lines.push(line);
    }

    Ok(lines.join("\n"))
}

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Render the extended active-goals block for the autonomy channel briefing.
///
/// Unlike the short channel format, this includes each goal's full
/// description, full progress notes, and linked-task counts — the autonomy
/// channel reasons about which work serves which goal, so it gets the whole
/// picture. Returns an empty string when there are no active goals.
pub async fn render_active_goals_extended(store: &GoalStore) -> Result<String> {
    let goals = store
        .list(GoalListFilter {
            status: Some(GoalStatus::Active),
            limit: Some(ACTIVE_GOALS_PROMPT_LIMIT),
        })
        .await?;

    if goals.is_empty() {
        return Ok(String::new());
    }

    let mut sections = Vec::with_capacity(goals.len());
    for goal in goals {
        let mut lines = vec![format!(
            "### [{}] {}{}",
            goal.priority.as_str().to_uppercase(),
            goal.title,
            goal.due_date
                .as_deref()
                .map(|due_date| format!(" (due: {due_date})"))
                .unwrap_or_default()
        )];
        if let Some(description) = goal.description.as_deref().map(str::trim)
            && !description.is_empty()
        {
            lines.push(description.to_string());
        }
        if let Some(notes) = goal.notes.as_deref().map(str::trim)
            && !notes.is_empty()
        {
            lines.push(format!("Notes: {notes}"));
        }
        let counts = store.linked_task_counts(&goal.id).await?;
        lines.push(format!(
            "Tasks: {} ({} pending approval, {} ready, {} in progress, {} done, {} failed)",
            counts.total(),
            counts.pending_approval,
            counts.ready,
            counts.in_progress,
            counts.done,
            counts.failed
        ));
        sections.push(lines.join("\n"));
    }

    Ok(sections.join("\n\n"))
}
