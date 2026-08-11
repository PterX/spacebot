//! Skill loading and prompt injection.
//!
//! Skills are directories containing a SKILL.md file with YAML frontmatter
//! (name, description, metadata) and a markdown body with instructions.
//! Compatible with OpenClaw's skill format and skills.sh registry.
//!
//! Skills are loaded from three sources (later wins on name conflicts):
//! 1. Built-in: embedded in the binary at compile time
//! 2. Instance-level: `{instance_dir}/skills/`
//! 3. Agent workspace: `{workspace}/skills/`
//!
//! The channel sees a summary of available skills and is instructed to
//! delegate skill work to workers. Workers receive the full skill content
//! in their system prompt.

pub mod builtin;
mod installer;
mod usage;

pub use installer::{install_from_file, install_from_github};
pub use usage::{SkillUsageRecord, SkillUsageStore, WriteOrigin};

use anyhow::Context as _;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Support subdirectories inside a skill directory. Files under these are
/// excluded from skill discovery and surfaced as `linked_files` by read_skill,
/// so a skill body can point at deeper material without inflating the index.
pub const SUPPORT_SUBDIRS: &[&str] = &["references", "templates", "scripts", "assets"];

/// Character budget for skill descriptions in prompt indexes. Enforced on
/// create/edit paths; pre-existing skills render truncated instead of
/// failing to load.
pub const DESCRIPTION_BUDGET: usize = 80;

/// Typed SKILL.md frontmatter. Unknown fields are ignored so skills from
/// other ecosystems (carrying `version`, `author`, `license`, ...) load fine.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name; falls back to the directory name when absent.
    pub name: Option<String>,
    /// Short description shown in prompt indexes.
    pub description: Option<String>,
    /// Host platforms this skill applies to. Absent means all platforms.
    pub platforms: Option<Vec<Platform>>,
    /// Free-form tags.
    pub tags: Option<Vec<String>>,
    /// Names of related skills, surfaced by read_skill as navigation hints.
    pub related_skills: Option<Vec<String>>,
    /// GitHub `owner/repo` this skill was installed from, if any.
    pub source_repo: Option<String>,
}

/// A host platform a skill can be gated to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Macos,
    Windows,
    /// Any value we don't recognize. Never matches the host, so a skill
    /// gated to an unknown platform is skipped rather than failing to parse.
    Other,
}

impl<'de> Deserialize<'de> for Platform {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.trim().to_ascii_lowercase().as_str() {
            "linux" => Platform::Linux,
            "macos" | "darwin" | "mac" => Platform::Macos,
            "windows" | "win" => Platform::Windows,
            _ => Platform::Other,
        })
    }
}

impl Platform {
    /// The platform of the host this process is running on.
    pub fn host() -> Platform {
        match std::env::consts::OS {
            "linux" => Platform::Linux,
            "macos" => Platform::Macos,
            "windows" => Platform::Windows,
            _ => Platform::Other,
        }
    }

    /// Whether a skill's platform list matches the host.
    ///
    /// `Other` never matches — a skill gated to unrecognized platforms stays
    /// off every host, and an unrecognized host OS fails all gates rather
    /// than passing them.
    pub fn list_matches_host(platforms: &[Platform]) -> bool {
        let host = Platform::host();
        host != Platform::Other && platforms.contains(&host)
    }
}

/// A loaded skill definition.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name from frontmatter.
    pub name: String,
    /// Short description from frontmatter.
    pub description: String,
    /// Absolute path to the SKILL.md file.
    pub file_path: PathBuf,
    /// Directory containing the skill.
    pub base_dir: PathBuf,
    /// Full rendered content (frontmatter stripped, `{baseDir}` resolved).
    pub content: String,
    /// Where this skill was loaded from.
    pub source: SkillSource,
    /// GitHub `owner/repo` that this skill was installed from, if any.
    pub source_repo: Option<String>,
    /// Free-form tags from frontmatter.
    pub tags: Vec<String>,
    /// Names of related skills from frontmatter, advisory.
    pub related_skills: Vec<String>,
    /// Files under the skill's support subdirectories, relative to `base_dir`.
    pub linked_files: Vec<String>,
    /// Category derived from the directory path. Top-level skills get
    /// `"general"`; categorized skills get their parent directory name.
    pub category: String,
}

