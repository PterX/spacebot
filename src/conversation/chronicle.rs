//! Session chronicle persistence (SQLite).
//!
//! A chronicle is an append-only sequence of checkpoints over a channel's
//! conversation log. Each checkpoint summarizes exactly the span since the
//! previous one, so coverage is contiguous and no span is ever summarized
//! twice. Boundaries are anchored to `conversation_messages` rather than to a
//! channel's in-memory history, because the log is what survives restart and
//! what a later expansion can be resolved against.
//!
//! Ordering is `(created_at, id)` everywhere — cut selection, boundary
//! comparison, and expansion all use it. `created_at` defaults to
//! `CURRENT_TIMESTAMP`, which SQLite resolves to whole seconds, so `id`
//! breaks ties. Within a one-second burst the order may not be true arrival
//! order, but it is deterministic and identical for every consumer, which is
//! what contiguity requires.

use crate::conversation::history::ConversationMessage;
use crate::error::Result;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Row as _, SqlitePool};

/// Timestamp format matching SQLite's `CURRENT_TIMESTAMP`, so bound values and
/// stored values compare identically under `datetime()`.
const SQL_TIMESTAMP: &str = "%Y-%m-%d %H:%M:%S";

/// Render a timestamp in the same shape SQLite writes for `CURRENT_TIMESTAMP`.
pub fn sql_timestamp(at: DateTime<Utc>) -> String {
    at.format(SQL_TIMESTAMP).to_string()
}

/// Why a checkpoint was cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointKind {
    /// The configured message or token interval elapsed.
    Interval,
    /// The first checkpoint for a channel that already had history.
    Bootstrap,
    /// Context pressure forced a cut before the interval elapsed.
    Pressure,
    /// Emergency truncation discarded the span without summarizing it.
    Emergency,
    /// A higher-level summary over a contiguous run of checkpoints.
    Rollup,
}

impl CheckpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckpointKind::Interval => "interval",
            CheckpointKind::Bootstrap => "bootstrap",
            CheckpointKind::Pressure => "pressure",
            CheckpointKind::Emergency => "emergency",
            CheckpointKind::Rollup => "rollup",
        }
    }

    /// Map the stored column value back to a kind, defaulting to `Interval`
    /// for anything a future version wrote that this one does not know.
    pub fn from_db(value: &str) -> Self {
        match value {
            "bootstrap" => CheckpointKind::Bootstrap,
            "pressure" => CheckpointKind::Pressure,
            "emergency" => CheckpointKind::Emergency,
            "rollup" => CheckpointKind::Rollup,
            _ => CheckpointKind::Interval,
        }
    }
}

/// One end of a coverage range: a position in the `(created_at, id)` ordering.
///
/// `message_id` is `None` only for the open start of a channel that had no
/// logged messages when its chronicle began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChronicleBoundary {
    pub at: DateTime<Utc>,
    pub message_id: Option<String>,
}

impl ChronicleBoundary {
    pub fn new(at: DateTime<Utc>, message_id: Option<String>) -> Self {
        Self { at, message_id }
    }

    /// The boundary a channel with no prior chronicle starts from.
    pub fn origin(at: DateTime<Utc>) -> Self {
        Self {
            at,
            message_id: None,
        }
    }
}

/// A committed checkpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ChronicleCheckpoint {
    pub id: String,
    pub channel_id: String,
    pub seq: i64,
    pub level: i64,
    pub kind: CheckpointKind,
    pub title: String,
    pub summary: String,
    pub covers_from_at: DateTime<Utc>,
    pub covers_to_at: DateTime<Utc>,
    pub covers_from_message_id: Option<String>,
    pub covers_to_message_id: Option<String>,
    pub message_count: i64,
    pub token_estimate: i64,
    pub rolled_up_into: Option<String>,
    pub rolls_up_from_seq: Option<i64>,
    pub rolls_up_to_seq: Option<i64>,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ChronicleCheckpoint {
    /// The boundary a following checkpoint must start from.
    pub fn end_boundary(&self) -> ChronicleBoundary {
        ChronicleBoundary::new(self.covers_to_at, self.covers_to_message_id.clone())
    }
}

