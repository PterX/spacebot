//! `[[agents.X.wakes]]` config entries and their reconciliation into the
//! wake definition store.
//!
//! Config is a seed, the database is the source of truth — the same
//! relationship cron has. Config-owned rows are upserted by id on load
//! (preserving live schedule cursors), rows whose config entry is gone are
//! deleted, and builtin or user-owned rows are never touched.

use super::defs::{WakeDef, WakeDefStore, WakeTrigger};
use crate::config::AutonomyLevel;
use crate::error::{ConfigError, Result};
use crate::wakes::SystemEvent;

use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// A `[[agents.X.wakes]]` entry. Exactly one trigger must be set: `schedule`
/// and/or `interval_secs` (schedule trigger), `webhook = true`, or `event`.
#[derive(Debug, Clone, Deserialize)]
pub struct WakeConfig {
    pub id: String,
    pub name: String,
    /// Standard 5-field cron expression.
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub interval_secs: Option<u64>,
    #[serde(default)]
    pub webhook: Option<bool>,
    /// System event name, e.g. "task.approved".
    #[serde(default)]
    pub event: Option<String>,
    pub instructions: String,
    #[serde(default = "default_min_level")]
    pub min_level: AutonomyLevel,
    #[serde(default = "default_wake_enabled")]
    pub enabled: bool,
    /// (start_hour, end_hour) in the agent's cron timezone.
    #[serde(default)]
    pub active_hours: Option<(u8, u8)>,
    /// Delivery target in "adapter:target" format.
    #[serde(default)]
    pub delivery_target: Option<String>,
}

fn default_min_level() -> AutonomyLevel {
    AutonomyLevel::Observe
}

fn default_wake_enabled() -> bool {
    true
}

impl WakeConfig {
    /// Validate this entry and produce its trigger. `scope` names the config
    /// path (e.g. "agents.main.wakes.morning-brief") so load errors point at
    /// the offending wake.
    pub fn validated(&self, scope: &str) -> Result<WakeTrigger> {
        let has_schedule = self.schedule.is_some() || self.interval_secs.is_some();
        let has_webhook = self.webhook == Some(true);
        let has_event = self.event.is_some();
        let trigger_count = [has_schedule, has_webhook, has_event]
            .iter()
            .filter(|set| **set)
            .count();
        if trigger_count != 1 {
            return Err(ConfigError::Invalid(format!(
                "{scope} must set exactly one trigger: schedule/interval_secs, webhook = true, or event"
            ))
            .into());
        }

        if let Some((start, end)) = self.active_hours
            && (start > 23 || end > 23)
        {
            return Err(ConfigError::Invalid(format!(
                "{scope}.active_hours hours must be 0-23, got [{start}, {end}]"
            ))
            .into());
        }

        if let Some(target) = self.delivery_target.as_deref()
            && crate::messaging::target::parse_delivery_target(target).is_none()
        {
            return Err(ConfigError::Invalid(format!(
                "{scope}.delivery_target '{target}' is invalid: expected format 'adapter:target'"
            ))
            .into());
        }

        if let Some(name) = self.event.as_deref() {
            let event = SystemEvent::parse(name).ok_or_else(|| {
                ConfigError::Invalid(format!("{scope}.event unknown event name '{name}'"))
            })?;
            return Ok(WakeTrigger::Event { event });
        }

        if has_webhook {
            return Ok(WakeTrigger::Webhook);
        }

        if let Some(interval) = self.interval_secs
            && interval < 60
        {
            return Err(ConfigError::Invalid(format!(
                "{scope}.interval_secs must be >= 60, got {interval}"
            ))
            .into());
        }
        let cron_expr = crate::cron::scheduler::normalize_cron_expr(self.schedule.clone())
            .map_err(|error| ConfigError::Invalid(format!("{scope}.schedule: {error}")))?;
        if cron_expr.is_none() && self.interval_secs.is_none() {
            return Err(ConfigError::Invalid(format!(
                "{scope}.schedule must be a non-empty 5-field cron expression"
            ))
            .into());
        }

        Ok(WakeTrigger::Schedule {
            cron_expr,
            interval_secs: self.interval_secs,
        })
    }