/// Where a skill was loaded from, used for precedence tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// Compiled into the binary.
    Builtin,
    /// Instance-level `{instance_dir}/skills/`.
    Instance,
    /// Agent workspace `{workspace}/skills/`.
    Workspace,
}

/// All skills loaded for an agent.
#[derive(Debug, Clone, Default)]
pub struct SkillSet {
    /// Skills keyed by name (lowercase). Later sources override earlier ones.
    skills: HashMap<String, Skill>,
    /// Category descriptions loaded from `index.md` files in category
    /// directories, keyed by category name.
    category_descriptions: HashMap<String, String>,
}

impl SkillSet {
    /// Load skills from builtin, instance, and workspace sources.
    ///
    /// Precedence: Builtin < Instance < Workspace (later wins on name conflicts).
    pub async fn load(instance_skills_dir: &Path, workspace_skills_dir: &Path) -> Self {
        let mut set = Self::default();

        // Builtin skills (lowest precedence)
        for skill in builtin::load() {
            set.skills.insert(skill.name.to_lowercase(), skill);
        }

        // Instance skills
        if instance_skills_dir.is_dir()
            && let Ok((skills, descriptions)) =
                load_skills_from_dir(instance_skills_dir, SkillSource::Instance).await
        {
            for skill in skills {
                set.skills.insert(skill.name.to_lowercase(), skill);
            }
            for (cat, desc) in descriptions {
                set.category_descriptions.entry(cat).or_insert(desc);
            }
        }

        // Workspace skills (highest precedence, overrides instance)
        if workspace_skills_dir.is_dir()
            && let Ok((skills, descriptions)) =
                load_skills_from_dir(workspace_skills_dir, SkillSource::Workspace).await
        {
            for skill in skills {
                set.skills.insert(skill.name.to_lowercase(), skill);
            }
            for (cat, desc) in descriptions {
                set.category_descriptions.entry(cat).or_insert(desc);
            }
        }

        if !set.skills.is_empty() {
            tracing::info!(
                count = set.skills.len(),
                names = %set.skills.keys().cloned().collect::<Vec<_>>().join(", "),
                "skills loaded"
            );
        }

        set
    }