/// A checkpoint about to be committed.
#[derive(Debug, Clone)]
pub struct NewCheckpoint {
    pub channel_id: String,
    pub level: i64,
    pub kind: CheckpointKind,
    pub title: String,
    pub summary: String,
    pub covers_from: ChronicleBoundary,
    pub covers_to: ChronicleBoundary,
    pub message_count: i64,
    pub token_estimate: i64,
    pub rolls_up_from_seq: Option<i64>,
    pub rolls_up_to_seq: Option<i64>,
    pub model: Option<String>,
}

/// What happened to a commit attempt.
#[derive(Debug)]
pub enum CommitOutcome {
    Committed(Box<ChronicleCheckpoint>),
    /// The tail moved while this cut was being summarized, so its start
    /// boundary no longer joins the last committed checkpoint. The span stays
    /// unsummarized and the next cut covers it.
    Superseded {
        expected: Option<String>,
        found: Option<String>,
    },
}

/// Aggregate chronicle state for a channel, used to build the prompt header.
#[derive(Debug, Clone, Default)]
pub struct ChronicleStats {
    pub checkpoint_count: i64,
    pub interval_count: i64,
    pub rollup_count: i64,
    pub total_messages: i64,
    pub first_message_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    /// Messages logged after the newest checkpoint's end boundary.
    pub unsummarized_messages: i64,
}

/// Reads and writes chronicle checkpoints.
///
/// Unlike `ConversationLogger`, commits are awaited rather than fire-and-forget:
/// the boundary read and the insert have to be one transaction for coverage to
/// stay contiguous.
#[derive(Debug, Clone)]
pub struct ChronicleStore {
    pool: SqlitePool,
}

