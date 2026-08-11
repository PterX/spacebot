//! Identity file loading: SOUL.md, IDENTITY.md, ROLE.md.
//!
//! Identity files live in the **agent root** directory (one level above the
//! workspace), which places them outside the sandbox boundary. This means
//! worker file tools cannot read or write them — all identity mutations must
//! go through the identity management API or factory tools.
//!
//! USER.md is deprecated — human context now lives on the org graph via
//! `HUMAN.md` files in `instance_dir/humans/{id}/` and is inherited by
//! linked agents automatically.

use anyhow::Context as _;
use std::path::Path;

/// Loaded identity files for an agent.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub soul: Option<String>,
    pub identity: Option<String>,
    pub role: Option<String>,
}

impl Identity {
    /// Load identity files from the agent root directory.
    ///
    /// Identity files live at `instance_dir/agents/{id}/` — one level above
    /// the workspace — so they are outside the sandbox boundary and
    /// inaccessible to worker file tools.
    pub async fn load(identity_dir: &Path) -> Self {
        Self {
            soul: load_optional_file(&identity_dir.join("SOUL.md")).await,
            identity: load_optional_file(&identity_dir.join("IDENTITY.md")).await,
            role: load_optional_file(&identity_dir.join("ROLE.md")).await,
        }
    }

    /// Render identity context for injection into system prompts.
    ///
    /// Files render verbatim — an identity file carries its own headings,
    /// so the renderer adds none — and blank files render nothing.
    pub fn render(&self) -> String {
        let mut output = String::new();

        for section in [&self.soul, &self.identity, &self.role] {
            let Some(content) = section else { continue };
            let content = content.trim();
            if content.is_empty() {
                continue;
            }
            output.push_str(content);
            output.push_str("\n\n");
        }

        output
    }
}

/// Default identity file templates for new agents.
///
/// Uses the `main-agent` preset content so fresh instances start with a
/// reasonable baseline rather than empty placeholder comments.
const DEFAULT_IDENTITY_FILES: &[(&str, &str)] = &[
    ("SOUL.md", include_str!("../../presets/main-agent/SOUL.md")),
    (
        "IDENTITY.md",
        include_str!("../../presets/main-agent/IDENTITY.md"),
    ),
    ("ROLE.md", include_str!("../../presets/main-agent/ROLE.md")),
];

/// Write template identity files into the agent root if they don't already exist.
///
/// Identity files live in the agent root directory (one level above the
/// workspace), outside the sandbox boundary.
///
/// Only writes files that are missing — existing files are left untouched.
pub async fn scaffold_identity_files(identity_dir: &Path) -> crate::error::Result<()> {
    // Ensure the identity directory exists (it should, but be safe).
    tokio::fs::create_dir_all(identity_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create identity directory: {}",
                identity_dir.display()
            )
        })?;

    for (filename, content) in DEFAULT_IDENTITY_FILES {
        let target = identity_dir.join(filename);
        if !target.exists() {
            tokio::fs::write(&target, content).await.with_context(|| {
                format!("failed to write identity template: {}", target.display())
            })?;
            tracing::info!(path = %target.display(), "wrote identity template");
        }
    }

    Ok(())
}

/// Load a file if it exists, returning None if missing.
async fn load_optional_file(path: &Path) -> Option<String> {
    tokio::fs::read_to_string(path).await.ok()
}
