//! `spacebot project` — project, repo, and worktree management over the
//! control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::projects::{
    ActionResponse, DiskUsageResponse, ProjectListResponse, ProjectResponse, RepoResponse,
    WorktreeResponse,
};

#[derive(Subcommand)]
pub enum ProjectCommand {
    /// List projects
    List {
        /// Filter by status (active or archived)
        #[arg(short, long)]
        status: Option<String>,
    },
    /// Show a project with its repos and worktrees
    Get {
        /// Project ID
        id: String,
    },
    /// Create a project
    Create {
        /// Project name
        name: String,
        /// Absolute path to the project root directory
        root_path: String,
        /// Project description
        #[arg(long)]
        description: Option<String>,
        /// Icon identifier
        #[arg(long)]
        icon: Option<String>,
        /// Tag (repeatable)
        #[arg(short, long)]
        tags: Vec<String>,
        /// Skip scanning the root for git repos
        #[arg(long)]
        no_discover: bool,
    },
    /// Update a project
    Update {
        /// Project ID
        id: String,
        /// New project name
        #[arg(short, long)]
        name: Option<String>,
        /// New description
        #[arg(long)]
        description: Option<String>,
        /// New icon identifier
        #[arg(long)]
        icon: Option<String>,
        /// Replace tags (repeatable)
        #[arg(short, long)]
        tags: Option<Vec<String>>,
        /// New status (active or archived)
        #[arg(long)]
        status: Option<String>,
    },
    /// Delete a project (database records only, files stay on disk)
    Delete {
        /// Project ID
        id: String,
    },
    /// Re-scan the project root for repos and worktrees
    Scan {
        /// Project ID
        id: String,
    },
    /// Show disk usage of the project root
    DiskUsage {
        /// Project ID
        id: String,
    },
    /// Manage project repos
    #[command(subcommand)]
    Repo(RepoCommand),
    /// Manage project worktrees
    #[command(subcommand)]
    Worktree(WorktreeCommand),
}

