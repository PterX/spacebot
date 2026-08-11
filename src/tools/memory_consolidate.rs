//! Memory consolidation tool for branches.
//!
//! Executes one atomic consolidation batch for a partition (memory type):
//! each op merges an absorbed memory into a survivor with the branch's
//! composed content, re-embeds the survivor, and deletes the absorbed memory.
//! Batches are serialized per partition and clear the partition's
//! write-time consolidation debt on success.

use crate::memory::MemorySearch;
use crate::memory::consolidation::{ConsolidateOp, ConsolidationReport};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Tool for consolidating memory near-duplicates.
#[derive(Debug, Clone)]
pub struct MemoryConsolidateTool {
    memory_search: Arc<MemorySearch>,
}

impl MemoryConsolidateTool {
    /// Create a new memory consolidate tool.
    pub fn new(memory_search: Arc<MemorySearch>) -> Self {
        Self { memory_search }
    }
}

/// Error type for memory consolidate tool.
#[derive(Debug, thiserror::Error)]
#[error("Memory consolidation failed: {0}")]
pub struct MemoryConsolidateError(String);

impl From<anyhow::Error> for MemoryConsolidateError {
    fn from(e: anyhow::Error) -> Self {
        MemoryConsolidateError(format!("{e}"))
    }
}

/// One merge operation of a consolidation batch.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConsolidateOpArgs {
    /// The ID of the memory that stays.
    pub survivor_id: String,
    /// The ID of the memory to merge into the survivor and delete.
    pub absorbed_id: String,
    /// The combined, de-duplicated content for the survivor.
    pub merged_content: String,
}

/// Arguments for memory consolidate tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryConsolidateArgs {
    /// The partition (memory type label, e.g. "fact") being consolidated.
    pub partition: String,
    /// The merge operations, applied in order.
    pub batch: Vec<ConsolidateOpArgs>,
}

/// Output from memory consolidate tool.
#[derive(Debug, Serialize)]
pub struct MemoryConsolidateOutput {
    /// The partition that was consolidated.
    pub partition: String,
    /// Merges applied in the batch.
    pub consolidated: usize,
    /// Memories remaining in the partition after the batch.
    pub remaining_count: i64,
    /// Whether the batch succeeded.
    pub success: bool,
    /// Message about the result.
    pub message: String,
}

impl Tool for MemoryConsolidateTool {
    const NAME: &'static str = "memory_consolidate";

    type Error = MemoryConsolidateError;
    type Args = MemoryConsolidateArgs;
    type Output = MemoryConsolidateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/memory_consolidate").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "partition": {
                        "type": "string",
                        "description": "Memory type label of the partition to consolidate (fact, preference, decision, identity, event, observation, goal, todo, human)."
                    },
                    "batch": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "survivor_id": {
                                    "type": "string",
                                    "description": "ID of the memory that stays."
                                },
                                "absorbed_id": {
                                    "type": "string",
                                    "description": "ID of the memory merged into the survivor and deleted."
                                },
                                "merged_content": {
                                    "type": "string",
                                    "description": "Combined, de-duplicated content for the survivor."
                                }
                            },
                            "required": ["survivor_id", "absorbed_id", "merged_content"]
                        },
                        "description": "Merge operations, applied in order."
                    }
                },
                "required": ["partition", "batch"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        let Some(memory_type) = crate::memory::MemoryType::from_label(&args.partition) else {
            return Err(MemoryConsolidateError(format!(
                "unknown partition {:?} — expected a memory type label such as \"fact\"",
                args.partition
            )));
        };

        if args.batch.is_empty() {
            let remaining_count = self
                .memory_search
                .store()
                .count_by_type(memory_type)
                .await
                .map_err(|e| MemoryConsolidateError(format!("{e}")))?;
            return Ok(MemoryConsolidateOutput {
                partition: args.partition.clone(),
                consolidated: 0,
                remaining_count,
                success: true,
                message: "Empty batch — nothing to consolidate".to_string(),
            });
        }

        let ops: Vec<ConsolidateOp> = args
            .batch
            .into_iter()
            .map(|op| ConsolidateOp {
                survivor_id: op.survivor_id,
                absorbed_id: op.absorbed_id,
                merged_content: op.merged_content,
            })
            .collect();

        let report: ConsolidationReport = crate::memory::consolidation::execute_batch(
            self.memory_search.consolidation(),
            self.memory_search.store(),
            self.memory_search.embedding_table(),
            self.memory_search.embedding_model_arc(),
            &args.partition,
            &ops,
        )
        .await?;

        Ok(MemoryConsolidateOutput {
            partition: report.partition.clone(),
            consolidated: report.consolidated,
            remaining_count: report.remaining_count,
            success: true,
            message: format!(
                "Consolidated {} memory(ies) in partition {}; {} remain",
                report.consolidated, report.partition, report.remaining_count
            ),
        })
    }
}
