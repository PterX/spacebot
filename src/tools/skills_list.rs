//! Skills list tool — name, description, and lifecycle state for every
//! loaded skill, joined with the usage table.
//!
//! Gives skill-writing processes (branches, cortex) the full picture the
//! prompt index compresses away: provenance, pin state, and usage counters.

use crate::config::RuntimeConfig;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Tool that lists loaded skills with usage and provenance detail.
#[derive(Debug, Clone)]
pub struct SkillsListTool {
    runtime_config: Arc<RuntimeConfig>,
}

impl SkillsListTool {
    pub fn new(runtime_config: Arc<RuntimeConfig>) -> Self {
        Self { runtime_config }
    }
}

/// Error type for skills_list tool.
#[derive(Debug, thiserror::Error)]
#[error("skills_list failed: {0}")]
pub struct SkillsListError(String);

/// Arguments for skills_list tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillsListArgs {}

/// One skill in the listing.
#[derive(Debug, Serialize)]
pub struct SkillsListEntry {
    pub name: String,
    pub description: String,
    /// builtin | instance | workspace
    pub source: String,
    /// Category derived from the directory path.
    pub category: String,
    /// user | agent | installed (absent when no usage row exists yet)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// active | stale | archived
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    pub read_count: i64,
    pub patch_count: i64,
}

/// Output from skills_list tool.
#[derive(Debug, Serialize)]
pub struct SkillsListOutput {
    pub count: usize,
    pub skills: Vec<SkillsListEntry>,
}

impl Tool for SkillsListTool {
    const NAME: &'static str = "skills_list";

    type Error = SkillsListError;
    type Args = SkillsListArgs;
    type Output = SkillsListOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/skills_list").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let skills = self.runtime_config.skills.load();
        let usage = self.runtime_config.skill_usage.load();

        let mut records = std::collections::HashMap::new();
        if let Some(store) = usage.as_ref() {
            match store.list().await {
                Ok(rows) => {
                    for row in rows {
                        records.insert(row.skill_name.clone(), row);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to load skill usage rows for listing");
                }
            }
        }

        let mut entries: Vec<SkillsListEntry> = skills
            .iter()
            .map(|skill| {
                let record = records.get(&skill.name.to_lowercase());
                SkillsListEntry {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    source: match skill.source {
                        crate::skills::SkillSource::Builtin => "builtin".to_string(),
                        crate::skills::SkillSource::Instance => "instance".to_string(),
                        crate::skills::SkillSource::Workspace => "workspace".to_string(),
                    },
                    category: skill.category.clone(),
                    created_by: record.map(|r| r.created_by.clone()),
                    state: record.map(|r| r.state.clone()),
                    pinned: record.is_some_and(|r| r.pinned),
                    read_count: record.map(|r| r.read_count).unwrap_or(0),
                    patch_count: record.map(|r| r.patch_count).unwrap_or(0),
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(SkillsListOutput {
            count: entries.len(),
            skills: entries,
        })
    }
}
