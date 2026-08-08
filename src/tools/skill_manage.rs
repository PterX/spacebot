//! Skill management tool — create, patch, edit, and delete skills.
//!
//! One action-dispatched tool for every skill mutation. The write origin is
//! set by the process constructing the tool server, never by the model, and
//! decides the rails: autonomous writers can't touch installed, pinned, or
//! instance-level skills, must have read a skill this session before
//! patching it, and their deletes archive instead of remove.

use crate::config::RuntimeConfig;
use crate::skills::{
    DESCRIPTION_BUDGET, SUPPORT_SUBDIRS, SkillSet, SkillSource, WriteOrigin, parse_skill_markdown,
};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Maximum SKILL.md size accepted by create/edit/patch.
const MAX_SKILL_BYTES: usize = 100 * 1024;
/// Maximum support file size accepted by write_file.
const MAX_SUPPORT_FILE_BYTES: usize = 1024 * 1024;
/// Maximum skill or category name length.
const MAX_NAME_LEN: usize = 64;

/// Shared per-session record of which skills have been read via `read_skill`.
///
/// Autonomous writers must have loaded the exact target in the current
/// session before they may patch it — a mechanical rail against patching
/// imagined content.
pub type SkillReadTracker = Arc<std::sync::Mutex<std::collections::HashSet<String>>>;

/// Create a fresh read tracker for a tool server session.
pub fn new_skill_read_tracker() -> SkillReadTracker {
    Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Tool for skill mutations, scoped by write origin.
#[derive(Clone)]
pub struct SkillManageTool {
    runtime_config: Arc<RuntimeConfig>,
    origin: WriteOrigin,
    read_tracker: Option<SkillReadTracker>,
    /// Conversation that spawned the writing process; recorded as provenance
    /// on agent-created skills.
    conversation_id: Option<String>,
}

impl std::fmt::Debug for SkillManageTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillManageTool")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl SkillManageTool {
    pub fn new(runtime_config: Arc<RuntimeConfig>, origin: WriteOrigin) -> Self {
        Self {
            runtime_config,
            origin,
            read_tracker: None,
            conversation_id: None,
        }
    }

    /// Attach the session read tracker shared with `read_skill`.
    pub fn with_read_tracker(mut self, tracker: SkillReadTracker) -> Self {
        self.read_tracker = Some(tracker);
        self
    }

    /// Attach the originating conversation for created-skill provenance.
    pub fn with_conversation_id(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    fn workspace_skills_dir(&self) -> PathBuf {
        self.runtime_config.workspace_dir.join("skills")
    }

    /// Reload the live SkillSet after a mutation — the deterministic path,
    /// no reliance on the file watcher.
    async fn reload(&self) {
        let instance_skills_dir = self.runtime_config.instance_dir.join("skills");
        let skills = SkillSet::load(&instance_skills_dir, &self.workspace_skills_dir()).await;
        self.runtime_config.reload_skills(skills);
    }

    fn usage_store(&self) -> Option<Arc<crate::skills::SkillUsageStore>> {
        (**self.runtime_config.skill_usage.load()).clone()
    }

    fn has_read(&self, name: &str) -> bool {
        self.read_tracker
            .as_ref()
            .map(|tracker| {
                tracker
                    .lock()
                    .expect("skill read tracker lock")
                    .contains(&name.to_lowercase())
            })
            .unwrap_or(false)
    }

    /// Rails that gate mutations of an existing skill. Returns a refusal
    /// message when the mutation is not allowed.
    async fn existing_skill_rails(
        &self,
        skill: &crate::skills::Skill,
        action: &str,
    ) -> Option<String> {
        let record = match self.usage_store() {
            Some(store) => store.get(&skill.name).await.ok().flatten(),
            None => None,
        };
        let pinned = record.as_ref().is_some_and(|r| r.pinned);
        let installed = record.as_ref().is_some_and(|r| r.created_by == "installed");

        match self.origin {
            WriteOrigin::Agent => {
                if skill.source != SkillSource::Workspace {
                    return Some(format!(
                        "'{}' is a {} skill; autonomous writes are limited to workspace skills",
                        skill.name,
                        source_label(&skill.source)
                    ));
                }
                if installed {
                    return Some(format!(
                        "'{}' was installed from a registry; autonomous writes can't modify installed skills. A reinstall would silently discard the change.",
                        skill.name
                    ));
                }
                if pinned {
                    return Some(format!(
                        "'{}' is pinned; pinned skills are read-only for autonomous writers",
                        skill.name
                    ));
                }
                if matches!(action, "patch" | "edit") && !self.has_read(&skill.name) {
                    return Some(format!(
                        "read '{}' with read_skill before modifying it — patches must be written against the current content",
                        skill.name
                    ));
                }
                None
            }
            WriteOrigin::User => {
                if skill.source == SkillSource::Builtin {
                    return Some(format!(
                        "'{}' is built into the binary and can't be modified",
                        skill.name
                    ));
                }
                if action == "delete" && pinned {
                    return Some(format!(
                        "'{}' is pinned; unpin it before deleting",
                        skill.name
                    ));
                }
                None
            }
        }
    }
}

fn source_label(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::Builtin => "builtin",
        SkillSource::Instance => "instance-level",
        SkillSource::Workspace => "workspace",
    }
}

