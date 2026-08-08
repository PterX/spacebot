//! Per-skill provenance and usage tracking, backed by the agent's SQLite.
//!
//! Every skill gets a row in `skill_usage` recording who created it, how
//! often it's read and patched, and its lifecycle state. Curation only ever
//! operates on skills with `created_by = 'agent'` — user-authored and
//! registry-installed skills are outside curator jurisdiction unless the
//! user explicitly adopts them.

use sqlx::{Row as _, SqlitePool};

/// Who initiated a skill write. Set by the process constructing the tool
/// server, never supplied by the model. Autonomous writers (`Agent`) get a
/// narrower blast radius: they can't touch installed, pinned, or
/// instance-level skills, and their deletes archive instead of remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOrigin {
    /// A present human: channel conversations, API, CLI.
    User,
    /// An autonomous process: reflection branches, cortex curation.
    Agent,
}

/// A row from the `skill_usage` table.
#[derive(Debug, Clone)]
pub struct SkillUsageRecord {
    pub skill_name: String,
    pub created_by: String,
    pub origin_conversation_id: Option<String>,
    pub state: String,
    pub pinned: bool,
    pub read_count: i64,
    pub patch_count: i64,
    pub last_read_at: Option<String>,
    pub last_patched_at: Option<String>,
    pub created_at: String,
    pub archived_at: Option<String>,
}

/// Store for skill provenance and usage counters.
#[derive(Clone)]
pub struct SkillUsageStore {
    pool: SqlitePool,
}

impl std::fmt::Debug for SkillUsageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillUsageStore").finish_non_exhaustive()
    }
}

impl SkillUsageStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Seed rows for skills that don't have one yet.
    ///
    /// Seeded skills get `created_by = 'user'` and `created_at = now` — a
    /// newly noticed skill's staleness clock starts at discovery, not at
    /// epoch, and unattributed skills default to the origin that protects
    /// them from auto-curation.
    pub async fn seed(&self, names: &[String]) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        for name in names {
            sqlx::query(
                "INSERT OR IGNORE INTO skill_usage (skill_name, created_by, created_at) \
                 VALUES (?, 'user', ?)",
            )
            .bind(name.to_lowercase())
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Record a read: bump the counter, stamp the time, and reactivate a
    /// stale skill.
    pub async fn record_read(&self, name: &str) -> anyhow::Result<()> {
        let key = name.to_lowercase();
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT OR IGNORE INTO skill_usage (skill_name, created_by, created_at) \
             VALUES (?, 'user', ?)",
        )
        .bind(&key)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "UPDATE skill_usage SET \
                 read_count = read_count + 1, \
                 last_read_at = ?, \
                 state = CASE WHEN state = 'stale' THEN 'active' ELSE state END \
             WHERE skill_name = ?",
        )
        .bind(&now)
        .bind(&key)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Mark skills as registry-installed, creating rows as needed.
    ///
    /// An install that replaces an existing row also clears any agent
    /// conversation origin — the content on disk no longer comes from that
    /// conversation.
    pub async fn record_installed(&self, names: &[String]) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        for name in names {
            sqlx::query(
                "INSERT INTO skill_usage (skill_name, created_by, created_at) \
                 VALUES (?, 'installed', ?) \
                 ON CONFLICT(skill_name) DO UPDATE SET \
                     created_by = 'installed', \
                     origin_conversation_id = NULL",
            )
            .bind(name.to_lowercase())
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Record a skill created through `skill_manage`.
    ///
    /// `origin_conversation_id` is stored only for agent-created skills —
    /// it's the provenance link curation and the inspector surface later.
    pub async fn record_created(
        &self,
        name: &str,
        origin: WriteOrigin,
        conversation_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let created_by = match origin {
            WriteOrigin::User => "user",
            WriteOrigin::Agent => "agent",
        };
        let origin_conversation = match origin {
            WriteOrigin::Agent => conversation_id,
            WriteOrigin::User => None,
        };

        sqlx::query(
            "INSERT INTO skill_usage (skill_name, created_by, origin_conversation_id, created_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(skill_name) DO UPDATE SET \
                 created_by = excluded.created_by, \
                 origin_conversation_id = excluded.origin_conversation_id, \
                 state = 'active', \
                 archived_at = NULL",
        )
        .bind(name.to_lowercase())
        .bind(created_by)
        .bind(origin_conversation)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record a patch or edit: bump the counter and stamp the time.
    pub async fn record_patch(&self, name: &str) -> anyhow::Result<()> {
        let key = name.to_lowercase();
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT OR IGNORE INTO skill_usage (skill_name, created_by, created_at) \
             VALUES (?, 'user', ?)",
        )
        .bind(&key)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "UPDATE skill_usage SET \
                 patch_count = patch_count + 1, \
                 last_patched_at = ?, \
                 state = CASE WHEN state = 'stale' THEN 'active' ELSE state END \
             WHERE skill_name = ?",
        )
        .bind(&now)
        .bind(&key)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Set or clear the pin flag.
    pub async fn set_pinned(&self, name: &str, pinned: bool) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO skill_usage (skill_name, created_by, pinned, created_at) \
             VALUES (?, 'user', ?, ?) \
             ON CONFLICT(skill_name) DO UPDATE SET pinned = excluded.pinned",
        )
        .bind(name.to_lowercase())
        .bind(pinned as i64)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Hand a skill to curation: flip `created_by` to 'agent'.
    ///
    /// This is the explicit user act that puts a skill inside curator
    /// jurisdiction; it is never done automatically.
    pub async fn adopt(&self, name: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO skill_usage (skill_name, created_by, created_at) \
             VALUES (?, 'agent', ?) \
             ON CONFLICT(skill_name) DO UPDATE SET created_by = 'agent'",
        )
        .bind(name.to_lowercase())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Mark a skill archived.
    pub async fn set_archived(&self, name: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO skill_usage (skill_name, created_by, state, archived_at, created_at) \
             VALUES (?, 'user', 'archived', ?, ?) \
             ON CONFLICT(skill_name) DO UPDATE SET \
                 state = 'archived', \
                 archived_at = excluded.archived_at",
        )
        .bind(name.to_lowercase())
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Mark an archived skill active again.
    pub async fn set_restored(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE skill_usage SET state = 'active', archived_at = NULL WHERE skill_name = ?",
        )
        .bind(name.to_lowercase())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Drop the row for a removed skill.
    pub async fn remove(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM skill_usage WHERE skill_name = ?")
            .bind(name.to_lowercase())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fetch a single skill's usage record.
    pub async fn get(&self, name: &str) -> anyhow::Result<Option<SkillUsageRecord>> {
        let row = sqlx::query("SELECT * FROM skill_usage WHERE skill_name = ?")
            .bind(name.to_lowercase())
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(record_from_row))
    }

    /// List all usage records, ordered by skill name.
    pub async fn list(&self) -> anyhow::Result<Vec<SkillUsageRecord>> {
        let rows = sqlx::query("SELECT * FROM skill_usage ORDER BY skill_name")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows.into_iter().map(record_from_row).collect())
    }
}