    /// Get a skill by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(&name.to_lowercase())
    }

    /// Iterate over all loaded skills.
    pub fn iter(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    /// Number of loaded skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether any skills are loaded.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Render the skills summary for injection into the channel system prompt.
    ///
    /// The channel sees skill names and descriptions but is instructed to
    /// delegate actual skill execution to workers.
    pub fn render_channel_prompt(
        &self,
        prompt_engine: &crate::prompts::PromptEngine,
    ) -> crate::error::Result<String> {
        if self.skills.is_empty() {
            return Ok(String::new());
        }

        let mut sorted_skills: Vec<&Skill> = self.skills.values().collect();
        sorted_skills.sort_by(|a, b| a.name.cmp(&b.name));

        let skill_infos: Vec<crate::prompts::SkillInfo> = sorted_skills
            .into_iter()
            .map(|s| crate::prompts::SkillInfo {
                name: s.name.clone(),
                description: index_description(&s.description),
                location: s.file_path.display().to_string(),
                suggested: false,
                category: s.category.clone(),
            })
            .collect();

        prompt_engine.render_skills_channel(skill_infos, &self.category_descriptions)
    }

    /// Render the skills listing for injection into a branch system prompt.
    ///
    /// Branches see the same index as channels but read skills directly via
    /// `read_skill`, or pass names to workers they spawn as `suggested_skills`.
    pub fn render_branch_skills(
        &self,
        prompt_engine: &crate::prompts::PromptEngine,
    ) -> crate::error::Result<String> {
        if self.skills.is_empty() {
            return Ok(String::new());
        }

        let mut sorted_skills: Vec<&Skill> = self.skills.values().collect();
        sorted_skills.sort_by(|a, b| a.name.cmp(&b.name));

        let skill_infos: Vec<crate::prompts::SkillInfo> = sorted_skills
            .into_iter()
            .map(|s| crate::prompts::SkillInfo {
                name: s.name.clone(),
                description: index_description(&s.description),
                location: s.file_path.display().to_string(),
                suggested: false,
                category: s.category.clone(),
            })
            .collect();

        prompt_engine.render_skills_branch(skill_infos, &self.category_descriptions)
    }

    /// Render the skills listing for injection into a worker system prompt.
    ///
    /// Workers see all available skills with any channel-suggested skills flagged.
    /// They decide which skills are relevant and read them via the read_skill tool.
    pub fn render_worker_skills(
        &self,
        suggested: &[&str],
        prompt_engine: &crate::prompts::PromptEngine,
    ) -> crate::error::Result<String> {
        if self.skills.is_empty() {
            return Ok(String::new());
        }

        let mut sorted_skills: Vec<&Skill> = self.skills.values().collect();
        sorted_skills.sort_by(|a, b| a.name.cmp(&b.name));

        let suggested_lower: Vec<String> = suggested.iter().map(|s| s.to_lowercase()).collect();

        let skill_infos: Vec<crate::prompts::SkillInfo> = sorted_skills
            .into_iter()
            .map(|s| crate::prompts::SkillInfo {
                suggested: suggested_lower.contains(&s.name.to_lowercase()),
                name: s.name.clone(),
                description: index_description(&s.description),
                location: s.file_path.display().to_string(),
                category: s.category.clone(),
            })
            .collect();

        prompt_engine.render_skills_worker(skill_infos, &self.category_descriptions)
    }

    /// Remove a skill by name.
    ///
    /// Only workspace-level skills can be removed via this method. Built-in
    /// and instance-level skills cannot be deleted through the per-agent API.
    ///
    /// Returns the base directory path if the skill was found and removed.
    pub async fn remove(&mut self, name: &str) -> anyhow::Result<Option<PathBuf>> {
        let key = name.to_lowercase();
        let skill = match self.skills.get(&key) {
            Some(s) => s,
            None => return Ok(None),
        };

        if skill.source == SkillSource::Builtin {
            anyhow::bail!(
                "cannot remove built-in skill '{}'; \
                 built-in skills are compiled into the binary",
                name
            );
        }

        if skill.source == SkillSource::Instance {
            anyhow::bail!(
                "cannot remove instance-level skill '{}' via the agent API; \
                 instance skills are shared across all agents and must be \
                 removed from the instance skills directory directly",
                name
            );
        }

        let skill = self.skills.remove(&key).unwrap();

        // Remove the skill directory from disk
        if skill.base_dir.exists() {
            tokio::fs::remove_dir_all(&skill.base_dir)
                .await
                .with_context(|| {
                    format!(
                        "failed to remove skill directory: {}",
                        skill.base_dir.display()
                    )
                })?;

            tracing::info!(
                skill = %name,
                path = %skill.base_dir.display(),
                "skill removed"
            );
        }

        Ok(Some(skill.base_dir))
    }

    /// List all loaded skills with their metadata.
    pub fn list(&self) -> Vec<SkillInfo> {
        let mut skills: Vec<_> = self.skills.values().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));

        skills
            .into_iter()
            .map(|s| SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
                file_path: s.file_path.clone(),
                base_dir: s.base_dir.clone(),
                source: s.source.clone(),
                source_repo: s.source_repo.clone(),
                category: s.category.clone(),
            })
            .collect()
    }
}

/// Truncate a description to the index budget (in characters) with an
/// ellipsis.
///
/// The budget is enforced on create/edit paths; skills that predate it (or
/// were installed from a registry) render truncated rather than failing.
fn index_description(description: &str) -> String {
    if description.chars().count() <= DESCRIPTION_BUDGET {
        return description.to_string();
    }

    let truncated: String = description
        .chars()
        .take(DESCRIPTION_BUDGET.saturating_sub(3))
        .collect();
    format!("{truncated}...")
}