/// Validate a skill or category name segment.
fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Validate SKILL.md content: frontmatter parses, description present and
/// within budget, body non-empty, size within cap.
fn validate_skill_content(content: &str) -> Result<(), String> {
    if content.len() > MAX_SKILL_BYTES {
        return Err(format!(
            "SKILL.md is {} bytes; the cap is {MAX_SKILL_BYTES}",
            content.len()
        ));
    }

    let (frontmatter, body) =
        parse_skill_markdown(content).map_err(|error| format!("invalid frontmatter: {error}"))?;

    let description = frontmatter.description.unwrap_or_default();
    if description.trim().is_empty() {
        return Err("frontmatter must include a description".to_string());
    }
    if description.chars().count() > DESCRIPTION_BUDGET {
        return Err(format!(
            "description is {} chars; the budget is {DESCRIPTION_BUDGET}. The description is loaded into every system prompt — compress it.",
            description.chars().count()
        ));
    }
    if body.trim().is_empty() {
        return Err("skill body is empty".to_string());
    }

    Ok(())
}

/// Resolve a support file path inside a skill directory, rejecting escapes.
///
/// The path must start with a support subdirectory, contain no `..` or
/// absolute components, and — for the portion that already exists on disk —
/// resolve inside the skill directory without crossing a symlink.
fn resolve_support_path(base_dir: &Path, file_path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(file_path);

    if relative.is_absolute() {
        return Err("file_path must be relative to the skill directory".to_string());
    }
    for component in relative.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => return Err("file_path may not contain '..' or special components".to_string()),
        }
    }

    let Some(first) = relative.components().next() else {
        return Err("file_path is empty".to_string());
    };
    let first = first.as_os_str().to_string_lossy();
    if !SUPPORT_SUBDIRS.contains(&first.as_ref()) {
        return Err(format!(
            "support files must live under one of: {}",
            SUPPORT_SUBDIRS.join(", ")
        ));
    }

    let target = base_dir.join(relative);

    // Walk existing ancestors and refuse symlinks; canonicalize the deepest
    // existing one and require it stays inside the skill directory.
    let canonical_base = base_dir
        .canonicalize()
        .map_err(|error| format!("skill directory unavailable: {error}"))?;
    let mut probe = target.clone();
    let deepest_existing = loop {
        if probe.exists() {
            break probe;
        }
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => return Err("file_path resolves outside the skill directory".to_string()),
        }
    };
    if deepest_existing
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("file_path crosses a symlink".to_string());
    }
    let canonical_existing = deepest_existing
        .canonicalize()
        .map_err(|error| format!("failed to resolve file_path: {error}"))?;
    if !canonical_existing.starts_with(&canonical_base) {
        return Err("file_path resolves outside the skill directory".to_string());
    }

    Ok(target)
}

/// Error type for skill_manage tool.
#[derive(Debug, thiserror::Error)]
#[error("skill_manage failed: {0}")]
pub struct SkillManageError(String);