impl ChronicleStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The backing pool, so a test can reopen a store the way a restart would.
    #[cfg(test)]
    pub fn pool_for_tests(&self) -> &SqlitePool {
        &self.pool
    }

    /// The newest checkpoint at a level, or `None` when the chronicle is empty.
    pub async fn latest(
        &self,
        channel_id: &str,
        level: i64,
    ) -> Result<Option<ChronicleCheckpoint>> {
        let row = sqlx::query(
            "SELECT * FROM channel_chronicle_checkpoints \
             WHERE channel_id = ? AND level = ? \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(channel_id)
        .bind(level)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(row.map(checkpoint_from_row))
    }

    /// Commit a checkpoint, rejecting it if the tail moved underneath it.
    ///
    /// The start boundary must join the newest checkpoint's end boundary,
    /// checked inside the transaction that allocates the sequence. A racing
    /// commit that slips past the check trips the `(channel_id, seq)` or
    /// `(channel_id, level, covers_to_message_id)` unique index and is
    /// reported the same way.
    pub async fn commit(&self, new: NewCheckpoint) -> Result<CommitOutcome> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| anyhow::anyhow!(error))?;

        let last = sqlx::query(
            "SELECT seq, covers_to_message_id FROM channel_chronicle_checkpoints \
             WHERE channel_id = ? AND level = ? \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(&new.channel_id)
        .bind(new.level)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        let expected_start: Option<String> = last
            .as_ref()
            .and_then(|row| row.try_get("covers_to_message_id").ok().flatten());

        if expected_start != new.covers_from.message_id {
            return Ok(CommitOutcome::Superseded {
                expected: expected_start,
                found: new.covers_from.message_id,
            });
        }

        let next_seq: i64 = sqlx::query(
            "SELECT COALESCE(MAX(seq), 0) + 1 AS next FROM channel_chronicle_checkpoints \
             WHERE channel_id = ?",
        )
        .bind(&new.channel_id)
        .fetch_one(&mut *transaction)
        .await
        .map(|row| row.try_get("next").unwrap_or(1))
        .map_err(|error| anyhow::anyhow!(error))?;

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now();

        let insert = sqlx::query(
            "INSERT INTO channel_chronicle_checkpoints \
             (id, channel_id, seq, level, kind, title, summary, covers_from_at, covers_to_at, \
              covers_from_message_id, covers_to_message_id, message_count, token_estimate, \
              rolls_up_from_seq, rolls_up_to_seq, model, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&new.channel_id)
        .bind(next_seq)
        .bind(new.level)
        .bind(new.kind.as_str())
        .bind(&new.title)
        .bind(&new.summary)
        .bind(sql_timestamp(new.covers_from.at))
        .bind(sql_timestamp(new.covers_to.at))
        .bind(&new.covers_from.message_id)
        .bind(&new.covers_to.message_id)
        .bind(new.message_count)
        .bind(new.token_estimate)
        .bind(new.rolls_up_from_seq)
        .bind(new.rolls_up_to_seq)
        .bind(&new.model)
        .bind(sql_timestamp(created_at))
        .execute(&mut *transaction)
        .await;

        if let Err(error) = insert {
            if is_unique_violation(&error) {
                return Ok(CommitOutcome::Superseded {
                    expected: expected_start,
                    found: new.covers_from.message_id,
                });
            }
            return Err(anyhow::anyhow!(error).into());
        }

        transaction
            .commit()
            .await
            .map_err(|error| anyhow::anyhow!(error))?;

        Ok(CommitOutcome::Committed(Box::new(ChronicleCheckpoint {
            id,
            channel_id: new.channel_id,
            seq: next_seq,
            level: new.level,
            kind: new.kind,
            title: new.title,
            summary: new.summary,
            covers_from_at: new.covers_from.at,
            covers_to_at: new.covers_to.at,
            covers_from_message_id: new.covers_from.message_id,
            covers_to_message_id: new.covers_to.message_id,
            message_count: new.message_count,
            token_estimate: new.token_estimate,
            rolled_up_into: None,
            rolls_up_from_seq: new.rolls_up_from_seq,
            rolls_up_to_seq: new.rolls_up_to_seq,
            model: new.model,
            created_at,
        })))
    }

    /// Checkpoints at a level, newest first.
    pub async fn list(
        &self,
        channel_id: &str,
        level: i64,
        limit: i64,
    ) -> Result<Vec<ChronicleCheckpoint>> {
        let rows = sqlx::query(
            "SELECT * FROM channel_chronicle_checkpoints \
             WHERE channel_id = ? AND level = ? \
             ORDER BY seq DESC LIMIT ?",
        )
        .bind(channel_id)
        .bind(level)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(rows.into_iter().map(checkpoint_from_row).collect())
    }

    /// Checkpoints at a level whose coverage ends at or after `since`, oldest
    /// first. Used to select the recent window for the prompt.
    pub async fn list_since(
        &self,
        channel_id: &str,
        level: i64,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ChronicleCheckpoint>> {
        let rows = sqlx::query(
            "SELECT * FROM ( \
                SELECT * FROM channel_chronicle_checkpoints \
                WHERE channel_id = ? AND level = ? \
                  AND datetime(covers_to_at) >= datetime(?) \
                ORDER BY seq DESC LIMIT ? \
             ) ORDER BY seq ASC",
        )
        .bind(channel_id)
        .bind(level)
        .bind(sql_timestamp(since))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(rows.into_iter().map(checkpoint_from_row).collect())
    }

    /// Checkpoints at a level with a sequence below `before_seq`, oldest first.
    /// Used to select the older-history portion of the prompt view.
    pub async fn list_before_seq(
        &self,
        channel_id: &str,
        level: i64,
        before_seq: i64,
        limit: i64,
    ) -> Result<Vec<ChronicleCheckpoint>> {
        let rows = sqlx::query(
            "SELECT * FROM ( \
                SELECT * FROM channel_chronicle_checkpoints \
                WHERE channel_id = ? AND level = ? AND seq < ? \
                ORDER BY seq DESC LIMIT ? \
             ) ORDER BY seq ASC",
        )
        .bind(channel_id)
        .bind(level)
        .bind(before_seq)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(rows.into_iter().map(checkpoint_from_row).collect())
    }

    /// Checkpoints for the channel timeline, newest first.
    ///
    /// Ordered and filtered by commit time rather than coverage, so a
    /// checkpoint lands inline at the point the conversation reached it.
    pub async fn list_for_timeline(
        &self,
        channel_id: &str,
        limit: i64,
        before: Option<&str>,
    ) -> Result<Vec<ChronicleCheckpoint>> {
        let before_clause = if before.is_some() {
            "AND datetime(created_at) < datetime(?3)"
        } else {
            ""
        };
        let sql = format!(
            "SELECT * FROM channel_chronicle_checkpoints \
             WHERE channel_id = ?1 {before_clause} \
             ORDER BY created_at DESC LIMIT ?2"
        );

        let mut query = sqlx::query(&sql).bind(channel_id).bind(limit);
        if let Some(before) = before {
            query = query.bind(before);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;

        Ok(rows.into_iter().map(checkpoint_from_row).collect())
    }

    /// A single checkpoint by sequence number.
    pub async fn get_by_seq(
        &self,
        channel_id: &str,
        seq: i64,
    ) -> Result<Option<ChronicleCheckpoint>> {
        let row = sqlx::query(
            "SELECT * FROM channel_chronicle_checkpoints WHERE channel_id = ? AND seq = ?",
        )
        .bind(channel_id)
        .bind(seq)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(row.map(checkpoint_from_row))
    }

    /// A single checkpoint by id, scoped to its channel.
    pub async fn get_by_id(
        &self,
        channel_id: &str,
        id: &str,
    ) -> Result<Option<ChronicleCheckpoint>> {
        let row = sqlx::query(
            "SELECT * FROM channel_chronicle_checkpoints WHERE channel_id = ? AND id = ?",
        )
        .bind(channel_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(row.map(checkpoint_from_row))
    }

    /// Aggregate state for the prompt header.
    pub async fn stats(&self, channel_id: &str) -> Result<ChronicleStats> {
        let counts = sqlx::query(
            "SELECT COUNT(*) AS total, \
                    SUM(CASE WHEN level = 0 THEN 1 ELSE 0 END) AS intervals, \
                    SUM(CASE WHEN level > 0 THEN 1 ELSE 0 END) AS rollups \
             FROM channel_chronicle_checkpoints WHERE channel_id = ?",
        )
        .bind(channel_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        let messages = sqlx::query(
            "SELECT COUNT(*) AS total, MIN(created_at) AS first_at, MAX(created_at) AS last_at \
             FROM conversation_messages WHERE channel_id = ?",
        )
        .bind(channel_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        let latest = self.latest(channel_id, 0).await?;
        let unsummarized = match &latest {
            Some(checkpoint) => {
                self.count_messages_after(channel_id, &checkpoint.end_boundary())
                    .await?
            }
            None => messages.try_get("total").unwrap_or(0),
        };

        Ok(ChronicleStats {
            checkpoint_count: counts.try_get("total").unwrap_or(0),
            interval_count: counts.try_get("intervals").unwrap_or(0),
            rollup_count: counts.try_get("rollups").unwrap_or(0),
            total_messages: messages.try_get("total").unwrap_or(0),
            first_message_at: messages.try_get("first_at").ok(),
            last_message_at: messages.try_get("last_at").ok(),
            unsummarized_messages: unsummarized,
        })
    }

    /// How many logged messages sit after a boundary.
    pub async fn count_messages_after(
        &self,
        channel_id: &str,
        boundary: &ChronicleBoundary,
    ) -> Result<i64> {
        let sql = format!(
            "SELECT COUNT(*) AS total FROM conversation_messages \
             WHERE channel_id = ?1 AND {}",
            after_boundary_predicate(boundary)
        );

        let mut query = sqlx::query(&sql).bind(channel_id);
        query = bind_boundary(query, boundary);

        let row = query
            .fetch_one(&self.pool)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;

        Ok(row.try_get("total").unwrap_or(0))
    }

    /// Logged messages after a boundary, oldest first.
    pub async fn messages_after(
        &self,
        channel_id: &str,
        boundary: &ChronicleBoundary,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        let sql = format!(
            "SELECT id, channel_id, role, sender_name, sender_id, content, metadata, created_at \
             FROM conversation_messages \
             WHERE channel_id = ?1 AND {} \
             ORDER BY created_at ASC, id ASC LIMIT ?4",
            after_boundary_predicate(boundary)
        );

        let mut query = sqlx::query(&sql).bind(channel_id);
        query = bind_boundary(query, boundary);
        query = query.bind(limit);

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;

        Ok(rows.into_iter().map(message_from_row).collect())
    }

    /// Logged messages inside a checkpoint's coverage, oldest first.
    ///
    /// The range is half-open in the same sense the boundaries are:
    /// `(from, to]`.
    pub async fn messages_in_range(
        &self,
        channel_id: &str,
        from: &ChronicleBoundary,
        to: &ChronicleBoundary,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        let sql = format!(
            "SELECT id, channel_id, role, sender_name, sender_id, content, metadata, created_at \
             FROM conversation_messages \
             WHERE channel_id = ?1 AND {} \
               AND (datetime(created_at) < datetime(?4) \
                    OR (datetime(created_at) = datetime(?4) AND id <= ?5)) \
             ORDER BY created_at ASC, id ASC LIMIT ?6",
            after_boundary_predicate(from)
        );

        let mut query = sqlx::query(&sql).bind(channel_id);
        query = bind_boundary(query, from);
        query = query
            .bind(sql_timestamp(to.at))
            .bind(to.message_id.clone().unwrap_or_default())
            .bind(limit);

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;

        Ok(rows.into_iter().map(message_from_row).collect())
    }

    /// The oldest logged message's position, used to open a bootstrap range.
    pub async fn earliest_message(
        &self,
        channel_id: &str,
    ) -> Result<Option<(DateTime<Utc>, String)>> {
        let row = sqlx::query(
            "SELECT id, created_at FROM conversation_messages \
             WHERE channel_id = ? ORDER BY created_at ASC, id ASC LIMIT 1",
        )
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

        Ok(row.and_then(|row| {
            let id: String = row.try_get("id").ok()?;
            let at: DateTime<Utc> = row.try_get("created_at").ok()?;
            Some((at, id))
        }))
    }
}

/// SQL predicate selecting rows strictly after a boundary in `(created_at, id)`
/// order. Parameters 2 and 3 carry the boundary; `bind_boundary` supplies them
/// in the same order for every call site.
fn after_boundary_predicate(boundary: &ChronicleBoundary) -> &'static str {
    if boundary.message_id.is_some() {
        "(datetime(created_at) > datetime(?2) \
          OR (datetime(created_at) = datetime(?2) AND id > ?3))"
    } else {
        // An open start covers from its timestamp inclusive; the null id is
        // still bound so positional indexes line up across both forms.
        "(datetime(created_at) >= datetime(?2) AND ?3 IS NULL)"
    }
}

fn bind_boundary<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    boundary: &ChronicleBoundary,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    query
        .bind(sql_timestamp(boundary.at))
        .bind(boundary.message_id.clone())
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("2067")
        || db.message().contains("UNIQUE constraint failed"))
}

fn checkpoint_from_row(row: sqlx::sqlite::SqliteRow) -> ChronicleCheckpoint {
    let kind: String = row.try_get("kind").unwrap_or_else(|_| "interval".into());
    ChronicleCheckpoint {
        id: row.try_get("id").unwrap_or_default(),
        channel_id: row.try_get("channel_id").unwrap_or_default(),
        seq: row.try_get("seq").unwrap_or_default(),
        level: row.try_get("level").unwrap_or_default(),
        kind: CheckpointKind::from_db(&kind),
        title: row.try_get("title").unwrap_or_default(),
        summary: row.try_get("summary").unwrap_or_default(),
        covers_from_at: row.try_get("covers_from_at").unwrap_or_else(|_| Utc::now()),
        covers_to_at: row.try_get("covers_to_at").unwrap_or_else(|_| Utc::now()),
        covers_from_message_id: row.try_get("covers_from_message_id").ok().flatten(),
        covers_to_message_id: row.try_get("covers_to_message_id").ok().flatten(),
        message_count: row.try_get("message_count").unwrap_or_default(),
        token_estimate: row.try_get("token_estimate").unwrap_or_default(),
        rolled_up_into: row.try_get("rolled_up_into").ok().flatten(),
        rolls_up_from_seq: row.try_get("rolls_up_from_seq").ok().flatten(),
        rolls_up_to_seq: row.try_get("rolls_up_to_seq").ok().flatten(),
        model: row.try_get("model").ok().flatten(),
        created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
    }
}

fn message_from_row(row: sqlx::sqlite::SqliteRow) -> ConversationMessage {
    ConversationMessage {
        id: row.try_get("id").unwrap_or_default(),
        channel_id: row.try_get("channel_id").unwrap_or_default(),
        role: row.try_get("role").unwrap_or_default(),
        sender_name: row.try_get("sender_name").ok(),
        sender_id: row.try_get("sender_id").ok(),
        content: row.try_get("content").unwrap_or_default(),
        metadata: row.try_get("metadata").ok(),
        created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> ChronicleStore {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite memory pool");

        sqlx::raw_sql(include_str!(
            "../../migrations/20260211000002_conversations.sql"
        ))
        .execute(&pool)
        .await
        .expect("conversations migration");
        sqlx::raw_sql(include_str!(
            "../../migrations/20260809000004_session_chronicles.sql"
        ))
        .execute(&pool)
        .await
        .expect("chronicle migration");

        ChronicleStore::new(pool)
    }

    async fn insert_message(store: &ChronicleStore, channel: &str, id: &str, at: &str) {
        sqlx::query(
            "INSERT INTO conversation_messages (id, channel_id, role, content, created_at) \
             VALUES (?, ?, 'user', 'hello', ?)",
        )
        .bind(id)
        .bind(channel)
        .bind(at)
        .execute(&store.pool)
        .await
        .expect("insert message");
    }

    fn boundary(at: &str, id: Option<&str>) -> ChronicleBoundary {
        ChronicleBoundary::new(
            DateTime::parse_from_rfc3339(at)
                .unwrap()
                .with_timezone(&Utc),
            id.map(String::from),
        )
    }

    fn new_checkpoint(
        channel: &str,
        from: ChronicleBoundary,
        to: ChronicleBoundary,
        count: i64,
    ) -> NewCheckpoint {
        NewCheckpoint {
            channel_id: channel.to_string(),
            level: 0,
            kind: CheckpointKind::Interval,
            title: "a span".into(),
            summary: "things happened".into(),
            covers_from: from,
            covers_to: to,
            message_count: count,
            token_estimate: 10,
            rolls_up_from_seq: None,
            rolls_up_to_seq: None,
            model: Some("test-model".into()),
        }
    }

    #[tokio::test]
    async fn commits_allocate_sequential_boundaries_with_no_gap() {
        let store = setup().await;
        let first = store
            .commit(new_checkpoint(
                "ch",
                boundary("2026-08-01T00:00:00Z", None),
                boundary("2026-08-01T01:00:00Z", Some("m10")),
                10,
            ))
            .await
            .expect("commit");
        let CommitOutcome::Committed(first) = first else {
            panic!("first commit should succeed")
        };
        assert_eq!(first.seq, 1);

        let second = store
            .commit(new_checkpoint(
                "ch",
                first.end_boundary(),
                boundary("2026-08-01T02:00:00Z", Some("m20")),
                10,
            ))
            .await
            .expect("commit");
        let CommitOutcome::Committed(second) = second else {
            panic!("second commit should succeed")
        };

        assert_eq!(second.seq, 2);
        assert_eq!(
            second.covers_from_message_id, first.covers_to_message_id,
            "checkpoint N starts exactly where N-1 ended"
        );
    }

    #[tokio::test]
    async fn stale_cut_is_superseded_and_writes_nothing() {
        let store = setup().await;
        let CommitOutcome::Committed(first) = store
            .commit(new_checkpoint(
                "ch",
                boundary("2026-08-01T00:00:00Z", None),
                boundary("2026-08-01T01:00:00Z", Some("m10")),
                10,
            ))
            .await
            .expect("commit")
        else {
            panic!("expected commit")
        };

        // A cut that started before `first` landed still carries the old start.
        let outcome = store
            .commit(new_checkpoint(
                "ch",
                boundary("2026-08-01T00:00:00Z", None),
                boundary("2026-08-01T00:30:00Z", Some("m5")),
                5,
            ))
            .await
            .expect("commit call should not error");

        assert!(matches!(outcome, CommitOutcome::Superseded { .. }));
        let all = store.list("ch", 0, 10).await.expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, first.id);
    }

    #[tokio::test]
    async fn duplicate_commit_yields_one_row() {
        let store = setup().await;
        let cut = new_checkpoint(
            "ch",
            boundary("2026-08-01T00:00:00Z", None),
            boundary("2026-08-01T01:00:00Z", Some("m10")),
            10,
        );

        let first = store.commit(cut.clone()).await.expect("commit");
        assert!(matches!(first, CommitOutcome::Committed(_)));

        // A retry of the same cut no longer joins the tail it did before.
        let second = store.commit(cut).await.expect("commit");
        assert!(matches!(second, CommitOutcome::Superseded { .. }));
        assert_eq!(store.list("ch", 0, 10).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn boundary_selection_orders_same_second_messages_by_id() {
        let store = setup().await;
        // Three messages sharing a second: the ordering key has to break the
        // tie the same way for counting and for expansion.
        insert_message(&store, "ch", "m-a", "2026-08-01 00:00:00").await;
        insert_message(&store, "ch", "m-b", "2026-08-01 00:00:00").await;
        insert_message(&store, "ch", "m-c", "2026-08-01 00:00:00").await;
        insert_message(&store, "ch", "m-d", "2026-08-01 00:00:05").await;

        let after_b = boundary("2026-08-01T00:00:00Z", Some("m-b"));
        let count = store
            .count_messages_after("ch", &after_b)
            .await
            .expect("count");
        assert_eq!(count, 2, "m-c and m-d follow m-b");

        let messages = store
            .messages_after("ch", &after_b, 10)
            .await
            .expect("messages");
        let ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["m-c", "m-d"]);
    }

    #[tokio::test]
    async fn open_start_boundary_selects_everything() {
        let store = setup().await;
        insert_message(&store, "ch", "m-a", "2026-08-01 00:00:00").await;
        insert_message(&store, "ch", "m-b", "2026-08-01 00:00:01").await;

        let origin = ChronicleBoundary::origin(
            DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(
            store
                .count_messages_after("ch", &origin)
                .await
                .expect("count"),
            2
        );
    }

    #[tokio::test]
    async fn range_selection_is_half_open_and_contiguous() {
        let store = setup().await;
        for (index, id) in ["m1", "m2", "m3", "m4", "m5"].iter().enumerate() {
            insert_message(&store, "ch", id, &format!("2026-08-01 00:00:0{index}")).await;
        }

        let first_range = (
            ChronicleBoundary::origin(
                DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            boundary("2026-08-01T00:00:02Z", Some("m3")),
        );
        let second_range = (
            first_range.1.clone(),
            boundary("2026-08-01T00:00:04Z", Some("m5")),
        );

        let first = store
            .messages_in_range("ch", &first_range.0, &first_range.1, 100)
            .await
            .expect("range");
        let second = store
            .messages_in_range("ch", &second_range.0, &second_range.1, 100)
            .await
            .expect("range");

        let first_ids: Vec<&str> = first.iter().map(|m| m.id.as_str()).collect();
        let second_ids: Vec<&str> = second.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(first_ids, vec!["m1", "m2", "m3"]);
        assert_eq!(second_ids, vec!["m4", "m5"]);
        assert!(
            first_ids.iter().all(|id| !second_ids.contains(id)),
            "ranges must not overlap"
        );
    }

    #[tokio::test]
    async fn stats_report_unsummarized_tail() {
        let store = setup().await;
        for (index, id) in ["m1", "m2", "m3", "m4"].iter().enumerate() {
            insert_message(&store, "ch", id, &format!("2026-08-01 00:00:0{index}")).await;
        }

        let empty = store.stats("ch").await.expect("stats");
        assert_eq!(empty.checkpoint_count, 0);
        assert_eq!(empty.unsummarized_messages, 4);

        store
            .commit(new_checkpoint(
                "ch",
                ChronicleBoundary::origin(
                    DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                boundary("2026-08-01T00:00:02Z", Some("m3")),
                3,
            ))
            .await
            .expect("commit");

        let stats = store.stats("ch").await.expect("stats");
        assert_eq!(stats.checkpoint_count, 1);
        assert_eq!(stats.interval_count, 1);
        assert_eq!(stats.total_messages, 4);
        assert_eq!(stats.unsummarized_messages, 1);
    }
}