/// Public skill information for API responses.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub source: SkillSource,
    pub source_repo: Option<String>,
    /// Category derived from the directory path.
    pub category: String,
}

/// Name of the archive directory under a workspace skills root. Hidden, so
/// archived skills are excluded from discovery.
pub const ARCHIVE_DIR: &str = ".archive";

/// Move a skill directory into the workspace archive instead of deleting it.
///
/// The skill's path relative to the skills root is preserved under
/// `.archive/`, so a categorized skill (`{category}/{name}`) restores to the
/// same location. An existing archived copy is suffixed away (`{name}-1`,
/// `{name}-2`, ...) rather than overwritten, so repeated archive cycles
/// never destroy a prior snapshot.
pub async fn archive_skill_dir(
    workspace_skills_dir: &Path,
    skill_base_dir: &Path,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let relative_parent = skill_base_dir
        .parent()
        .and_then(|parent| parent.strip_prefix(workspace_skills_dir).ok())
        .unwrap_or_else(|| Path::new(""));

    let archive_parent = workspace_skills_dir.join(ARCHIVE_DIR).join(relative_parent);
    tokio::fs::create_dir_all(&archive_parent)
        .await
        .with_context(|| format!("failed to create {}", archive_parent.display()))?;

    let mut destination = archive_parent.join(name);
    let mut suffix = 0usize;
    while destination.exists() {
        suffix += 1;
        destination = archive_parent.join(format!("{name}-{suffix}"));
    }

    tokio::fs::rename(skill_base_dir, &destination)
        .await
        .with_context(|| {
            format!(
                "failed to archive {} to {}",
                skill_base_dir.display(),
                destination.display()
            )
        })?;

    Ok(destination)
}

/// Find the most recently archived copy of a skill in `dir`: the highest
/// suffix of `{name}`, or `None`.
fn latest_archived_copy(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut source = None;
    let plain = dir.join(name);
    if plain.is_dir() {
        source = Some(plain);
    }
    let mut suffix = 1usize;
    loop {
        let candidate = dir.join(format!("{name}-{suffix}"));
        if !candidate.is_dir() {
            break;
        }
        source = Some(candidate);
        suffix += 1;
    }
    source
}

/// Restore the most recently archived copy of a skill to the location it
/// was archived from (category directories are preserved).
///
/// Fails when no archived copy exists or when an active skill directory
/// already occupies the name.
pub async fn restore_skill_dir(workspace_skills_dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let archive_root = workspace_skills_dir.join(ARCHIVE_DIR);

    // Look in the archive root, then one category level deep — mirroring
    // discovery's two levels.
    let mut found: Option<(PathBuf, PathBuf)> = latest_archived_copy(&archive_root, name)
        .map(|source| (source, workspace_skills_dir.join(name)));

    if found.is_none()
        && let Ok(mut entries) = tokio::fs::read_dir(&archive_root).await
    {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let category_dir = entry.path();
            if !category_dir.is_dir() {
                continue;
            }
            if let Some(source) = latest_archived_copy(&category_dir, name) {
                let category = entry.file_name();
                found = Some((source, workspace_skills_dir.join(category).join(name)));
                break;
            }
        }
    }

    let Some((source, destination)) = found else {
        anyhow::bail!(
            "no archived copy of '{name}' found under {}",
            archive_root.display()
        );
    };

    if destination.exists() {
        anyhow::bail!(
            "cannot restore '{name}': an active skill directory already exists at {}",
            destination.display()
        );
    }

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    tokio::fs::rename(&source, &destination)
        .await
        .with_context(|| {
            format!(
                "failed to restore {} to {}",
                source.display(),
                destination.display()
            )
        })?;

    Ok(destination)
}