#[derive(Subcommand)]
pub enum RepoCommand {
    /// Add a repo to a project
    Create {
        /// Project ID
        project_id: String,
        /// Repo name
        name: String,
        /// Repo path relative to the project root
        path: String,
        /// Remote URL
        #[arg(long)]
        remote_url: Option<String>,
        /// Default branch (defaults to main)
        #[arg(long)]
        default_branch: Option<String>,
        /// Repo description
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a repo from a project (database record only)
    Delete {
        /// Project ID
        project_id: String,
        /// Repository ID
        repo_id: String,
    },
}

#[derive(Subcommand)]
pub enum WorktreeCommand {
    /// Create a git worktree for a repo
    Create {
        /// Project ID
        project_id: String,
        /// Repository ID
        repo_id: String,
        /// Branch to check out in the worktree
        branch: String,
        /// Worktree directory name (defaults to the branch name)
        #[arg(short, long)]
        name: Option<String>,
        /// Commit-ish to create the branch from
        #[arg(long)]
        start_point: Option<String>,
    },
    /// Remove a worktree (runs git worktree remove)
    Delete {
        /// Project ID
        project_id: String,
        /// Worktree ID
        worktree_id: String,
    },
}

pub async fn run(ctx: &super::Context, project_cmd: ProjectCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match project_cmd {
        ProjectCommand::List { status } => {
            let path = match &status {
                Some(status) => {
                    format!("agents/projects?status={}", urlencoding::encode(status))
                }
                None => "agents/projects".to_string(),
            };
            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ProjectListResponse = client::parse(value)?;
            if response.projects.is_empty() {
                eprintln!("No projects.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .projects
                .iter()
                .map(|project| {
                    vec![
                        project.name.clone(),
                        output::enum_label(&project.status),
                        project.root_path.clone(),
                        output::short_timestamp(&project.updated_at),
                        project.id.clone(),
                    ]
                })
                .collect();
            output::table(&["NAME", "STATUS", "ROOT", "UPDATED", "ID"], &rows);
            Ok(())
        }
        ProjectCommand::Get { id } => {
            let value = client
                .get(&format!("agents/projects/{}", client::encode_path(&id)))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ProjectResponse = client::parse(value)?;
            let project = &response.project.project;
            println!("Name:     {}", project.name);
            println!("ID:       {}", project.id);
            println!("Status:   {}", output::enum_label(&project.status));
            println!("Root:     {}", project.root_path);
            if !project.description.is_empty() {
                println!("About:    {}", project.description);
            }
            if !project.tags.is_empty() {
                println!("Tags:     {}", project.tags.join(", "));
            }
            println!("Created:  {}", project.created_at);
            println!("Updated:  {}", project.updated_at);

            if !response.project.repos.is_empty() {
                println!();
                let rows: Vec<Vec<String>> = response
                    .project
                    .repos
                    .iter()
                    .map(|repo| {
                        vec![
                            repo.name.clone(),
                            repo.path.clone(),
                            repo.current_branch
                                .clone()
                                .unwrap_or_else(|| repo.default_branch.clone()),
                            repo.remote_url.clone(),
                            repo.id.clone(),
                        ]
                    })
                    .collect();
                output::table(&["REPO", "PATH", "BRANCH", "REMOTE", "ID"], &rows);
            }

            if !response.project.worktrees.is_empty() {
                println!();
                let rows: Vec<Vec<String>> = response
                    .project
                    .worktrees
                    .iter()
                    .map(|entry| {
                        vec![
                            entry.worktree.name.clone(),
                            entry.repo_name.clone(),
                            entry.worktree.branch.clone(),
                            entry.worktree.path.clone(),
                            entry.worktree.id.clone(),
                        ]
                    })
                    .collect();
                output::table(&["WORKTREE", "REPO", "BRANCH", "PATH", "ID"], &rows);
            }
            Ok(())
        }
        ProjectCommand::Create {
            name,
            root_path,
            description,
            icon,
            tags,
            no_discover,
        } => {
            let mut body = serde_json::json!({
                "name": name,
                "root_path": root_path,
                "auto_discover": !no_discover,
            });
            if let Some(description) = &description {
                body["description"] = serde_json::json!(description);
            }
            if let Some(icon) = &icon {
                body["icon"] = serde_json::json!(icon);
            }
            if !tags.is_empty() {
                body["tags"] = serde_json::json!(tags);
            }

            let value = client.post("agents/projects", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ProjectResponse = client::parse(value)?;
            let project = &response.project.project;
            eprintln!("Created project {} ({}).", project.name, project.id);
            if !no_discover {
                eprintln!("Scanning root for repos and worktrees in the background.");
            }
            Ok(())
        }
        ProjectCommand::Update {
            id,
            name,
            description,
            icon,
            tags,
            status,
        } => {
            let mut body = serde_json::json!({});
            if let Some(name) = &name {
                body["name"] = serde_json::json!(name);
            }
            if let Some(description) = &description {
                body["description"] = serde_json::json!(description);
            }
            if let Some(icon) = &icon {
                body["icon"] = serde_json::json!(icon);
            }
            if let Some(tags) = &tags {
                body["tags"] = serde_json::json!(tags);
            }
            if let Some(status) = &status {
                body["status"] = serde_json::json!(status);
            }
            if body.as_object().is_some_and(|fields| fields.is_empty()) {
                anyhow::bail!("nothing to update — pass at least one field flag");
            }

            let value = client
                .put(
                    &format!("agents/projects/{}", client::encode_path(&id)),
                    &body,
                )
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ProjectResponse = client::parse(value)?;
            eprintln!("Updated project {}.", response.project.project.name);
            Ok(())
        }
        ProjectCommand::Delete { id } => {
            let value = client
                .delete(&format!("agents/projects/{}", client::encode_path(&id)))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: ActionResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
        ProjectCommand::Scan { id } => {
            let value = client
                .post(
                    &format!("agents/projects/{}/scan", client::encode_path(&id)),
                    &serde_json::json!({}),
                )
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: ProjectResponse = client::parse(value)?;
            eprintln!(
                "Scan complete: {} repos, {} worktrees.",
                response.project.repos.len(),
                response.project.worktrees.len()
            );
            Ok(())
        }
        ProjectCommand::DiskUsage { id } => {
            let value = client
                .get(&format!(
                    "agents/projects/{}/disk-usage",
                    client::encode_path(&id)
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: DiskUsageResponse = client::parse(value)?;
            if response.entries.is_empty() {
                eprintln!("No entries — project root is empty or missing.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .entries
                .iter()
                .map(|entry| {
                    vec![
                        entry.name.clone(),
                        output::format_bytes(entry.bytes),
                        if entry.is_dir { "dir" } else { "file" }.to_string(),
                    ]
                })
                .collect();
            output::table(&["NAME", "SIZE", "TYPE"], &rows);
            println!();
            println!("Total: {}", output::format_bytes(response.total_bytes));
            Ok(())
        }
        ProjectCommand::Repo(repo_cmd) => match repo_cmd {
            RepoCommand::Create {
                project_id,
                name,
                path,
                remote_url,
                default_branch,
                description,
            } => {
                let mut body = serde_json::json!({
                    "name": name,
                    "path": path,
                });
                if let Some(remote_url) = &remote_url {
                    body["remote_url"] = serde_json::json!(remote_url);
                }
                if let Some(default_branch) = &default_branch {
                    body["default_branch"] = serde_json::json!(default_branch);
                }
                if let Some(description) = &description {
                    body["description"] = serde_json::json!(description);
                }

                let value = client
                    .post(
                        &format!("agents/projects/{}/repos", client::encode_path(&project_id)),
                        &body,
                    )
                    .await?;
                if ctx.json {
                    output::json(&value);
                    return Ok(());
                }
                let response: RepoResponse = client::parse(value)?;
                eprintln!("Added repo {} ({}).", response.repo.name, response.repo.id);
                Ok(())
            }
            RepoCommand::Delete {
                project_id,
                repo_id,
            } => {
                let value = client
                    .delete(&format!(
                        "agents/projects/{}/repos/{}",
                        client::encode_path(&project_id),
                        client::encode_path(&repo_id)
                    ))
                    .await?;
                if ctx.json {
                    output::json(&value);
                    return Ok(());
                }
                let result: ActionResponse = client::parse(value)?;
                eprintln!("{}", result.message);
                Ok(())
            }
        },
        ProjectCommand::Worktree(worktree_cmd) => match worktree_cmd {
            WorktreeCommand::Create {
                project_id,
                repo_id,
                branch,
                name,
                start_point,
            } => {
                let mut body = serde_json::json!({
                    "repo_id": repo_id,
                    "branch": branch,
                });
                if let Some(name) = &name {
                    body["worktree_name"] = serde_json::json!(name);
                }
                if let Some(start_point) = &start_point {
                    body["start_point"] = serde_json::json!(start_point);
                }

                let value = client
                    .post(
                        &format!(
                            "agents/projects/{}/worktrees",
                            client::encode_path(&project_id)
                        ),
                        &body,
                    )
                    .await?;
                if ctx.json {
                    output::json(&value);
                    return Ok(());
                }
                let response: WorktreeResponse = client::parse(value)?;
                eprintln!(
                    "Created worktree {} on {} ({}).",
                    response.worktree.name, response.worktree.branch, response.worktree.id
                );
                Ok(())
            }
            WorktreeCommand::Delete {
                project_id,
                worktree_id,
            } => {
                let value = client
                    .delete(&format!(
                        "agents/projects/{}/worktrees/{}",
                        client::encode_path(&project_id),
                        client::encode_path(&worktree_id)
                    ))
                    .await?;
                if ctx.json {
                    output::json(&value);
                    return Ok(());
                }
                let result: ActionResponse = client::parse(value)?;
                eprintln!("{}", result.message);
                Ok(())
            }
        },
    }
}
