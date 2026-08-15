//! Durable records of what was sent to a model.
//!
//! Every LLM request in Spacebot passes through `SpacebotModel`, so recording
//! there covers every process type — channels, branches, workers, the
//! compactor, chronicle runs, cortex runs — including ones not written yet.
//! It is also the only layer that sees the whole request: by the time a
//! prompt reaches here the tool definitions have been assembled, which the
//! process that built the prompt never gets to see.
//!
//! Payloads are JSON files under `<data_dir>/prompts/<date>/`, indexed in
//! SQLite. The split is deliberate: the index answers "which requests", which
//! wants joins and ordering, while the payload is large, append-only, and
//! worth being able to read, diff and delete without going through the API.

use crate::prompts::PromptBlock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Which process produced a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRef {
    /// channel, branch, worker, compactor, cortex, chronicle, ingestion.
    pub kind: String,
    /// Branch/worker uuid, or the channel id for a channel turn.
    pub id: Option<String>,
    /// Narrower label where a kind has variants (builtin, opencode).
    pub process_type: Option<String>,
    pub channel_id: Option<String>,
}

/// What caused the request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Trigger {
    /// Short label: user_message, retrigger, spawn_worker, compaction, cron.
    pub kind: String,
    /// Conversation message that started the turn, when there was one.
    pub message_id: Option<String>,
    /// The text the process was prompted with.
    pub input: Option<String>,
    /// Process that started this one, when it was not a user.
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    pub name: String,
    pub provider: String,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// A tool definition as the model received it.
///
/// Schemas are kept whole: a tool whose description drifted is exactly the
/// kind of thing this record exists to make visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRef {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
    /// Serialized size, which is what the tool actually costs.
    pub chars: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageRef {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    pub cached_write_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseRef {
    pub text: Option<String>,
    pub tool_calls: Vec<String>,
    pub error: Option<String>,
}

/// The assembled system prompt and the map of what it is made of.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemRef {
    pub text: String,
    pub blocks: Vec<PromptBlock>,
}

/// One captured request, start to finish.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRecord {
    pub request_id: String,
    pub agent_id: String,
    pub process: ProcessRef,
    pub trigger: Trigger,
    pub model: ModelRef,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub system: SystemRef,
    pub tools: Vec<ToolRef>,
    /// Conversation history as serialized rig messages, verbatim.
    pub messages: serde_json::Value,
    pub history_length: usize,
    pub response: ResponseRef,
    pub usage: UsageRef,
}

impl PromptRecord {
    fn status(&self) -> &'static str {
        if self.response.error.is_some() {
            "error"
        } else {
            "ok"
        }
    }
}

/// Row shape for listing requests without loading their payloads.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PromptRequestSummary {
    pub request_id: String,
    pub process_kind: String,
    pub process_id: Option<String>,
    pub process_type: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub trigger: Option<String>,
    pub model: String,
    pub provider: String,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Option<i64>,
    pub system_chars: i64,
    pub history_length: i64,
    pub tool_count: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub status: String,
}

/// Context a process attaches to its model so its requests can be identified.
///
/// Without one a request is still recorded — the prompt is worth having even
/// unlabelled — it just cannot be linked back to the process that sent it.
#[derive(Debug, Clone, Default)]
pub struct DebugContext {
    pub process: Option<ProcessRef>,
    pub trigger: Option<Trigger>,
    /// Block map for the system prompt, from `PromptEngine::render_segmented`.
    pub blocks: Vec<PromptBlock>,
}

/// Writes prompt records and maintains their index.
#[derive(Clone)]
pub struct PromptRecordStore {
    dir: PathBuf,
    pool: SqlitePool,
    enabled: Arc<AtomicBool>,
}