/// Arguments for skill_manage tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillManageArgs {
    /// Action: create, patch, edit, delete, write_file, or remove_file.
    pub action: String,
    /// Skill name (lowercase; letters, digits, `.`, `_`, `-`).
    pub name: String,
    /// Full SKILL.md text for create/edit.
    pub content: Option<String>,
    /// Optional category directory for create (single path segment).
    pub category: Option<String>,
    /// Exact text to replace for patch.
    pub old_string: Option<String>,
    /// Replacement text for patch.
    pub new_string: Option<String>,
    /// Replace every occurrence for patch (default: false, must be unique).
    pub replace_all: Option<bool>,
    /// For delete after consolidation: the skill that absorbed this one.
    /// Must exist and differ from the target.
    pub absorbed_into: Option<String>,
    /// Path of a support file, relative to the skill directory, under
    /// references/, templates/, scripts/, or assets/.
    pub file_path: Option<String>,
    /// Content for write_file.
    pub file_content: Option<String>,
}

/// Output from skill_manage tool.
#[derive(Debug, Serialize)]
pub struct SkillManageOutput {
    pub success: bool,
    pub action: String,
    pub name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl SkillManageOutput {
    fn refused(action: &str, name: &str, message: String) -> Self {
        Self {
            success: false,
            action: action.to_string(),
            name: name.to_string(),
            message,
            path: None,
        }
    }

    fn done(action: &str, name: &str, message: String, path: Option<PathBuf>) -> Self {
        Self {
            success: true,
            action: action.to_string(),
            name: name.to_string(),
            message,
            path: path.map(|p| p.display().to_string()),
        }
    }
}

impl Tool for SkillManageTool {
    const NAME: &'static str = "skill_manage";