/// Load all skills from a directory.
///
/// Each subdirectory containing a `SKILL.md` file is treated as a skill. A
/// subdirectory without one is treated as a category and scanned one level
/// deeper, so both `skills/{name}/SKILL.md` and
/// `skills/{category}/{name}/SKILL.md` load. Hidden directories (`.archive`,
/// `.snapshots`, `.git`, ...) are excluded from discovery.
///
/// Returns the loaded skills and any category descriptions found in
/// `index.md` files within category directories.
async fn load_skills_from_dir(
    dir: &Path,
    source: SkillSource,
) -> anyhow::Result<(Vec<Skill>, HashMap<String, String>)> {
    let mut skills = Vec::new();
    let mut category_descriptions = HashMap::new();

    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("failed to read skills directory: {}", dir.display()))?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }

        if path.join("SKILL.md").exists() {
            load_skill_into(&mut skills, &path, "general", source.clone()).await;
            continue;
        }

        // Category directory: scan its direct subdirectories for skills.
        let category_name = entry.file_name().to_string_lossy().to_string();

        // Load category description from index.md, if present.
        if let Ok(Some(desc)) = load_category_description(&path).await {
            category_descriptions.insert(category_name.clone(), desc);
        }

        let Ok(mut category_entries) = tokio::fs::read_dir(&path).await else {
            continue;
        };
        while let Ok(Some(category_entry)) = category_entries.next_entry().await {
            let skill_dir = category_entry.path();
            if !skill_dir.is_dir()
                || category_entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.')
                || !skill_dir.join("SKILL.md").exists()
            {
                continue;
            }
            load_skill_into(&mut skills, &skill_dir, &category_name, source.clone()).await;
        }
    }

    Ok((skills, category_descriptions))
}

/// Load a category description from an `index.md` file inside a category
/// directory. Returns `None` when the file is absent or has no description.
async fn load_category_description(category_dir: &Path) -> anyhow::Result<Option<String>> {
    let index_path = category_dir.join("index.md");
    let raw = match tokio::fs::read_to_string(&index_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let (frontmatter, _body) = parse_skill_markdown(&raw)?;
    Ok(frontmatter.description)
}

/// Load one skill directory into `skills`, logging failures and platform gates.
async fn load_skill_into(
    skills: &mut Vec<Skill>,
    path: &Path,
    category: &str,
    source: SkillSource,
) {
    let skill_file = path.join("SKILL.md");
    match load_skill(&skill_file, path, category, source).await {
        Ok(Some(skill)) => {
            tracing::debug!(
                name = %skill.name,
                path = %skill_file.display(),
                "loaded skill"
            );
            skills.push(skill);
        }
        Ok(None) => {
            tracing::debug!(
                path = %skill_file.display(),
                "skill gated to another platform, skipping"
            );
        }
        Err(error) => {
            tracing::warn!(
                path = %skill_file.display(),
                %error,
                "failed to load skill, skipping"
            );
        }
    }
}

/// Load a single skill from its SKILL.md file.
///
/// Returns `Ok(None)` when the skill declares `platforms` that don't include
/// the host platform.
async fn load_skill(
    file_path: &Path,
    base_dir: &Path,
    category: &str,
    source: SkillSource,
) -> anyhow::Result<Option<Skill>> {
    let raw = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("failed to read {}", file_path.display()))?;

    let (frontmatter, body) = parse_skill_markdown(&raw)?;

    if let Some(platforms) = &frontmatter.platforms
        && !Platform::list_matches_host(platforms)
    {
        return Ok(None);
    }

    let name = frontmatter.name.clone().unwrap_or_else(|| {
        // Fall back to directory name if no name in frontmatter
        base_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    // Resolve {baseDir} template variable in the body
    let base_dir_str = base_dir.to_string_lossy();
    let content = body.replace("{baseDir}", &base_dir_str);

    let linked_files = collect_linked_files(base_dir).await;

    Ok(Some(Skill {
        name,
        description: frontmatter.description.unwrap_or_default(),
        file_path: file_path.to_path_buf(),
        base_dir: base_dir.to_path_buf(),
        content,
        source,
        source_repo: frontmatter.source_repo,
        tags: frontmatter.tags.unwrap_or_default(),
        related_skills: frontmatter.related_skills.unwrap_or_default(),
        linked_files,
        category: category.to_string(),
    }))
}

/// List files under the skill's support subdirectories, relative to `base_dir`.
async fn collect_linked_files(base_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();

    for subdir in SUPPORT_SUBDIRS {
        let mut pending = vec![base_dir.join(subdir)];

        while let Some(dir) = pending.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                // Support directories are optional; absence is the common case.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(%error, path = %dir.display(), "failed to read skill support dir");
                    continue;
                }
            };
            loop {
                match entries.next_entry().await {
                    Ok(Some(entry)) => {
                        let path = entry.path();
                        if path.is_dir() {
                            pending.push(path);
                        } else if let Ok(relative) = path.strip_prefix(base_dir) {
                            files.push(relative.to_string_lossy().to_string());
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, path = %dir.display(), "failed to enumerate skill support dir");
                        break;
                    }
                }
            }
        }
    }

    files.sort();
    files
}