impl PromptRecordStore {
    /// `dir` is the agent data directory; payloads go in `prompts/` beneath it.
    pub fn new(dir: &Path, pool: SqlitePool, enabled: bool) -> Self {
        Self {
            dir: dir.join("prompts"),
            pool,
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Path a record's payload is written to, relative to the data directory.
    fn relative_path(started_at: DateTime<Utc>, request_id: &str) -> String {
        format!(
            "prompts/{}/{request_id}.json",
            started_at.format("%Y-%m-%d")
        )
    }

    /// Persist a record. Failures are logged and swallowed — capture is a
    /// debugging aid and must never be able to fail a turn.
    pub async fn save(&self, record: &PromptRecord) {
        if let Err(error) = self.write(record).await {
            tracing::warn!(
                request_id = %record.request_id,
                %error,
                "failed to save prompt record"
            );
        }
    }

    async fn write(&self, record: &PromptRecord) -> anyhow::Result<()> {
        let day_dir = self
            .dir
            .join(record.started_at.format("%Y-%m-%d").to_string());
        tokio::fs::create_dir_all(&day_dir).await?;

        let path = day_dir.join(format!("{}.json", record.request_id));
        let payload = serde_json::to_vec_pretty(record)?;
        tokio::fs::write(&path, &payload).await?;

        let relative = Self::relative_path(record.started_at, &record.request_id);
        let status = record.status();

        sqlx::query(
            "INSERT OR REPLACE INTO prompt_requests (
                request_id, agent_id, process_kind, process_id, process_type,
                channel_id, message_id, trigger, model, provider, started_at,
                duration_ms, system_chars, history_length, tool_count,
                input_tokens, output_tokens, cached_tokens, status, path
             ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
             )",
        )
        .bind(&record.request_id)
        .bind(&record.agent_id)
        .bind(&record.process.kind)
        .bind(record.process.id.as_deref())
        .bind(record.process.process_type.as_deref())
        .bind(record.process.channel_id.as_deref())
        .bind(record.trigger.message_id.as_deref())
        .bind(&record.trigger.kind)
        .bind(&record.model.name)
        .bind(&record.model.provider)
        .bind(record.started_at)
        .bind(record.duration_ms as i64)
        .bind(record.system.text.chars().count() as i64)
        .bind(record.history_length as i64)
        .bind(record.tools.len() as i64)
        .bind(record.usage.input_tokens as i64)
        .bind(record.usage.output_tokens as i64)
        .bind((record.usage.cached_read_tokens + record.usage.cached_write_tokens) as i64)
        .bind(status)
        .bind(&relative)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Load a payload by exact id, or by unique id prefix.
    ///
    /// Prefix resolution exists because a record's id is something a person
    /// copies out of the UI and pastes into a terminal; an ambiguous prefix is
    /// reported rather than resolved arbitrarily.
    pub async fn get(&self, request_id: &str) -> anyhow::Result<Option<PromptRecord>> {
        let matches: Vec<(String, String)> = sqlx::query_as(
            "SELECT request_id, path FROM prompt_requests
             WHERE request_id = ? OR request_id LIKE ? || '%'
             LIMIT 2",
        )
        .bind(request_id)
        .bind(request_id)
        .fetch_all(&self.pool)
        .await?;

        let path = match matches.as_slice() {
            [] => return Ok(None),
            [(_, path)] => path.clone(),
            _ => anyhow::bail!("request id '{request_id}' is ambiguous; use more characters"),
        };

        let full = self.data_dir().join(&path);
        let bytes = match tokio::fs::read(&full).await {
            Ok(bytes) => bytes,
            // An index row whose payload was swept is a miss, not an error.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    /// The agent data directory the payload paths are relative to.
    fn data_dir(&self) -> &Path {
        // `dir` is `<data_dir>/prompts`, so its parent is the data directory.
        self.dir.parent().unwrap_or(&self.dir)
    }

    /// List requests, newest first, optionally scoped to a channel or process.
    pub async fn list(
        &self,
        channel_id: Option<&str>,
        process_id: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<PromptRequestSummary>> {
        let rows = sqlx::query_as::<_, PromptRequestSummary>(
            "SELECT request_id, process_kind, process_id, process_type, channel_id,
                    message_id, trigger, model, provider, started_at, duration_ms,
                    system_chars, history_length, tool_count, input_tokens,
                    output_tokens, cached_tokens, status
             FROM prompt_requests
             WHERE (?1 IS NULL OR channel_id = ?1)
               AND (?2 IS NULL OR process_id = ?2)
             ORDER BY started_at DESC
             LIMIT ?3",
        )
        .bind(channel_id)
        .bind(process_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Requests recorded for a conversation message, oldest first.
    ///
    /// A single message can produce several requests — the channel turn, any
    /// branches and workers it spawned, a compaction it triggered — so this
    /// returns all of them rather than assuming one.
    pub async fn for_message(&self, message_id: &str) -> anyhow::Result<Vec<PromptRequestSummary>> {
        let rows = sqlx::query_as::<_, PromptRequestSummary>(
            "SELECT request_id, process_kind, process_id, process_type, channel_id,
                    message_id, trigger, model, provider, started_at, duration_ms,
                    system_chars, history_length, tool_count, input_tokens,
                    output_tokens, cached_tokens, status
             FROM prompt_requests
             WHERE message_id = ?
             ORDER BY started_at ASC",
        )
        .bind(message_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Drop records older than `keep_days`, payloads and index rows together.
    ///
    /// Returns the number of records removed. Whole day directories are
    /// removed rather than individual files, which is why payloads are laid
    /// out by date.
    pub async fn sweep(&self, keep_days: i64) -> anyhow::Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::days(keep_days);

        let removed: Vec<String> = sqlx::query_scalar(
            "DELETE FROM prompt_requests WHERE started_at < ? RETURNING request_id",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };

        let cutoff_day = cutoff.format("%Y-%m-%d").to_string();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Directory names are `%Y-%m-%d`, which sorts lexicographically.
            if name.as_ref() < cutoff_day.as_str() {
                if let Err(error) = tokio::fs::remove_dir_all(entry.path()).await {
                    tracing::warn!(day = %name, %error, "failed to sweep prompt records");
                }
            }
        }

        Ok(removed.len())
    }
}

impl std::fmt::Debug for PromptRecordStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptRecordStore")
            .field("dir", &self.dir)
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (PromptRecordStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("pool");
        sqlx::query(include_str!(
            "../../migrations/20260815000001_prompt_requests.sql"
        ))
        .execute(&pool)
        .await
        .expect("migration");
        (PromptRecordStore::new(dir.path(), pool, true), dir)
    }

    fn record(request_id: &str, channel: &str, message_id: Option<&str>) -> PromptRecord {
        PromptRecord {
            request_id: request_id.to_string(),
            agent_id: "main".to_string(),
            process: ProcessRef {
                kind: "channel".to_string(),
                id: Some(channel.to_string()),
                process_type: None,
                channel_id: Some(channel.to_string()),
            },
            trigger: Trigger {
                kind: "user_message".to_string(),
                message_id: message_id.map(str::to_string),
                input: Some("hello".to_string()),
                parent: None,
            },
            model: ModelRef {
                name: "anthropic/claude-opus-5".to_string(),
                provider: "anthropic".to_string(),
                max_tokens: None,
                temperature: None,
            },
            started_at: Utc::now(),
            duration_ms: 1200,
            system: SystemRef {
                text: "# Orion\n\nBe useful.".to_string(),
                blocks: Vec::new(),
            },
            tools: Vec::new(),
            messages: serde_json::json!([]),
            history_length: 2,
            response: ResponseRef::default(),
            usage: UsageRef::default(),
        }
    }

    #[tokio::test]
    async fn round_trips_a_record_through_disk_and_index() {
        let (store, _dir) = store().await;
        let saved = record("aaaa1111", "telegram:1", Some("m7"));
        store.save(&saved).await;

        let loaded = store
            .get("aaaa1111")
            .await
            .expect("get")
            .expect("record present");
        assert_eq!(loaded.request_id, "aaaa1111");
        assert_eq!(loaded.system.text, "# Orion\n\nBe useful.");

        let listed = store
            .list(Some("telegram:1"), None, 10)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].system_chars, 19);
    }

    #[tokio::test]
    async fn resolves_a_unique_id_prefix_and_rejects_an_ambiguous_one() {
        let (store, _dir) = store().await;
        store.save(&record("abc123", "telegram:1", None)).await;
        store.save(&record("abc456", "telegram:1", None)).await;
        store.save(&record("zzz999", "telegram:1", None)).await;

        let unique = store.get("zzz").await.expect("get").expect("present");
        assert_eq!(unique.request_id, "zzz999");

        assert!(
            store.get("abc").await.is_err(),
            "an ambiguous prefix must be reported, not resolved arbitrarily"
        );
    }

    #[tokio::test]
    async fn lists_every_request_for_a_message() {
        let (store, _dir) = store().await;
        store.save(&record("r1", "telegram:1", Some("m42"))).await;
        store.save(&record("r2", "telegram:1", Some("m42"))).await;
        store.save(&record("r3", "telegram:1", Some("m43"))).await;

        let found = store.for_message("m42").await.expect("for_message");
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .all(|row| row.message_id.as_deref() == Some("m42"))
        );
    }

    #[tokio::test]
    async fn missing_record_is_a_miss_not_an_error() {
        let (store, _dir) = store().await;
        assert!(store.get("nothing").await.expect("get").is_none());
    }
}