    type Error = SkillManageError;
    type Args = SkillManageArgs;
    type Output = SkillManageOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::prompts::text::get("tools/skill_manage").to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["create", "patch", "edit", "delete", "write_file", "remove_file"],
                        "description": "The mutation to perform."
                    },
                    "name": {
                        "type": "string",
                        "description": "Skill name. Lowercase letters, digits, '.', '_', '-'; must name the task class (e.g. 'discord-formatting'), never a single incident."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full SKILL.md text (YAML frontmatter with a description, then the markdown body). Required for create and edit."
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional category directory for create; single path segment."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Exact text to find for patch. Must match the current file content."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement text for patch."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every occurrence for patch. Without it, old_string must be unique."
                    },
                    "absorbed_into": {
                        "type": "string",
                        "description": "When deleting after merging content into another skill, the absorbing skill's name. Required for consolidation deletes; must exist."
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Support file path relative to the skill directory, under references/, templates/, scripts/, or assets/. For write_file and remove_file."
                    },
                    "file_content": {
                        "type": "string",
                        "description": "File content for write_file."
                    }
                },
                "required": ["action", "name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let action = args.action.trim().to_ascii_lowercase();
        let name = args.name.trim().to_lowercase();

        if !valid_name(&name) {
            return Ok(SkillManageOutput::refused(
                &action,
                &name,
                format!(
                    "invalid skill name '{name}': lowercase letters, digits, '.', '_', '-' only; \
                     must start with a letter or digit; max {MAX_NAME_LEN} chars"
                ),
            ));
        }

        let skills = self.runtime_config.skills.load();

        match action.as_str() {
            "create" => {
                let Some(content) = args.content.as_deref() else {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        "create requires content (the full SKILL.md text)".to_string(),
                    ));
                };

                if let Some(existing) = skills.get(&name) {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!(
                            "a {} skill named '{name}' already exists — patch or edit it instead of creating a duplicate",
                            source_label(&existing.source)
                        ),
                    ));
                }

                if let Err(message) = validate_skill_content(content) {
                    return Ok(SkillManageOutput::refused(&action, &name, message));
                }

                let mut dir = self.workspace_skills_dir();
                if let Some(category) = args
                    .category
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                {
                    let category = category.to_lowercase();
                    if !valid_name(&category) {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            format!(
                                "invalid category '{category}': single path segment, same charset as names"
                            ),
                        ));
                    }
                    dir = dir.join(category);
                }
                let dir = dir.join(&name);

                if dir.exists() {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!("directory already exists at {}", dir.display()),
                    ));
                }

                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|error| SkillManageError(error.to_string()))?;
                let file = dir.join("SKILL.md");
                tokio::fs::write(&file, content)
                    .await
                    .map_err(|error| SkillManageError(error.to_string()))?;

                if let Some(store) = self.usage_store()
                    && let Err(error) = store
                        .record_created(&name, self.origin, self.conversation_id.as_deref())
                        .await
                {
                    tracing::warn!(%error, skill = %name, "failed to record skill creation");
                }

                self.reload().await;
                tracing::info!(skill = %name, origin = ?self.origin, "skill created");

                Ok(SkillManageOutput::done(
                    &action,
                    &name,
                    format!("created '{name}'"),
                    Some(file),
                ))
            }
            "patch" | "edit" => {
                let Some(skill) = skills.get(&name) else {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!("skill '{name}' not found"),
                    ));
                };

                if let Some(refusal) = self.existing_skill_rails(skill, &action).await {
                    return Ok(SkillManageOutput::refused(&action, &name, refusal));
                }

                let new_content = if action == "edit" {
                    match args.content.as_deref() {
                        Some(content) => content.to_string(),
                        None => {
                            return Ok(SkillManageOutput::refused(
                                &action,
                                &name,
                                "edit requires content (the full SKILL.md text)".to_string(),
                            ));
                        }
                    }
                } else {
                    let (Some(old_string), Some(new_string)) =
                        (args.old_string.as_deref(), args.new_string.as_deref())
                    else {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            "patch requires old_string and new_string".to_string(),
                        ));
                    };
                    if old_string == new_string {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            "old_string and new_string are identical".to_string(),
                        ));
                    }

                    let current = tokio::fs::read_to_string(&skill.file_path)
                        .await
                        .map_err(|error| SkillManageError(error.to_string()))?;

                    let occurrences = current.matches(old_string).count();
                    if occurrences == 0 {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            "old_string not found in the skill — read_skill it and patch against the current content".to_string(),
                        ));
                    }
                    if occurrences > 1 && !args.replace_all.unwrap_or(false) {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            format!(
                                "old_string appears {occurrences} times; make it unique or set replace_all"
                            ),
                        ));
                    }

                    if args.replace_all.unwrap_or(false) {
                        current.replace(old_string, new_string)
                    } else {
                        current.replacen(old_string, new_string, 1)
                    }
                };

                if let Err(message) = validate_skill_content(&new_content) {
                    return Ok(SkillManageOutput::refused(&action, &name, message));
                }

                tokio::fs::write(&skill.file_path, &new_content)
                    .await
                    .map_err(|error| SkillManageError(error.to_string()))?;

                if let Some(store) = self.usage_store()
                    && let Err(error) = store.record_patch(&name).await
                {
                    tracing::warn!(%error, skill = %name, "failed to record skill patch");
                }

                let path = skill.file_path.clone();
                self.reload().await;
                tracing::info!(skill = %name, action = %action, origin = ?self.origin, "skill modified");

                Ok(SkillManageOutput::done(
                    &action,
                    &name,
                    format!("{action}ed '{name}'"),
                    Some(path),
                ))
            }
            "delete" => {
                let Some(skill) = skills.get(&name) else {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!("skill '{name}' not found"),
                    ));
                };

                if let Some(refusal) = self.existing_skill_rails(skill, &action).await {
                    return Ok(SkillManageOutput::refused(&action, &name, refusal));
                }

                if skill.source != SkillSource::Workspace {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!(
                            "'{name}' is a {} skill; only workspace skills can be deleted here",
                            source_label(&skill.source)
                        ),
                    ));
                }

                // A consolidation delete must name a real, different skill
                // that absorbed the content — fail closed on unverified
                // destruction.
                if let Some(absorbed_into) = args.absorbed_into.as_deref().map(str::trim) {
                    let absorbed_key = absorbed_into.to_lowercase();
                    if absorbed_key == name {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            "absorbed_into must name a different skill".to_string(),
                        ));
                    }
                    match skills.get(&absorbed_key) {
                        Some(absorbing) if absorbing.file_path.exists() => {}
                        _ => {
                            return Ok(SkillManageOutput::refused(
                                &action,
                                &name,
                                format!(
                                    "absorbed_into '{absorbed_key}' does not exist on disk — merge the content first, then delete"
                                ),
                            ));
                        }
                    }
                }

                // Guard against deleting anything that isn't a real skill
                // directory under the workspace skills root.
                let workspace_skills = self.workspace_skills_dir();
                if skill.base_dir == workspace_skills
                    || !skill.base_dir.starts_with(&workspace_skills)
                {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!(
                            "refusing to delete {} — not a skill directory under the workspace skills root",
                            skill.base_dir.display()
                        ),
                    ));
                }

                let (message, path) = match self.origin {
                    WriteOrigin::Agent => {
                        let archived = crate::skills::archive_skill_dir(
                            &workspace_skills,
                            &skill.base_dir,
                            &name,
                        )
                        .await
                        .map_err(|error| SkillManageError(error.to_string()))?;

                        if let Some(store) = self.usage_store()
                            && let Err(error) = store.set_archived(&name).await
                        {
                            tracing::warn!(%error, skill = %name, "failed to mark skill archived");
                        }

                        (format!("archived '{name}' (recoverable)"), Some(archived))
                    }
                    WriteOrigin::User => {
                        tokio::fs::remove_dir_all(&skill.base_dir)
                            .await
                            .map_err(|error| SkillManageError(error.to_string()))?;

                        if let Some(store) = self.usage_store()
                            && let Err(error) = store.remove(&name).await
                        {
                            tracing::warn!(%error, skill = %name, "failed to remove skill usage row");
                        }

                        (format!("deleted '{name}'"), None)
                    }
                };

                self.reload().await;
                tracing::info!(skill = %name, origin = ?self.origin, "skill deleted");

                Ok(SkillManageOutput {
                    success: true,
                    action,
                    name,
                    message,
                    path: path.map(|p| p.display().to_string()),
                })
            }
            "write_file" | "remove_file" => {
                let Some(skill) = skills.get(&name) else {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!("skill '{name}' not found"),
                    ));
                };

                if let Some(refusal) = self.existing_skill_rails(skill, &action).await {
                    return Ok(SkillManageOutput::refused(&action, &name, refusal));
                }

                let Some(file_path) = args
                    .file_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                else {
                    return Ok(SkillManageOutput::refused(
                        &action,
                        &name,
                        format!("{action} requires file_path"),
                    ));
                };

                let target = match resolve_support_path(&skill.base_dir, file_path) {
                    Ok(target) => target,
                    Err(message) => {
                        return Ok(SkillManageOutput::refused(&action, &name, message));
                    }
                };

                if action == "write_file" {
                    let Some(file_content) = args.file_content.as_deref() else {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            "write_file requires file_content".to_string(),
                        ));
                    };
                    if file_content.len() > MAX_SUPPORT_FILE_BYTES {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            format!(
                                "file is {} bytes; the cap is {MAX_SUPPORT_FILE_BYTES}",
                                file_content.len()
                            ),
                        ));
                    }

                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent)
                            .await
                            .map_err(|error| SkillManageError(error.to_string()))?;
                    }
                    tokio::fs::write(&target, file_content)
                        .await
                        .map_err(|error| SkillManageError(error.to_string()))?;
                } else {
                    if !target.is_file() {
                        return Ok(SkillManageOutput::refused(
                            &action,
                            &name,
                            format!("no file at {file_path}"),
                        ));
                    }
                    tokio::fs::remove_file(&target)
                        .await
                        .map_err(|error| SkillManageError(error.to_string()))?;
                }

                if let Some(store) = self.usage_store()
                    && let Err(error) = store.record_patch(&name).await
                {
                    tracing::warn!(%error, skill = %name, "failed to record skill file change");
                }

                self.reload().await;
                tracing::info!(skill = %name, action = %action, file = %file_path, "skill support file changed");

                Ok(SkillManageOutput::done(
                    &action,
                    &name,
                    format!("{action} '{file_path}' in '{name}'"),
                    Some(target),
                ))
            }
            other => Ok(SkillManageOutput::refused(
                other,
                &name,
                format!(
                    "invalid action '{other}'. Valid: create, patch, edit, delete, write_file, remove_file"
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_name_accepts_task_class_names() {
        assert!(valid_name("discord-formatting"));
        assert!(valid_name("deploy.checklist"));
        assert!(valid_name("3d-printing"));
        assert!(valid_name("a"));
    }

    #[test]
    fn valid_name_rejects_escapes_and_noise() {
        assert!(!valid_name(""));
        assert!(!valid_name(".."));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("-leading-dash"));
        assert!(!valid_name("Upper"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("path/segment"));
        assert!(!valid_name(&"x".repeat(MAX_NAME_LEN + 1)));
    }

    #[test]
    fn validate_skill_content_enforces_budget_and_body() {
        let good = "---\ndescription: How to deploy the app safely.\n---\n\n# Deploy\n\nSteps.";
        assert!(validate_skill_content(good).is_ok());

        let no_description = "---\nname: x\n---\n\nBody.";
        assert!(validate_skill_content(no_description).is_err());

        let long_description = format!(
            "---\ndescription: {}\n---\n\nBody.",
            "d".repeat(DESCRIPTION_BUDGET + 1)
        );
        assert!(validate_skill_content(&long_description).is_err());

        let empty_body = "---\ndescription: Fine.\n---\n\n   ";
        assert!(validate_skill_content(empty_body).is_err());

        let oversized = format!(
            "---\ndescription: Fine.\n---\n\n{}",
            "x".repeat(MAX_SKILL_BYTES)
        );
        assert!(validate_skill_content(&oversized).is_err());
    }

    #[test]
    fn resolve_support_path_rails() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        assert!(resolve_support_path(base, "references/notes.md").is_ok());
        assert!(resolve_support_path(base, "scripts/deep/run.sh").is_ok());

        assert!(resolve_support_path(base, "../outside.md").is_err());
        assert!(resolve_support_path(base, "references/../../outside.md").is_err());
        assert!(resolve_support_path(base, "/etc/passwd").is_err());
        assert!(resolve_support_path(base, "SKILL.md").is_err());
        assert!(resolve_support_path(base, "other/notes.md").is_err());
        assert!(resolve_support_path(base, "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_support_path_refuses_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let base = dir.path();

        std::os::unix::fs::symlink(outside.path(), base.join("references")).unwrap();
        assert!(resolve_support_path(base, "references/notes.md").is_err());
    }

    struct Harness {
        _dir: tempfile::TempDir,
        runtime_config: Arc<RuntimeConfig>,
        store: Arc<crate::skills::SkillUsageStore>,
    }

    async fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let instance_dir = dir.path().join("instance");

        let agent = crate::config::AgentConfig {
            id: "test-agent".to_string(),
            ..Default::default()
        };
        let defaults = crate::config::DefaultsConfig::default();
        let resolved = agent.resolve(&instance_dir, &defaults);
        std::fs::create_dir_all(resolved.workspace.join("skills")).unwrap();

        let runtime_config = Arc::new(RuntimeConfig::new(
            &instance_dir,
            &resolved,
            &defaults,
            crate::prompts::PromptEngine::new("en").unwrap(),
            crate::identity::Identity::default(),
            SkillSet::default(),
        ));

        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let store = Arc::new(crate::skills::SkillUsageStore::new(pool));
        runtime_config.set_skill_usage(store.clone());

        Harness {
            _dir: dir,
            runtime_config,
            store,
        }
    }

    fn tool(harness: &Harness, origin: WriteOrigin) -> SkillManageTool {
        SkillManageTool::new(harness.runtime_config.clone(), origin)
    }

    const SKILL_MD: &str =
        "---\ndescription: How to deploy the app safely.\n---\n\n# Deploy\n\nSteps.";

    async fn create_skill(harness: &Harness, name: &str) {
        let output = tool(harness, WriteOrigin::User)
            .call(SkillManageArgs {
                action: "create".to_string(),
                name: name.to_string(),
                content: Some(SKILL_MD.to_string()),
                category: None,
                old_string: None,
                new_string: None,
                replace_all: None,
                absorbed_into: None,
                file_path: None,
                file_content: None,
            })
            .await
            .unwrap();
        assert!(output.success, "create failed: {}", output.message);
    }

    fn args(action: &str, name: &str) -> SkillManageArgs {
        SkillManageArgs {
            action: action.to_string(),
            name: name.to_string(),
            content: None,
            category: None,
            old_string: None,
            new_string: None,
            replace_all: None,
            absorbed_into: None,
            file_path: None,
            file_content: None,
        }
    }

    #[tokio::test]
    async fn user_create_records_provenance_and_reloads() {
        let harness = harness().await;
        create_skill(&harness, "deploy").await;

        let record = harness.store.get("deploy").await.unwrap().unwrap();
        assert_eq!(record.created_by, "user");

        let skills = harness.runtime_config.skills.load();
        assert!(skills.get("deploy").is_some(), "live SkillSet reloaded");
    }

    #[tokio::test]
    async fn create_refuses_duplicate_name() {
        let harness = harness().await;
        create_skill(&harness, "deploy").await;

        let output = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                content: Some(SKILL_MD.to_string()),
                ..args("create", "deploy")
            })
            .await
            .unwrap();
        assert!(!output.success);
        assert!(output.message.contains("already exists"));
    }

    #[tokio::test]
    async fn agent_create_records_conversation_origin() {
        let harness = harness().await;

        let output = tool(&harness, WriteOrigin::Agent)
            .with_conversation_id("conv-42")
            .call(SkillManageArgs {
                content: Some(SKILL_MD.to_string()),
                ..args("create", "agent-made")
            })
            .await
            .unwrap();
        assert!(output.success, "{}", output.message);

        let record = harness.store.get("agent-made").await.unwrap().unwrap();
        assert_eq!(record.created_by, "agent");
        assert_eq!(record.origin_conversation_id.as_deref(), Some("conv-42"));
    }

    #[tokio::test]
    async fn agent_patch_requires_prior_read() {
        let harness = harness().await;
        create_skill(&harness, "deploy").await;

        let tracker = new_skill_read_tracker();
        let agent_tool = tool(&harness, WriteOrigin::Agent).with_read_tracker(tracker.clone());

        let denied = agent_tool
            .call(SkillManageArgs {
                old_string: Some("Steps.".to_string()),
                new_string: Some("Steps, carefully.".to_string()),
                ..args("patch", "deploy")
            })
            .await
            .unwrap();
        assert!(!denied.success);
        assert!(denied.message.contains("read_skill"));

        tracker.lock().unwrap().insert("deploy".to_string());
        let allowed = agent_tool
            .call(SkillManageArgs {
                old_string: Some("Steps.".to_string()),
                new_string: Some("Steps, carefully.".to_string()),
                ..args("patch", "deploy")
            })
            .await
            .unwrap();
        assert!(allowed.success, "{}", allowed.message);

        let record = harness.store.get("deploy").await.unwrap().unwrap();
        assert_eq!(record.patch_count, 1);
    }

    #[tokio::test]
    async fn agent_cannot_touch_installed_or_pinned() {
        let harness = harness().await;
        create_skill(&harness, "installed-skill").await;
        create_skill(&harness, "pinned-skill").await;
        harness
            .store
            .record_installed(&["installed-skill".to_string()])
            .await
            .unwrap();
        harness
            .store
            .set_pinned("pinned-skill", true)
            .await
            .unwrap();

        let tracker = new_skill_read_tracker();
        tracker
            .lock()
            .unwrap()
            .insert("installed-skill".to_string());
        tracker.lock().unwrap().insert("pinned-skill".to_string());
        let agent_tool = tool(&harness, WriteOrigin::Agent).with_read_tracker(tracker);

        let installed = agent_tool
            .call(SkillManageArgs {
                old_string: Some("Steps.".to_string()),
                new_string: Some("Other.".to_string()),
                ..args("patch", "installed-skill")
            })
            .await
            .unwrap();
        assert!(!installed.success);
        assert!(installed.message.contains("installed"));

        let pinned = agent_tool
            .call(SkillManageArgs {
                old_string: Some("Steps.".to_string()),
                new_string: Some("Other.".to_string()),
                ..args("patch", "pinned-skill")
            })
            .await
            .unwrap();
        assert!(!pinned.success);
        assert!(pinned.message.contains("pinned"));
    }

    #[tokio::test]
    async fn agent_delete_archives_and_user_delete_removes() {
        let harness = harness().await;
        create_skill(&harness, "ephemeral").await;
        create_skill(&harness, "doomed").await;

        let archived = tool(&harness, WriteOrigin::Agent)
            .call(args("delete", "ephemeral"))
            .await
            .unwrap();
        assert!(archived.success, "{}", archived.message);
        let archive_path = archived.path.as_deref().unwrap();
        assert!(archive_path.contains(".archive"));
        assert!(std::path::Path::new(archive_path).join("SKILL.md").exists());
        let record = harness.store.get("ephemeral").await.unwrap().unwrap();
        assert_eq!(record.state, "archived");
        assert!(
            harness
                .runtime_config
                .skills
                .load()
                .get("ephemeral")
                .is_none()
        );

        let deleted = tool(&harness, WriteOrigin::User)
            .call(args("delete", "doomed"))
            .await
            .unwrap();
        assert!(deleted.success, "{}", deleted.message);
        assert!(harness.store.get("doomed").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn pinned_blocks_user_delete() {
        let harness = harness().await;
        create_skill(&harness, "keeper").await;
        harness.store.set_pinned("keeper", true).await.unwrap();

        let output = tool(&harness, WriteOrigin::User)
            .call(args("delete", "keeper"))
            .await
            .unwrap();
        assert!(!output.success);
        assert!(output.message.contains("pinned"));
    }

    #[tokio::test]
    async fn absorbed_into_fails_closed() {
        let harness = harness().await;
        create_skill(&harness, "merged-away").await;

        let missing = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                absorbed_into: Some("nonexistent".to_string()),
                ..args("delete", "merged-away")
            })
            .await
            .unwrap();
        assert!(!missing.success);
        assert!(missing.message.contains("does not exist"));

        let self_absorb = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                absorbed_into: Some("merged-away".to_string()),
                ..args("delete", "merged-away")
            })
            .await
            .unwrap();
        assert!(!self_absorb.success);

        create_skill(&harness, "absorber").await;
        let valid = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                absorbed_into: Some("absorber".to_string()),
                ..args("delete", "merged-away")
            })
            .await
            .unwrap();
        assert!(valid.success, "{}", valid.message);
    }

    #[tokio::test]
    async fn write_file_lands_in_support_dir_and_shows_in_linked_files() {
        let harness = harness().await;
        create_skill(&harness, "deploy").await;

        let output = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                file_path: Some("references/rollback.md".to_string()),
                file_content: Some("# Rollback\n\nSteps.".to_string()),
                ..args("write_file", "deploy")
            })
            .await
            .unwrap();
        assert!(output.success, "{}", output.message);

        let skills = harness.runtime_config.skills.load();
        let skill = skills.get("deploy").unwrap();
        assert_eq!(
            skill.linked_files,
            vec!["references/rollback.md".to_string()]
        );

        let escape = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                file_path: Some("../outside.md".to_string()),
                file_content: Some("nope".to_string()),
                ..args("write_file", "deploy")
            })
            .await
            .unwrap();
        assert!(!escape.success);
    }

    #[tokio::test]
    async fn create_with_category_is_discovered() {
        let harness = harness().await;

        let output = tool(&harness, WriteOrigin::User)
            .call(SkillManageArgs {
                content: Some(SKILL_MD.to_string()),
                category: Some("ops".to_string()),
                ..args("create", "categorized")
            })
            .await
            .unwrap();
        assert!(output.success, "{}", output.message);
        assert!(output.path.as_deref().unwrap().contains("/ops/"));

        let skills = harness.runtime_config.skills.load();
        assert!(skills.get("categorized").is_some());
    }
}