    fn to_def(&self, trigger: WakeTrigger) -> WakeDef {
        WakeDef {
            id: self.id.clone(),
            name: self.name.clone(),
            trigger,
            instructions: self.instructions.clone(),
            min_level: self.min_level,
            enabled: self.enabled,
            builtin: false,
            config_owned: true,
            delivery_target: self.delivery_target.clone(),
            webhook_token: None,
            active_hours: self.active_hours,
            next_run_at: None,
            last_fired_at: None,
            consecutive_failures: 0,
            created_by: "config".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

/// Reconcile `[[agents.X.wakes]]` entries into the store: upsert config-owned
/// rows by id (schedule cursors survive), delete config-owned rows whose
/// entry is gone, and never touch builtin or user-owned rows. Per-row
/// failures — id collisions with non-config rows, invalid entries, and store
/// errors alike — are logged and skipped so one bad row does not abort the
/// rest of the reconciliation, the same tolerance cron config seeding has.
/// Errors out only when the existing rows cannot be listed at all.
pub async fn reconcile_config_wakes(store: &WakeDefStore, configs: &[WakeConfig]) -> Result<()> {
    let existing = store.list().await?;
    let existing_by_id: HashMap<&str, &WakeDef> =
        existing.iter().map(|def| (def.id.as_str(), def)).collect();

    let mut config_ids: HashSet<&str> = HashSet::new();
    for config in configs {
        config_ids.insert(config.id.as_str());

        if let Some(current) = existing_by_id.get(config.id.as_str())
            && !current.config_owned
        {
            tracing::warn!(
                wake_id = %config.id,
                builtin = current.builtin,
                "config wake id collides with a non-config wake, skipping"
            );
            continue;
        }

        let scope = format!("wakes.{}", config.id);
        let trigger = match config.validated(&scope) {
            Ok(trigger) => trigger,
            Err(error) => {
                tracing::warn!(wake_id = %config.id, %error, "invalid config wake, skipping");
                continue;
            }
        };
        if let Err(error) = store.upsert(&config.to_def(trigger)).await {
            tracing::warn!(wake_id = %config.id, %error, "failed to upsert config wake, skipping");
        }
    }

    for def in &existing {
        if def.config_owned && !def.builtin && !config_ids.contains(def.id.as_str()) {
            if let Err(error) = store.delete(&def.id).await {
                tracing::warn!(
                    wake_id = %def.id,
                    %error,
                    "failed to delete removed config wake, skipping"
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wakes::defs::{TASK_APPROVED_WAKE_ID, seed_builtin_wakes};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn store() -> WakeDefStore {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        WakeDefStore::new(pool)
    }

    fn config(id: &str) -> WakeConfig {
        WakeConfig {
            id: id.to_string(),
            name: format!("{id} name"),
            schedule: None,
            interval_secs: None,
            webhook: None,
            event: Some("task.approved".to_string()),
            instructions: format!("{id} instructions"),
            min_level: AutonomyLevel::Observe,
            enabled: true,
            active_hours: None,
            delivery_target: None,
        }
    }

    #[test]
    fn requires_exactly_one_trigger() {
        let mut none = config("w");
        none.event = None;
        assert!(none.validated("wakes.w").is_err());

        let mut two = config("w");
        two.schedule = Some("0 8 * * *".to_string());
        assert!(two.validated("wakes.w").is_err());

        assert!(config("w").validated("wakes.w").is_ok());
    }

    #[test]
    fn rejects_unknown_event_names() {
        let mut wake = config("w");
        wake.event = Some("task.deleted".to_string());
        let error = wake.validated("wakes.w").expect_err("unknown event");
        assert!(error.to_string().contains("task.deleted"));
    }

    #[test]
    fn rejects_malformed_cron_and_short_intervals() {
        let mut bad_expr = config("w");
        bad_expr.event = None;
        bad_expr.schedule = Some("not a cron".to_string());
        assert!(bad_expr.validated("wakes.w").is_err());

        let mut short = config("w");
        short.event = None;
        short.interval_secs = Some(30);
        assert!(short.validated("wakes.w").is_err());

        let mut valid = config("w");
        valid.event = None;
        valid.schedule = Some("0 8 * * *".to_string());
        assert_eq!(
            valid.validated("wakes.w").expect("valid"),
            WakeTrigger::Schedule {
                cron_expr: Some("0 8 * * *".to_string()),
                interval_secs: None,
            }
        );
    }

    #[test]
    fn rejects_bad_delivery_targets_and_active_hours() {
        let mut bad_target = config("w");
        bad_target.delivery_target = Some("no-colon".to_string());
        assert!(bad_target.validated("wakes.w").is_err());

        let mut bad_hours = config("w");
        bad_hours.active_hours = Some((8, 24));
        assert!(bad_hours.validated("wakes.w").is_err());
    }

    #[test]
    fn webhook_trigger_requires_flag_true() {
        let mut hook = config("w");
        hook.event = None;
        hook.webhook = Some(true);
        assert_eq!(
            hook.validated("wakes.w").expect("valid"),
            WakeTrigger::Webhook
        );

        // webhook = false does not count as a trigger.
        let mut off = config("w");
        off.event = None;
        off.webhook = Some(false);
        assert!(off.validated("wakes.w").is_err());
    }

    #[tokio::test]
    async fn reconcile_upserts_and_removes_config_rows() {
        let store = store().await;
        reconcile_config_wakes(&store, &[config("a"), config("b")])
            .await
            .expect("reconcile");

        let a = store.get("a").await.expect("get").expect("row");
        assert!(a.config_owned);
        assert!(!a.builtin);
        assert_eq!(a.created_by, "config");

        // Drop "b" from config: its row goes away, "a" stays.
        reconcile_config_wakes(&store, &[config("a")])
            .await
            .expect("reconcile");
        assert!(store.get("a").await.expect("get").is_some());
        assert!(store.get("b").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn reconcile_preserves_schedule_cursors() {
        let store = store().await;
        let mut scheduled = config("sched");
        scheduled.event = None;
        scheduled.interval_secs = Some(600);
        reconcile_config_wakes(&store, std::slice::from_ref(&scheduled))
            .await
            .expect("reconcile");
        assert!(
            store
                .claim_schedule_fire("sched", None, "2026-08-09T10:00:00.000Z")
                .await
                .expect("claim")
        );

        scheduled.instructions = "updated".to_string();
        reconcile_config_wakes(&store, &[scheduled])
            .await
            .expect("re-reconcile");

        let loaded = store.get("sched").await.expect("get").expect("row");
        assert_eq!(loaded.instructions, "updated");
        assert_eq!(
            loaded.next_run_at.as_deref(),
            Some("2026-08-09T10:00:00.000Z")
        );
    }

    #[tokio::test]
    async fn reconcile_never_touches_builtin_or_user_rows() {
        let store = store().await;
        seed_builtin_wakes(&store).await.expect("seed");
        let user_wake = {
            let mut def = config("user-wake").to_def(WakeTrigger::Webhook);
            def.config_owned = false;
            def.created_by = "user".to_string();
            def
        };
        store.upsert(&user_wake).await.expect("upsert user wake");

        // A config entry colliding with the builtin id is skipped.
        let mut collision = config(TASK_APPROVED_WAKE_ID);
        collision.instructions = "clobbered".to_string();
        reconcile_config_wakes(&store, &[collision])
            .await
            .expect("reconcile");
        let builtin = store
            .get(TASK_APPROVED_WAKE_ID)
            .await
            .expect("get")
            .expect("row");
        assert!(builtin.builtin);
        assert_ne!(builtin.instructions, "clobbered");

        // An empty config list deletes neither builtin nor user-owned rows.
        reconcile_config_wakes(&store, &[])
            .await
            .expect("reconcile");
        assert!(
            store
                .get(TASK_APPROVED_WAKE_ID)
                .await
                .expect("get")
                .is_some()
        );
        assert!(store.get("user-wake").await.expect("get").is_some());
    }
}
