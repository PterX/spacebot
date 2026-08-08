//! Built-in skills embedded in the binary.
//!
//! To add a built-in skill:
//! 1. Create `skills/builtin/{name}/SKILL.md` with standard frontmatter
//! 2. Add an entry to `BUILTIN_SKILLS` below with `include_str!`

use std::path::PathBuf;

use super::{Skill, SkillSource, parse_skill_markdown};

/// Each entry is `(directory_name, raw SKILL.md content)`.
const BUILTIN_SKILLS: &[(&str, &str)] = &[(
    "wiki-writing",
    include_str!("../../skills/builtin/wiki-writing/SKILL.md"),
)];

/// Parse all built-in skills from embedded content.
pub fn load() -> Vec<Skill> {
    BUILTIN_SKILLS
        .iter()
        .filter_map(|(dir_name, raw)| match parse_builtin(dir_name, raw) {
            Ok(skill) => skill,
            Err(error) => {
                tracing::warn!(
                    skill = %dir_name,
                    %error,
                    "failed to parse builtin skill"
                );
                None
            }
        })
        .collect()
}

/// Returns `Ok(None)` when the skill declares `platforms` that don't include
/// the host platform, matching filesystem skill loading.
fn parse_builtin(dir_name: &str, raw: &str) -> anyhow::Result<Option<Skill>> {
    let (frontmatter, body) = parse_skill_markdown(raw)?;

    if let Some(platforms) = &frontmatter.platforms
        && !super::Platform::list_matches_host(platforms)
    {
        return Ok(None);
    }

    let name = frontmatter.name.unwrap_or_else(|| dir_name.to_string());

    Ok(Some(Skill {
        name,
        description: frontmatter.description.unwrap_or_default(),
        file_path: PathBuf::from(format!("builtin://{dir_name}/SKILL.md")),
        base_dir: PathBuf::from(format!("builtin://{dir_name}")),
        content: body,
        source: SkillSource::Builtin,
        source_repo: None,
        tags: frontmatter.tags.unwrap_or_default(),
        related_skills: frontmatter.related_skills.unwrap_or_default(),
        linked_files: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_builtin_skills() {
        let skills = load();
        assert!(!skills.is_empty());
        assert!(skills.iter().all(|s| s.source == SkillSource::Builtin));
    }

    #[test]
    fn wiki_writing_skill_is_loaded() {
        let skills = load();
        let wiki = skills.iter().find(|s| s.name == "wiki-writing");
        assert!(wiki.is_some(), "wiki-writing skill should be present");
        let wiki = wiki.unwrap();
        assert!(!wiki.description.is_empty());
        assert!(wiki.content.contains("## Language"));
    }

    #[test]
    fn parse_builtin_skill() {
        let raw = "---\nname: test-skill\ndescription: A test skill.\n---\n\n# Test\n\nBody here.";
        let skill = parse_builtin("test-skill", raw).unwrap().unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill.");
        assert_eq!(skill.source, SkillSource::Builtin);
        assert!(skill.content.contains("# Test"));
        assert!(skill.file_path.to_string_lossy().contains("builtin://"));
    }

    #[test]
    fn parse_builtin_falls_back_to_dir_name() {
        let raw = "---\ndescription: No name field.\n---\n\nBody.";
        let skill = parse_builtin("fallback-name", raw).unwrap().unwrap();
        assert_eq!(skill.name, "fallback-name");
    }

    #[test]
    fn parse_builtin_respects_platform_gate() {
        // "android" deserializes to Platform::Other, which never matches any host.
        let raw = "---\nname: mobile-only\ndescription: Gated.\nplatforms: [android]\n---\n\nBody.";
        let skill = parse_builtin("mobile-only", raw).unwrap();
        assert!(skill.is_none());
    }
}