/// Split YAML frontmatter from a markdown file and parse it into
/// [`SkillFrontmatter`].
///
/// Expects `---` delimiters. Content without frontmatter yields default
/// (empty) frontmatter and the full content as body.
pub(crate) fn parse_skill_markdown(content: &str) -> anyhow::Result<(SkillFrontmatter, String)> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Ok((SkillFrontmatter::default(), content.to_string()));
    }

    // Find the closing ---
    let after_opening = &trimmed[3..];
    let Some(end_pos) = after_opening.find("\n---") else {
        anyhow::bail!("unclosed frontmatter: missing closing ---");
    };

    let frontmatter_str = after_opening[..end_pos].trim();
    let body_start = 3 + end_pos + 4; // skip opening --- + content + \n---
    let body = trimmed[body_start..].trim_start_matches('\n').to_string();

    let frontmatter = if frontmatter_str.is_empty() {
        SkillFrontmatter::default()
    } else {
        serde_yaml::from_str(frontmatter_str).context("failed to parse skill frontmatter")?
    };

    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = indoc::indoc! {r#"
            ---
            name: weather
            description: Get current weather and forecasts (no API key required).
            homepage: https://wttr.in/:help
            ---

            # Weather

            Two free services, no API keys needed.
        "#};

        let (fm, body) = parse_skill_markdown(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("weather"));
        assert_eq!(
            fm.description.as_deref(),
            Some("Get current weather and forecasts (no API key required).")
        );
        assert!(body.starts_with("# Weather"));
    }

    #[test]
    fn test_parse_frontmatter_with_metadata_json() {
        let content = indoc::indoc! {r#"
            ---
            name: github
            description: "Interact with GitHub using the gh CLI."
            metadata: { "openclaw": { "emoji": "octopus", "requires": { "bins": ["gh"] } } }
            ---

            # GitHub Skill
        "#};

        let (fm, body) = parse_skill_markdown(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("github"));
        assert_eq!(
            fm.description.as_deref(),
            Some("Interact with GitHub using the gh CLI.")
        );
        assert!(body.starts_with("# GitHub Skill"));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# Just a markdown file\n\nNo frontmatter here.";
        let (fm, body) = parse_skill_markdown(content).unwrap();
        assert!(fm.name.is_none());
        assert!(fm.description.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_frontmatter_quoted_description() {
        let content = indoc::indoc! {r#"
            ---
            name: test
            description: "A skill with 'quotes' inside"
            ---

            Body here.
        "#};

        let (fm, _body) = parse_skill_markdown(content).unwrap();
        assert_eq!(
            fm.description.as_deref(),
            Some("A skill with 'quotes' inside")
        );
    }

    #[test]
    fn test_parse_frontmatter_lists_and_multiline() {
        let content = indoc::indoc! {r#"
            ---
            name: deploy
            description: >-
              Deploy the app to production
              with the standard checklist.
            tags:
              - ops
              - release
            related_skills: [rollback, incident-response]
            platforms:
              - linux
              - darwin
            ---

            Body.
        "#};

        let (fm, _body) = parse_skill_markdown(content).unwrap();
        assert_eq!(
            fm.description.as_deref(),
            Some("Deploy the app to production with the standard checklist.")
        );
        assert_eq!(
            fm.tags.as_deref(),
            Some(&["ops".to_string(), "release".to_string()][..])
        );
        assert_eq!(
            fm.related_skills.as_deref(),
            Some(&["rollback".to_string(), "incident-response".to_string()][..])
        );
        assert_eq!(
            fm.platforms.as_deref(),
            Some(&[Platform::Linux, Platform::Macos][..])
        );
    }

    #[test]
    fn test_parse_frontmatter_unknown_platform_tolerated() {
        let content = "---\nname: mobile\nplatforms: [android]\n---\n\nBody.";
        let (fm, _body) = parse_skill_markdown(content).unwrap();
        assert_eq!(fm.platforms.as_deref(), Some(&[Platform::Other][..]));
    }

    #[test]
    fn test_index_description_truncated() {
        let long = "x".repeat(DESCRIPTION_BUDGET + 40);
        let rendered = index_description(&long);
        assert!(rendered.chars().count() <= DESCRIPTION_BUDGET);
        assert!(rendered.ends_with("..."));

        let short = "fits fine";
        assert_eq!(index_description(short), short);
    }

    #[test]
    fn test_index_description_budget_is_chars_not_bytes() {
        // 79 two-byte chars: 158 bytes, but within the 80-char budget.
        let multibyte = "é".repeat(DESCRIPTION_BUDGET - 1);
        assert_eq!(index_description(&multibyte), multibyte);

        let over = "é".repeat(DESCRIPTION_BUDGET + 10);
        let rendered = index_description(&over);
        assert_eq!(rendered.chars().count(), DESCRIPTION_BUDGET);
        assert!(rendered.ends_with("..."));
    }

    #[tokio::test]
    async fn archive_and_restore_preserve_category() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        let skill_dir = root.join("ops").join("deploy");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: d\n---\nBody.",
        )
        .await
        .unwrap();

        let archived = archive_skill_dir(&root, &skill_dir, "deploy")
            .await
            .unwrap();
        assert!(archived.starts_with(root.join(ARCHIVE_DIR).join("ops")));
        assert!(!skill_dir.exists());

        let restored = restore_skill_dir(&root, "deploy").await.unwrap();
        assert_eq!(restored, skill_dir);
        assert!(skill_dir.join("SKILL.md").exists());
    }

    #[tokio::test]
    async fn repeated_archive_keeps_prior_copies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");

        for round in 0..2 {
            let skill_dir = root.join("deploy");
            tokio::fs::create_dir_all(&skill_dir).await.unwrap();
            tokio::fs::write(skill_dir.join("SKILL.md"), format!("round {round}"))
                .await
                .unwrap();
            archive_skill_dir(&root, &skill_dir, "deploy")
                .await
                .unwrap();
        }

        assert!(root.join(ARCHIVE_DIR).join("deploy").is_dir());
        assert!(root.join(ARCHIVE_DIR).join("deploy-1").is_dir());

        // Restore takes the most recent copy.
        let restored = restore_skill_dir(&root, "deploy").await.unwrap();
        let content = tokio::fs::read_to_string(restored.join("SKILL.md"))
            .await
            .unwrap();
        assert_eq!(content, "round 1");
    }

    #[test]
    fn test_platform_other_never_matches_host() {
        assert!(!Platform::list_matches_host(&[Platform::Other]));
        assert!(!Platform::list_matches_host(&[]));
        // The full list contains the host on any supported platform.
        assert_eq!(
            Platform::list_matches_host(&[Platform::Linux, Platform::Macos, Platform::Windows]),
            Platform::host() != Platform::Other
        );
    }

    #[test]
    fn test_skill_set_channel_prompt_empty() {
        let set = SkillSet::default();
        let engine = crate::prompts::PromptEngine::new("en").unwrap();
        assert!(set.render_channel_prompt(&engine).unwrap().is_empty());
    }

    #[test]
    fn test_skill_set_channel_prompt() {
        let mut set = SkillSet::default();
        set.skills.insert(
            "weather".into(),
            Skill {
                name: "weather".into(),
                description: "Get weather forecasts".into(),
                file_path: PathBuf::from("/skills/weather/SKILL.md"),
                base_dir: PathBuf::from("/skills/weather"),
                content: "# Weather\n\nUse curl.".into(),
                source: SkillSource::Instance,
                source_repo: None,
                tags: Vec::new(),
                related_skills: Vec::new(),
                linked_files: Vec::new(),
                category: "general".to_string(),
            },
        );

        let engine = crate::prompts::PromptEngine::new("en").unwrap();
        let prompt = set.render_channel_prompt(&engine).unwrap();
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>weather</name>"));
        assert!(prompt.contains("<description>Get weather forecasts</description>"));
    }

    #[test]
    fn test_skill_set_worker_skills() {
        let mut set = SkillSet::default();
        set.skills.insert(
            "weather".into(),
            Skill {
                name: "weather".into(),
                description: "Get weather forecasts".into(),
                file_path: PathBuf::from("/skills/weather/SKILL.md"),
                base_dir: PathBuf::from("/skills/weather"),
                content: "# Weather\n\nUse curl.".into(),
                source: SkillSource::Instance,
                source_repo: None,
                tags: Vec::new(),
                related_skills: Vec::new(),
                linked_files: Vec::new(),
                category: "general".to_string(),
            },
        );

        let engine = crate::prompts::PromptEngine::new("en").unwrap();

        // Without suggestions
        let prompt = set.render_worker_skills(&[], &engine).unwrap();
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>weather</name>"));
        assert!(prompt.contains("<description>Get weather forecasts</description>"));
        assert!(!prompt.contains("suggested=\"true\""));

        // With suggestion
        let prompt = set.render_worker_skills(&["weather"], &engine).unwrap();
        assert!(prompt.contains("suggested=\"true\""));

        // Empty set returns empty string
        let empty_set = SkillSet::default();
        let prompt = empty_set.render_worker_skills(&[], &engine).unwrap();
        assert!(prompt.is_empty());
    }

    fn make_skill(name: &str, source: SkillSource) -> Skill {
        Skill {
            name: name.into(),
            description: format!("{name} skill"),
            file_path: PathBuf::from(format!("/skills/{name}/SKILL.md")),
            base_dir: PathBuf::from(format!("/tmp/test-skills-{}", uuid::Uuid::new_v4())),
            content: format!("# {name}"),
            source,
            source_repo: None,
            tags: Vec::new(),
            related_skills: Vec::new(),
            linked_files: Vec::new(),
            category: "general".to_string(),
        }
    }

    #[tokio::test]
    async fn remove_builtin_skill_is_rejected() {
        let mut set = SkillSet::default();
        set.skills.insert(
            "my-skill".into(),
            make_skill("my-skill", SkillSource::Builtin),
        );

        let result = set.remove("my-skill").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("built-in skill"), "unexpected error: {msg}");
        assert!(set.skills.contains_key("my-skill"));
    }

    #[tokio::test]
    async fn remove_instance_skill_is_rejected() {
        let mut set = SkillSet::default();
        set.skills.insert(
            "my-skill".into(),
            make_skill("my-skill", SkillSource::Instance),
        );

        let result = set.remove("my-skill").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("instance-level skill"),
            "unexpected error: {msg}"
        );

        // Skill should still be in the set (not removed)
        assert!(set.skills.contains_key("my-skill"));
    }

    #[tokio::test]
    async fn remove_workspace_skill_succeeds() {
        let mut set = SkillSet::default();
        let skill = make_skill("my-skill", SkillSource::Workspace);
        // Don't create the directory on disk - remove should still return the path
        let expected_dir = skill.base_dir.clone();
        set.skills.insert("my-skill".into(), skill);

        let result = set.remove("my-skill").await;
        assert!(result.is_ok());
        let path = result.unwrap();
        assert_eq!(path, Some(expected_dir));
        assert!(!set.skills.contains_key("my-skill"));
    }

    #[tokio::test]
    async fn remove_nonexistent_skill_returns_none() {
        let mut set = SkillSet::default();
        let result = set.remove("nonexistent").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn remove_is_case_insensitive() {
        let mut set = SkillSet::default();
        set.skills.insert(
            "my-skill".into(),
            make_skill("my-skill", SkillSource::Instance),
        );

        let result = set.remove("MY-SKILL").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("instance-level skill")
        );
    }
}
