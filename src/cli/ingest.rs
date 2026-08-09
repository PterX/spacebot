//! `spacebot ingest` — agent file ingestion over the control API.

use super::client::{self, ApiClient};
use super::output;
use anyhow::Context as _;
use clap::Subcommand;
use spacebot::api::ingest::{IngestDeleteResponse, IngestFilesResponse, IngestUploadResponse};

#[derive(Subcommand)]
pub enum IngestCommand {
    /// List ingested files with progress
    List {
        /// Agent ID
        agent_id: String,
    },
    /// Upload a file to the agent's ingest directory
    Upload {
        /// Agent ID
        agent_id: String,
        /// File to upload
        file: std::path::PathBuf,
    },
    /// Delete an ingestion record from history
    Delete {
        /// Agent ID
        agent_id: String,
        /// Content hash of the file (see `ingest list`)
        content_hash: String,
    },
}

pub async fn run(ctx: &super::Context, ingest_cmd: IngestCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match ingest_cmd {
        IngestCommand::List { agent_id } => {
            let value = client
                .get(&format!(
                    "agents/ingest/files?agent_id={}",
                    urlencoding::encode(&agent_id)
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: IngestFilesResponse = client::parse(value)?;
            if response.files.is_empty() {
                eprintln!("No ingested files.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = response
                .files
                .iter()
                .map(|file| {
                    vec![
                        file.filename.clone(),
                        output::format_bytes(file.file_size.max(0) as u64),
                        file.status.clone(),
                        format!("{}/{}", file.chunks_completed, file.total_chunks),
                        output::short_timestamp(&file.started_at),
                        file.content_hash.clone(),
                    ]
                })
                .collect();
            output::table(
                &["FILENAME", "SIZE", "STATUS", "CHUNKS", "STARTED", "HASH"],
                &rows,
            );
            Ok(())
        }
        IngestCommand::Upload { agent_id, file } => {
            let bytes = tokio::fs::read(&file)
                .await
                .with_context(|| format!("failed to read {}", file.display()))?;
            let filename = file
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "upload.txt".to_string());

            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
            let form = reqwest::multipart::Form::new().part("file", part);

            let value = client
                .post_multipart(
                    &format!(
                        "agents/ingest/files?agent_id={}",
                        urlencoding::encode(&agent_id)
                    ),
                    form,
                )
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: IngestUploadResponse = client::parse(value)?;
            if response.uploaded.is_empty() {
                anyhow::bail!("nothing was uploaded — the file may be empty");
            }
            for name in response.uploaded {
                eprintln!("Uploaded {name}.");
            }
            Ok(())
        }
        IngestCommand::Delete {
            agent_id,
            content_hash,
        } => {
            let value = client
                .delete(&format!(
                    "agents/ingest/files?agent_id={}&content_hash={}",
                    urlencoding::encode(&agent_id),
                    urlencoding::encode(&content_hash)
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: IngestDeleteResponse = client::parse(value)?;
            if result.success {
                eprintln!("Deleted ingestion record {content_hash}.");
            }
            Ok(())
        }
    }
}
