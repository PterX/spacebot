//! Project workspace tracking: repos, worktrees, and project-level configuration.

pub mod git;
pub mod migration;
pub mod store;

pub use store::{
    CreateProjectInput, CreateRepoInput, CreateWorktreeInput, Project, ProjectRepo, ProjectStatus,
    ProjectStore, ProjectWorktree, UpdateProjectInput, detect_logo,
};

/// A worktree created through [`provision_worktree`]: the registered record
/// plus its absolute on-disk path.
#[derive(Debug, Clone)]
pub struct ProvisionedWorktree {
    pub worktree: ProjectWorktree,
    pub abs_path: std::path::PathBuf,
}

/// Create a git worktree for a project repo and register it in the store.
///
/// Handles path placement (single-repo projects put worktrees beside the
/// repo, multi-repo projects put them inside the project root), name
/// sanitization, `git worktree add`, and database registration. `repo_id`
/// may be omitted for single-repo projects.
pub async fn provision_worktree(
    store: &ProjectStore,
    project_id: &str,
    repo_id: Option<&str>,
    branch: &str,
    worktree_name: Option<&str>,
    created_by: &str,
) -> anyhow::Result<ProvisionedWorktree> {
    use anyhow::Context as _;

    let project = store
        .get_project(project_id)
        .await?
        .with_context(|| format!("project not found: {project_id}"))?;

    let repo = match repo_id {
        Some(repo_id) => {
            let repo = store
                .get_repo(repo_id)
                .await?
                .with_context(|| format!("repo not found: {repo_id}"))?;
            anyhow::ensure!(
                repo.project_id == project.id,
                "repo '{repo_id}' does not belong to project '{}'",
                project.id
            );
            repo
        }
        None => {
            let repos = store.list_repos(&project.id).await?;
            match repos.len() {
                1 => repos.into_iter().next().expect("len checked"),
                0 => anyhow::bail!("project '{}' has no repos", project.name),
                _ => anyhow::bail!(
                    "project '{}' has multiple repos — specify repo_id (one of: {})",
                    project.name,
                    repos
                        .iter()
                        .map(|r| format!("{} ({})", r.name, r.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }
    };

    let worktree_dir_name = worktree_name
        .map(str::to_string)
        .unwrap_or_else(|| branch.replace('/', "-"));
    anyhow::ensure!(
        !worktree_dir_name.is_empty()
            && !worktree_dir_name.contains('/')
            && !worktree_dir_name.contains('\\')
            && worktree_dir_name != ".."
            && worktree_dir_name != ".",
        "invalid worktree name '{worktree_dir_name}': must be a single directory name"
    );

    let root = std::path::Path::new(&project.root_path);
    let repo_abs_path = root.join(&repo.path);
    let is_single_repo = repo.path == ".";

    // Single-repo projects place worktrees beside the repo; multi-repo
    // projects place them inside the project root.
    let (worktree_abs_path, worktree_db_path) = if is_single_repo {
        let parent = root
            .parent()
            .context("single-repo project root has no parent directory")?;
        (
            parent.join(&worktree_dir_name),
            format!("../{worktree_dir_name}"),
        )
    } else {
        (root.join(&worktree_dir_name), worktree_dir_name.clone())
    };

    git::create_worktree(&repo_abs_path, &worktree_abs_path, branch, None)
        .await
        .context("git worktree add failed")?;

    let worktree = match store
        .create_worktree(CreateWorktreeInput {
            project_id: project.id.clone(),
            repo_id: repo.id.clone(),
            name: worktree_dir_name,
            path: worktree_db_path,
            branch: branch.to_string(),
            created_by: created_by.to_string(),
        })
        .await
    {
        Ok(worktree) => worktree,
        Err(error) => {
            if let Err(cleanup_error) =
                git::remove_worktree(&repo_abs_path, &worktree_abs_path).await
            {
                tracing::warn!(
                    %cleanup_error,
                    worktree_path = %worktree_abs_path.display(),
                    "failed to remove worktree after database registration failed"
                );
            }
            return Err(error).context("failed to register worktree in database");
        }
    };

    Ok(ProvisionedWorktree {
        worktree,
        abs_path: worktree_abs_path,
    })
}

/// Refresh the sandbox allowlist with all active project root paths.
///
/// Queries all active projects and injects their root paths into the sandbox
/// config. Takes effect immediately for subsequent subprocess calls. Should be
/// called after project create/delete/scan.
pub async fn refresh_sandbox_project_paths(
    project_store: &ProjectStore,
    sandbox: &crate::sandbox::Sandbox,
) {
    let projects = match project_store
        .list_projects(Some(ProjectStatus::Active))
        .await
    {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!(%error, "failed to list projects for sandbox refresh");
            return;
        }
    };

    let paths: Vec<std::path::PathBuf> = projects
        .iter()
        .map(|project| std::path::PathBuf::from(&project.root_path))
        .collect();

    sandbox.refresh_project_paths(paths);
}
