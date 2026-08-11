//! Deterministic, LLM-free render of the memory store for the channel's
//! knowledge-context slot.
//!
//! Replaces read-time LLM knowledge synthesis: the render is a pure function
//! of the store, so identical store state produces identical bytes — a memory
//! write is the only thing that changes the block. No LLM call exists
//! anywhere in this path. See `docs/design-docs/memory-first-knowledge-context.md`.

use crate::memory::{MemoryStore, MemoryType};
use crate::tasks::{TaskListFilter, TaskStatus, TaskStore};
use anyhow::{Context, Result};

/// Typed sections in render order. Facts first — the grounding section — then
/// the taxonomy order the `memory_save` tool teaches. Identity is excluded
/// (identity files own that layer), events live on the chronicle/working-memory
/// spine, and concrete todos live on the task board.
const SECTIONS: &[(MemoryType, &str, usize)] = &[
    (MemoryType::Fact, "Facts", 10),
    (MemoryType::Decision, "Decisions", 10),
    (MemoryType::Preference, "Preferences", 10),
    (MemoryType::Goal, "Goals", 10),
    (MemoryType::Observation, "Observations", 8),
    (MemoryType::Human, "People", 5),
];

/// Render the global memory-store view for the knowledge-context slot.
///
/// `max_words` caps the memory sections; the Active Tasks section always
/// renders regardless — task-awareness is a standing signal, and an empty
/// board is itself information. Shown-of-total counts report what was
/// actually rendered against what the store holds, so the model knows when
/// branch recall into the full store is worth it.
pub async fn render_memory_store(
    store: &MemoryStore,
    task_store: &TaskStore,
    agent_id: &str,
    max_words: usize,
) -> Result<String> {
    let mut output = String::from("## Memory Store\n\nScope: global\n");
    let mut word_budget = max_words;

    for (memory_type, label, budget) in SECTIONS {
        let total = store.count_by_type(*memory_type).await?;
        if total == 0 {
            continue;
        }

        let mut entries = store.get_by_type(*memory_type, *budget as i64).await?;
        // Stable ordering: importance desc, then updated_at desc, then id —
        // byte-identical between store writes even when SQLite's natural
        // ordering does not tie-break identically.
        entries.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.id.cmp(&b.id))
        });

        output.push('\n');
        if entries.len() as i64 == total {
            output.push_str(&format!("### {label}\n"));
        } else {
            output.push_str(&format!("### {label} — {} of {total}\n", entries.len()));
        }

        for memory in &entries {
            let line = format!(
                "- {} ({})\n",
                first_line(&memory.content),
                memory.updated_at.format("%Y-%m-%d")
            );
            word_budget = word_budget.saturating_sub(line.split_whitespace().count());
            if word_budget == 0 {
                break;
            }
            output.push_str(&line);
        }
    }

    output.push_str(&render_active_tasks(task_store, agent_id).await?);
    Ok(output)
}

/// Non-done tasks assigned to this agent, as a standing section.
async fn render_active_tasks(task_store: &TaskStore, agent_id: &str) -> Result<String> {
    let mut all_tasks = Vec::new();
    for status in &[
        TaskStatus::InProgress,
        TaskStatus::Ready,
        TaskStatus::Backlog,
        TaskStatus::PendingApproval,
    ] {
        let tasks = task_store
            .list(TaskListFilter {
                assigned_agent_id: Some(agent_id.to_string()),
                status: Some(*status),
                limit: Some(20),
                ..Default::default()
            })
            .await
            .with_context(|| format!("failed to list {status} tasks for memory render"))?;
        all_tasks.extend(tasks);
    }

    let mut output = String::from("\n### Active Tasks\n");
    if all_tasks.is_empty() {
        output.push_str("- No active tasks.\n");
        return Ok(output);
    }
    for task in &all_tasks {
        let subtask_progress = if task.subtasks.is_empty() {
            String::new()
        } else {
            let done = task.subtasks.iter().filter(|s| s.completed).count();
            format!(" [{}/{}]", done, task.subtasks.len())
        };
        output.push_str(&format!(
            "- #{} [{}] ({}) {}{}\n",
            task.task_number, task.status, task.priority, task.title, subtask_progress,
        ));
    }
    Ok(output)
}

/// First non-empty line of a memory's content, for a single-line bullet.
fn first_line(content: &str) -> &str {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_skips_blank_lines() {
        assert_eq!(first_line("\n\nFirst line\nSecond"), "First line");
        assert_eq!(first_line("single"), "single");
        assert_eq!(first_line("   \n  \n"), "");
    }
}
