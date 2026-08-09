//! `spacebot wiki` — instance wiki management over the control API.

use super::client::{self, ApiClient};
use super::output;
use anyhow::Context as _;
use clap::Subcommand;
use spacebot::api::wiki::{
    WikiActionResponse, WikiHistoryResponse, WikiListResponse, WikiPageResponse,
};

#[derive(Subcommand)]
pub enum WikiCommand {
    /// List all wiki pages
    List {
        /// Filter by page type (entity, concept, decision, project, reference)
        #[arg(short = 't', long)]
        page_type: Option<String>,
    },
    /// Search wiki pages
    Search {
        /// Search query
        query: String,
        /// Filter by page type
        #[arg(short = 't', long)]
        page_type: Option<String>,
    },
    /// Print a wiki page (content goes to stdout)
    Get {
        /// Page slug
        slug: String,
        /// Read a specific historical version
        #[arg(short, long)]
        version: Option<i64>,
    },
    /// Create a new wiki page
    Create {
        /// Page title
        title: String,
        /// Page type (entity, concept, decision, project, reference)
        #[arg(short = 't', long)]
        page_type: String,
        /// Read page content from a file
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        /// Page content as a string
        #[arg(long, conflicts_with = "file")]
        content: Option<String>,
        /// Slug of a related page (repeatable)
        #[arg(short, long)]
        related: Vec<String>,
        /// Edit summary recorded in the page history
        #[arg(short, long)]
        summary: Option<String>,
    },
    /// Replace a page's content
    Edit {
        /// Page slug
        slug: String,
        /// Read new content from a file
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        /// New content as a string
        #[arg(long, conflicts_with = "file")]
        content: Option<String>,
        /// Edit summary recorded in the page history
        #[arg(short, long)]
        summary: Option<String>,
    },
    /// Archive a page
    Archive {
        /// Page slug
        slug: String,
    },
    /// List a page's version history
    History {
        /// Page slug
        slug: String,
        /// Maximum number of versions (default 20)
        #[arg(short, long)]
        limit: Option<i64>,
    },
    /// Restore a page to a historical version
    Restore {
        /// Page slug
        slug: String,
        /// Version number to restore
        version: i64,
    },
}

pub async fn run(ctx: &super::Context, wiki_cmd: WikiCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match wiki_cmd {
        WikiCommand::List { page_type } => {
            let mut path = "wiki".to_string();
            if let Some(page_type) = &page_type {
                path.push_str(&format!("?page_type={}", urlencoding::encode(page_type)));
            }
            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiListResponse = client::parse(value)?;
            print_page_list(&response);
            Ok(())
        }
        WikiCommand::Search { query, page_type } => {
            let mut path = format!("wiki/search?query={}", urlencoding::encode(&query));
            if let Some(page_type) = &page_type {
                path.push_str(&format!("&page_type={}", urlencoding::encode(page_type)));
            }
            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiListResponse = client::parse(value)?;
            print_page_list(&response);
            Ok(())
        }
        WikiCommand::Get { slug, version } => {
            let mut path = format!("wiki/{slug}");
            if let Some(version) = version {
                path.push_str(&format!("?version={version}"));
            }
            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiPageResponse = client::parse(value)?;
            let page = response.page;
            eprintln!(
                "{} ({}, v{}, updated {} by {})",
                page.title,
                page.page_type,
                page.version,
                output::short_timestamp(&page.updated_at),
                page.updated_by,
            );
            print!("{}", page.content);
            if !page.content.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        WikiCommand::Create {
            title,
            page_type,
            file,
            content,
            related,
            summary,
        } => {
            let content = resolve_content(file, content)?;
            let mut body = serde_json::json!({
                "title": title,
                "page_type": page_type,
                "content": content,
            });
            if !related.is_empty() {
                body["related"] = serde_json::json!(related);
            }
            if let Some(summary) = &summary {
                body["edit_summary"] = serde_json::json!(summary);
            }

            let value = client.post("wiki", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiPageResponse = client::parse(value)?;
            eprintln!(
                "Created '{}' (version {}).",
                response.page.slug, response.page.version
            );
            Ok(())
        }
        WikiCommand::Edit {
            slug,
            file,
            content,
            summary,
        } => {
            let new_content = resolve_content(file, content)?;

            // The API exposes string-replacement edits, so fetch the current
            // content and swap it wholesale for the provided source.
            let current = client.get(&format!("wiki/{slug}")).await?;
            let current: WikiPageResponse = client::parse(current)?;
            if current.page.content == new_content {
                eprintln!("No changes to '{slug}'.");
                return Ok(());
            }

            let mut body = serde_json::json!({
                "old_string": current.page.content,
                "new_string": new_content,
            });
            if let Some(summary) = &summary {
                body["edit_summary"] = serde_json::json!(summary);
            }

            let value = client.post(&format!("wiki/{slug}/edit"), &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiPageResponse = client::parse(value)?;
            eprintln!(
                "Updated '{}' (version {}).",
                response.page.slug, response.page.version
            );
            Ok(())
        }
        WikiCommand::Archive { slug } => {
            let value = client.delete(&format!("wiki/{slug}")).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiActionResponse = client::parse(value)?;
            eprintln!("{}", response.message);
            Ok(())
        }
        WikiCommand::History { slug, limit } => {
            let mut path = format!("wiki/{slug}/history");
            if let Some(limit) = limit {
                path.push_str(&format!("?limit={limit}"));
            }
            let value = client.get(&path).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiHistoryResponse = client::parse(value)?;
            if response.versions.is_empty() {
                eprintln!("No history for '{slug}'.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .versions
                .iter()
                .map(|version| {
                    vec![
                        version.version.to_string(),
                        output::short_timestamp(&version.created_at),
                        version.author_id.clone(),
                        version.edit_summary.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            output::table(&["VERSION", "DATE", "AUTHOR", "SUMMARY"], &rows);
            Ok(())
        }
        WikiCommand::Restore { slug, version } => {
            let body = serde_json::json!({ "version": version });
            let value = client.post(&format!("wiki/{slug}/restore"), &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: WikiPageResponse = client::parse(value)?;
            eprintln!(
                "Restored '{}' to version {} (now version {}).",
                response.page.slug, version, response.page.version
            );
            Ok(())
        }
    }
}

/// Render a page-summary table for list and search.
fn print_page_list(response: &WikiListResponse) {
    if response.pages.is_empty() {
        eprintln!("No pages found.");
        return;
    }
    let rows: Vec<Vec<String>> = response
        .pages
        .iter()
        .map(|page| {
            vec![
                page.slug.clone(),
                page.page_type.clone(),
                page.version.to_string(),
                output::short_timestamp(&page.updated_at),
                page.updated_by.clone(),
            ]
        })
        .collect();
    output::table(&["SLUG", "TYPE", "VERSION", "UPDATED", "BY"], &rows);
}

/// Resolve page content from `--file` or `--content`.
fn resolve_content(
    file: Option<std::path::PathBuf>,
    content: Option<String>,
) -> anyhow::Result<String> {
    match (file, content) {
        (Some(path), _) => std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display())),
        (None, Some(content)) => Ok(content),
        (None, None) => anyhow::bail!("provide page content with --file or --content"),
    }
}
