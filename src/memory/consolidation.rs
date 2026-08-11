//! Write-time consolidation: advisory near-duplicate detection and
//! per-partition debt, executed atomically by the branch.
//!
//! The save always lands — consolidation is advisory and never blocks a
//! write. After a memory is persisted, `check_save` reports two signals on
//! the output the branch sees:
//!
//! - **near-duplicates** — existing memories closely matching the new one
//!   (vector similarity above `consolidation_near_duplicate_threshold`)
//! - **per-partition over-cap debt** — the partition (memory type) has grown
//!   past `consolidation_partition_cap`, so the render's per-type budgets are
//!   showing a thinning slice of a bloated store
//!
//! The branch responds with one atomic `memory_consolidate` batch per
//! partition (`execute_batch`): merge each pair into a single survivor with
//! the branch's composed content, re-embed the survivor, delete the absorbed
//! memory, and clear the partition's debt. Batches are serialized per
//! partition — one consolidator at a time.

use crate::memory::{EmbeddingModel, EmbeddingTable, MemoryStore, MemoryType};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Shared per-agent consolidation state: per-partition debt counters and
/// per-partition serialization locks. Lives inside `MemorySearch` so every
/// tool clone shares one instance.
#[derive(Debug, Default)]
pub struct ConsolidationState {
    /// Unresolved consolidation batches per partition (memory type label).
    debt: Mutex<HashMap<String, u64>>,
    /// One lock per partition; held for the duration of a consolidation
    /// batch so concurrent batches never interleave.
    partition_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ConsolidationState {
    /// Record one more over-cap save for a partition. Returns the new debt.
    pub fn record_debt(&self, partition: &str) -> u64 {
        let mut debt = self.debt.lock().expect("consolidation debt poisoned");
        let count = debt.entry(partition.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    /// Current unresolved debt for a partition.
    pub fn debt(&self, partition: &str) -> u64 {
        self.debt
            .lock()
            .expect("consolidation debt poisoned")
            .get(partition)
            .copied()
            .unwrap_or(0)
    }

    /// Clear the debt for a partition after a successful consolidation batch.
    pub fn clear_debt(&self, partition: &str) {
        self.debt
            .lock()
            .expect("consolidation debt poisoned")
            .remove(partition);
    }

    /// The per-partition serialization lock (created on first use).
    fn partition_lock(&self, partition: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .partition_locks
            .lock()
            .expect("consolidation locks poisoned");
        locks
            .entry(partition.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

/// A closely matching existing memory, surfaced on a save.
#[derive(Debug, Clone, Serialize)]
pub struct NearDuplicate {
    pub memory_id: String,
    /// Similarity (1.0 - cosine distance) against the newly saved memory.
    pub similarity: f32,
    /// First non-empty line of the existing memory's content.
    pub content: String,
}

/// Advisory returned from a save, rendered on the tool output for the branch.
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationAdvice {
    /// Partition (memory type label) the advice applies to.
    pub partition: String,
    /// Total memories in the partition after this save.
    pub partition_count: i64,
    /// Per-partition cap from config.
    pub partition_cap: usize,
    /// True when the partition is over its cap.
    pub over_cap: bool,
    /// Unresolved consolidation batches queued for this partition.
    pub debt: u64,
    /// Closely matching existing memories.
    pub near_duplicates: Vec<NearDuplicate>,
}

/// One merge operation of a consolidation batch: absorb `absorbed_id` into
/// `survivor_id`, replacing the survivor's content with the branch's composed
/// `merged_content`.
#[derive(Debug, Clone)]
pub struct ConsolidateOp {
    pub survivor_id: String,
    pub absorbed_id: String,
    pub merged_content: String,
}

/// Outcome of a consolidation batch.
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationReport {
    pub partition: String,
    /// Merges applied in the batch.
    pub consolidated: usize,
    /// Memories remaining in the partition after the batch.
    pub remaining_count: i64,
}

/// Run the post-save consolidation check. Advisory: returns `None` when
/// nothing is actionable (no near-duplicates, partition under cap).
pub async fn check_save(
    state: &ConsolidationState,
    store: &MemoryStore,
    embedding_table: &EmbeddingTable,
    memory_id: &str,
    memory_type: MemoryType,
    cap: usize,
    threshold: f32,
) -> Result<Option<ConsolidationAdvice>> {
    let partition = memory_type.to_string();
    let partition_count = store.count_by_type(memory_type).await?;
    let over_cap = partition_count > cap as i64;
    let debt = if over_cap {
        state.record_debt(&partition)
    } else {
        state.debt(&partition)
    };

    let mut near_duplicates = Vec::new();
    for (candidate_id, similarity) in embedding_table
        .find_similar(memory_id, threshold, 5)
        .await?
    {
        let preview = match store.load(&candidate_id).await {
            Ok(Some(memory)) => first_line(&memory.content),
            _ => String::new(),
        };
        near_duplicates.push(NearDuplicate {
            memory_id: candidate_id,
            similarity,
            content: preview,
        });
    }

    if !over_cap && near_duplicates.is_empty() {
        return Ok(None);
    }

    Ok(Some(ConsolidationAdvice {
        partition,
        partition_count,
        partition_cap: cap,
        over_cap,
        debt,
        near_duplicates,
    }))
}

/// Execute a consolidation batch for one partition. Serialized per partition:
/// the partition lock is held for the whole batch. Each merge is atomic; on
/// any failure the remaining merges are aborted and the error propagates.
pub async fn execute_batch(
    state: &ConsolidationState,
    store: &MemoryStore,
    embedding_table: &EmbeddingTable,
    embedding_model: &Arc<EmbeddingModel>,
    partition: &str,
    batch: &[ConsolidateOp],
) -> Result<ConsolidationReport> {
    let partition_lock = state.partition_lock(partition);
    let _partition_guard = partition_lock.lock().await;

    for op in batch {
        let survivor = store
            .load(&op.survivor_id)
            .await?
            .with_context(|| format!("consolidation survivor {} missing", op.survivor_id))?;
        let absorbed = store
            .load(&op.absorbed_id)
            .await?
            .with_context(|| format!("consolidation target {} missing", op.absorbed_id))?;

        let merged_content = op.merged_content.trim().to_string();
        if merged_content.is_empty() {
            anyhow::bail!(
                "consolidation merged_content is empty for {}",
                op.survivor_id
            );
        }

        let mut updated_survivor = survivor.clone();
        updated_survivor.content = merged_content.clone();
        updated_survivor.importance = survivor.importance.max(absorbed.importance);
        updated_survivor.updated_at = chrono::Utc::now();

        // Supersede-with-provenance (1.7): record the chronicle checkpoint
        // whose span the absorbed memory came from, so the merged memory
        // carries where the superseded claim originated.
        if updated_survivor.supersedes_checkpoint_id.is_none()
            && let Some(channel_id) = &absorbed.channel_id
        {
            let chronicle =
                crate::conversation::chronicle::ChronicleStore::new(store.pool().clone());
            if let Ok(Some(checkpoint)) = chronicle
                .covering_checkpoint(channel_id, absorbed.created_at)
                .await
            {
                updated_survivor.supersedes_checkpoint_id = Some(checkpoint.id);
            }
        }

        store
            .merge_memories_atomic(&updated_survivor, &absorbed)
            .await
            .with_context(|| {
                format!("failed to merge {} into {}", op.absorbed_id, op.survivor_id)
            })?;

        // The survivor's content changed — its embedding is stale. Re-embed
        // before deleting the absorbed memory so a failed re-embed aborts the
        // batch while both rows still exist.
        let embedding = embedding_model
            .embed_one(&merged_content)
            .await
            .with_context(|| format!("failed to re-embed survivor {}", op.survivor_id))?;
        embedding_table
            .store(&updated_survivor.id, &merged_content, &embedding)
            .await
            .with_context(|| format!("failed to store re-embedding for {}", op.survivor_id))?;

        if let Err(error) = store.delete(&absorbed.id).await {
            // The merge is durable in the survivor; an orphaned embedding row
            // is invisible to recall (joins resolve through SQLite).
            tracing::warn!(
                survivor_id = %op.survivor_id,
                absorbed_id = %op.absorbed_id,
                %error,
                "failed to delete absorbed memory after merge"
            );
        }
        if let Err(error) = embedding_table.delete(&absorbed.id).await {
            tracing::warn!(
                absorbed_id = %op.absorbed_id,
                %error,
                "failed to delete absorbed memory embedding"
            );
        }
    }

    state.clear_debt(partition);
    let remaining = match MemoryType::from_label(partition) {
        Some(t) => store.count_by_type(t).await?,
        None => 0,
    };

    Ok(ConsolidationReport {
        partition: partition.to_string(),
        consolidated: batch.len(),
        remaining_count: remaining,
    })
}

/// First non-empty line, capped for previews.
fn first_line(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(140)
        .collect()
}