fn record_from_row(row: sqlx::sqlite::SqliteRow) -> SkillUsageRecord {
    SkillUsageRecord {
        skill_name: row.get("skill_name"),
        created_by: row.get("created_by"),
        origin_conversation_id: row.get("origin_conversation_id"),
        state: row.get("state"),
        pinned: row.get::<i64, _>("pinned") != 0,
        read_count: row.get("read_count"),
        patch_count: row.get("patch_count"),
        last_read_at: row.get("last_read_at"),
        last_patched_at: row.get("last_patched_at"),
        created_at: row.get("created_at"),
        archived_at: row.get("archived_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> SkillUsageStore {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        SkillUsageStore::new(pool)
    }

    #[tokio::test]
    async fn seed_is_idempotent_and_preserves_existing_rows() {
        let store = test_store().await;

        store.seed(&["Weather".to_string()]).await.unwrap();
        store.record_read("weather").await.unwrap();
        store.seed(&["weather".to_string()]).await.unwrap();

        let record = store.get("WEATHER").await.unwrap().unwrap();
        assert_eq!(record.skill_name, "weather");
        assert_eq!(record.created_by, "user");
        assert_eq!(record.read_count, 1);
    }

    #[tokio::test]
    async fn record_read_seeds_missing_row_and_reactivates_stale() {
        let store = test_store().await;

        store.record_read("deploy").await.unwrap();
        let record = store.get("deploy").await.unwrap().unwrap();
        assert_eq!(record.read_count, 1);
        assert!(record.last_read_at.is_some());

        sqlx::query("UPDATE skill_usage SET state = 'stale' WHERE skill_name = 'deploy'")
            .execute(&store.pool)
            .await
            .unwrap();

        store.record_read("deploy").await.unwrap();
        let record = store.get("deploy").await.unwrap().unwrap();
        assert_eq!(record.state, "active");
        assert_eq!(record.read_count, 2);
    }

    #[tokio::test]
    async fn record_installed_overrides_seeded_provenance() {
        let store = test_store().await;

        store.seed(&["github".to_string()]).await.unwrap();
        store
            .record_installed(&["github".to_string()])
            .await
            .unwrap();

        let record = store.get("github").await.unwrap().unwrap();
        assert_eq!(record.created_by, "installed");
    }

    #[tokio::test]
    async fn record_installed_clears_agent_conversation_origin() {
        let store = test_store().await;

        sqlx::query(
            "INSERT INTO skill_usage (skill_name, created_by, origin_conversation_id, created_at) \
             VALUES ('deploy', 'agent', 'conv-123', '2026-08-08T00:00:00Z')",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        store
            .record_installed(&["deploy".to_string()])
            .await
            .unwrap();

        let record = store.get("deploy").await.unwrap().unwrap();
        assert_eq!(record.created_by, "installed");
        assert!(record.origin_conversation_id.is_none());
    }

    #[tokio::test]
    async fn remove_drops_the_row() {
        let store = test_store().await;

        store.seed(&["temp".to_string()]).await.unwrap();
        store.remove("temp").await.unwrap();
        assert!(store.get("temp").await.unwrap().is_none());
        assert!(store.list().await.unwrap().is_empty());
    }
}
