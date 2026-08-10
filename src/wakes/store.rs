//! Persisted wake-event queue.
//!
//! Producers enqueue before ringing the wake doorbell; the autonomy channel
//! drains pending rows at the start of a run and marks them consumed with a
//! CAS-guarded update, the same idiom as cron cursor claiming. Durability
//! comes from this table; liveness comes from the doorbell; a missed ring
//! degrades to the interval poll.

use crate::error::Result;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row as _, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WakeEvent {
    pub id: String,
    pub wake_id: String,
    pub dedupe_key: String,
    pub payload: Value,
    pub fired_at: String,
    pub delivery_count: i64,
    pub consumed_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Inserted,
    Coalesced,
}

#[derive(Debug, Clone)]
pub struct WakeEventStore {
    pool: SqlitePool,
}

impl WakeEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Enqueue a wake event. An arrival matching a pending (wake_id,
    /// dedupe_key) pair coalesces into the existing row: delivery_count is
    /// bumped and the payload is replaced with the latest arrival's.
    pub async fn enqueue(
        &self,
        wake_id: &str,
        dedupe_key: &str,
        payload: &Value,
    ) -> Result<EnqueueOutcome> {
        let id = uuid::Uuid::new_v4().to_string();
        // RETURNING yields the surviving row's id: ours on insert, the
        // existing pending row's on coalesce.
        let stored_id: String = sqlx::query_scalar(
            "INSERT INTO wake_events (id, wake_id, dedupe_key, payload) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(wake_id, dedupe_key) WHERE consumed_by IS NULL \
             DO UPDATE SET delivery_count = delivery_count + 1, \
                           payload = excluded.payload \
             RETURNING id",
        )
        .bind(&id)
        .bind(wake_id)
        .bind(dedupe_key)
        .bind(payload.to_string())
        .fetch_one(&self.pool)
        .await
        .context("failed to enqueue wake event")?;

        Ok(if stored_id == id {
            EnqueueOutcome::Inserted
        } else {
            EnqueueOutcome::Coalesced
        })
    }

    /// Pending events in arrival order.
    pub async fn pending(&self, limit: i64) -> Result<Vec<WakeEvent>> {
        let rows = sqlx::query(
            "SELECT id, wake_id, dedupe_key, payload, fired_at, delivery_count, consumed_by \
             FROM wake_events WHERE consumed_by IS NULL \
             ORDER BY fired_at ASC, id ASC LIMIT ?",
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await
        .context("failed to list pending wake events")?;

        rows.into_iter().map(event_from_row).collect()
    }

    pub async fn pending_count(&self) -> Result<i64> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM wake_events WHERE consumed_by IS NULL")
            .fetch_one(&self.pool)
            .await
            .context("failed to count pending wake events")
            .map_err(Into::into)
    }

    /// Mark events consumed by a run. Guarded on `consumed_by IS NULL` so a
    /// concurrent consumer cannot double-claim; returns how many this caller
    /// actually claimed.
    pub async fn consume(&self, event_ids: &[String], run_id: &str) -> Result<u64> {
        if event_ids.is_empty() {
            return Ok(0);
        }

        let placeholders = vec!["?"; event_ids.len()].join(", ");
        let query = format!(
            "UPDATE wake_events SET consumed_by = ? \
             WHERE consumed_by IS NULL AND id IN ({placeholders})"
        );

        let mut sql = sqlx::query(&query).bind(run_id);
        for id in event_ids {
            sql = sql.bind(id);
        }

        let result = sql
            .execute(&self.pool)
            .await
            .context("failed to consume wake events")?;

        Ok(result.rows_affected())
    }

    /// Delete consumed events older than the retention window.
    pub async fn prune_consumed(&self, retention_days: u32) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM wake_events WHERE consumed_by IS NOT NULL \
             AND fired_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?)",
        )
        .bind(format!("-{retention_days} days"))
        .execute(&self.pool)
        .await
        .context("failed to prune wake events")?;

        Ok(result.rows_affected())
    }
}

fn event_from_row(row: sqlx::sqlite::SqliteRow) -> Result<WakeEvent> {
    let payload_text: String = row.try_get("payload").unwrap_or_else(|_| "{}".to_string());
    Ok(WakeEvent {
        id: row.try_get("id").context("failed to read wake event id")?,
        wake_id: row
            .try_get("wake_id")
            .context("failed to read wake event wake_id")?,
        dedupe_key: row
            .try_get("dedupe_key")
            .context("failed to read wake event dedupe_key")?,
        payload: serde_json::from_str(&payload_text)
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        fired_at: row
            .try_get("fired_at")
            .context("failed to read wake event fired_at")?,
        delivery_count: row
            .try_get("delivery_count")
            .context("failed to read wake event delivery_count")?,
        consumed_by: row.try_get("consumed_by").ok().flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn store() -> WakeEventStore {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        WakeEventStore::new(pool)
    }

    #[tokio::test]
    async fn enqueue_and_drain() {
        let store = store().await;

        let outcome = store
            .enqueue(
                "ci-failed",
                "run-123",
                &serde_json::json!({"job": "clippy"}),
            )
            .await
            .expect("enqueue");
        assert_eq!(outcome, EnqueueOutcome::Inserted);

        let pending = store.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].wake_id, "ci-failed");
        assert_eq!(pending[0].delivery_count, 1);

        let claimed = store
            .consume(&[pending[0].id.clone()], "run-abc")
            .await
            .expect("consume");
        assert_eq!(claimed, 1);
        assert_eq!(store.pending_count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn coalesces_pending_duplicates() {
        let store = store().await;

        store
            .enqueue("ci-failed", "main", &serde_json::json!({"attempt": 1}))
            .await
            .expect("first");
        let outcome = store
            .enqueue("ci-failed", "main", &serde_json::json!({"attempt": 2}))
            .await
            .expect("second");
        assert_eq!(outcome, EnqueueOutcome::Coalesced);

        let pending = store.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_count, 2);
        assert_eq!(pending[0].payload["attempt"], 2);
    }

    #[tokio::test]
    async fn consumed_events_do_not_block_new_arrivals() {
        let store = store().await;

        store
            .enqueue("ci-failed", "main", &serde_json::json!({}))
            .await
            .expect("first");
        let pending = store.pending(10).await.expect("pending");
        store
            .consume(&[pending[0].id.clone()], "run-1")
            .await
            .expect("consume");

        // Same dedupe key again after consumption: a fresh pending row, not a
        // coalesce into the consumed one.
        let outcome = store
            .enqueue("ci-failed", "main", &serde_json::json!({}))
            .await
            .expect("second");
        assert_eq!(outcome, EnqueueOutcome::Inserted);
        assert_eq!(store.pending_count().await.expect("count"), 1);
    }

    #[tokio::test]
    async fn consume_is_single_claimer() {
        let store = store().await;

        store
            .enqueue("survey", "", &serde_json::json!({}))
            .await
            .expect("enqueue");
        let pending = store.pending(10).await.expect("pending");
        let ids: Vec<String> = pending.iter().map(|e| e.id.clone()).collect();

        let first = store.consume(&ids, "run-1").await.expect("first claim");
        let second = store.consume(&ids, "run-2").await.expect("second claim");
        assert_eq!(first, 1);
        assert_eq!(second, 0);
    }
}
