//! Schedule wake producer.
//!
//! Runs on the cortex tick and fires due schedule-trigger wake definitions:
//! claims each due cursor with the same CAS idiom as cron's
//! `claim_and_advance`, enqueues one wake event per claimed fire, and rings
//! the wake doorbell once so the autonomy check in the same tick sees the
//! fresh events. Schedule resolution is bounded by the tick cadence.

use crate::AgentDeps;
use crate::config::RuntimeConfig;
use crate::cron::scheduler::{current_hour_and_timezone, hour_in_active_window};
use crate::schedule::ScheduleSpec;
use crate::wakes::{WakeDef, WakeDefStore, WakeEventStore, WakeTrigger, parse_run_timestamp};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// Fire all due schedule wakes. Never errors outward — production is
/// best-effort per tick and any failure degrades to the next pass.
pub async fn fire_due_schedule_wakes(deps: &AgentDeps) {
    let enqueued = run_schedule_pass(
        &deps.wake_def_store,
        &deps.wake_event_store,
        &deps.runtime_config,
        Utc::now(),
    )
    .await;

    if enqueued && let Some(wake_tx) = &deps.wake_tx {
        crate::agent::wake::fire_wake(wake_tx, &deps.agent_id);
    }
}

/// One producer pass over the due set. Returns whether any event was enqueued.
async fn run_schedule_pass(
    defs: &WakeDefStore,
    events: &WakeEventStore,
    runtime_config: &RuntimeConfig,
    now: DateTime<Utc>,
) -> bool {
    let due = match defs.due_schedule_wakes(&format_cursor(now)).await {
        Ok(due) => due,
        Err(error) => {
            tracing::warn!(%error, "failed to load due schedule wakes");
            return false;
        }
    };

    let mut enqueued_any = false;
    for def in due {
        if fire_schedule_wake(defs, events, runtime_config, &def, now).await {
            enqueued_any = true;
        }
    }

    enqueued_any
}

/// Process a single due definition. Returns whether an event was enqueued.
async fn fire_schedule_wake(
    defs: &WakeDefStore,
    events: &WakeEventStore,
    runtime_config: &RuntimeConfig,
    def: &WakeDef,
    now: DateTime<Utc>,
) -> bool {
    let WakeTrigger::Schedule {
        cron_expr,
        interval_secs,
    } = &def.trigger
    else {
        return false;
    };
    let spec = ScheduleSpec {
        cron_expr: cron_expr.clone(),
        interval_secs: *interval_secs,
    };

    let Some(cursor) = def.next_run_at.as_deref() else {
        // Cursor initialization: anchor the schedule without enqueueing so a
        // fresh or newly created definition does not fire at startup.
        let Some(next) = next_occurrence(&spec, now, runtime_config) else {
            tracing::warn!(wake_id = %def.id, "schedule wake has no computable next occurrence");
            return false;
        };
        if let Err(error) = defs
            .claim_schedule_fire(&def.id, None, &format_cursor(next))
            .await
        {
            tracing::warn!(wake_id = %def.id, %error, "failed to initialize schedule wake cursor");
        }
        return false;
    };

    let Some(scheduled_for) = parse_run_timestamp(cursor) else {
        // A cursor that cannot be parsed cannot anchor the next occurrence;
        // repair it forward from now without firing.
        tracing::warn!(wake_id = %def.id, cursor, "unparseable schedule wake cursor, repairing");
        if let Some(next) = next_occurrence(&spec, now, runtime_config)
            && let Err(error) = defs
                .claim_schedule_fire(&def.id, Some(cursor), &format_cursor(next))
                .await
        {
            tracing::warn!(wake_id = %def.id, %error, "failed to repair schedule wake cursor");
        }
        return false;
    };

    // Anchor the next occurrence to the due cursor so the cadence holds, and
    // fast-forward past a stale backlog so a long outage advances to the next
    // future occurrence instead of draining one missed fire per tick.
    let next = next_occurrence(&spec, scheduled_for, runtime_config)
        .filter(|next| *next > now)
        .or_else(|| next_occurrence(&spec, now, runtime_config));
    let Some(next) = next else {
        tracing::warn!(wake_id = %def.id, "schedule wake has no computable next occurrence");
        return false;
    };
    let next_text = format_cursor(next);

    // Outside the active-hours window the occurrence is skipped and the
    // cursor still advances, matching cron's active-hours suppression.
    if let Some((start, end)) = def.active_hours {
        let (current_hour, timezone) = current_hour_and_timezone(runtime_config);
        if !hour_in_active_window(current_hour, start, end) {
            tracing::debug!(
                wake_id = %def.id,
                %timezone,
                current_hour,
                start,
                end,
                "outside active hours, skipping schedule wake occurrence"
            );
            if let Err(error) = defs
                .claim_schedule_fire(&def.id, Some(cursor), &next_text)
                .await
            {
                tracing::warn!(
                    wake_id = %def.id,
                    %error,
                    "failed to advance skipped schedule wake cursor"
                );
            }
            return false;
        }
    }

    // The CAS makes concurrent passes single-fire: the loser sees a moved
    // cursor and enqueues nothing.
    let claimed = match defs
        .claim_schedule_fire(&def.id, Some(cursor), &next_text)
        .await
    {
        Ok(claimed) => claimed,
        Err(error) => {
            tracing::warn!(wake_id = %def.id, %error, "failed to claim schedule wake fire");
            return false;
        }
    };
    if !claimed {
        return false;
    }

    // Empty dedupe key: a backlog of missed schedule ticks coalesces into one
    // pending row rather than stacking a run per missed occurrence.
    let payload = serde_json::json!({ "scheduled_for": cursor });
    match events.enqueue(&def.id, "", &payload).await {
        Ok(_) => {
            if let Err(error) = defs.touch_last_fired(&def.id).await {
                tracing::warn!(wake_id = %def.id, %error, "failed to touch schedule wake last_fired_at");
            }
            true
        }
        Err(error) => {
            tracing::warn!(wake_id = %def.id, %error, "failed to enqueue schedule wake event");
            false
        }
    }
}

/// Next occurrence in the agent's configured cron timezone, falling back to
/// the system timezone the same way cron's scheduler does.
fn next_occurrence(
    spec: &ScheduleSpec,
    after: DateTime<Utc>,
    runtime_config: &RuntimeConfig,
) -> Option<DateTime<Utc>> {
    let timezone = runtime_config.cron_timezone.load();
    match timezone.as_deref().and_then(|name| name.parse::<Tz>().ok()) {
        Some(timezone) => spec.next_occurrence(after, timezone),
        None => spec.next_occurrence_in(after, &chrono::Local),
    }
}

/// Format a cursor timestamp in the store's `%Y-%m-%dT%H:%M:%fZ` shape so
/// lexicographic comparison in `due_schedule_wakes` matches time order.
fn format_cursor(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutonomyLevel;
    use crate::wakes::WakeDef;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::Arc;

    struct Harness {
        _dir: tempfile::TempDir,
        defs: WakeDefStore,
        events: WakeEventStore,
        runtime_config: Arc<RuntimeConfig>,
    }

    async fn harness() -> Harness {
        let dir = tempfile::tempdir().expect("tempdir");
        let instance_dir = dir.path().join("instance");
        let agent = crate::config::AgentConfig {
            id: "test-agent".to_string(),
            ..Default::default()
        };
        let defaults = crate::config::DefaultsConfig::default();
        let resolved = agent.resolve(&instance_dir, &defaults);
        let runtime_config = Arc::new(RuntimeConfig::new(
            &instance_dir,
            &resolved,
            &defaults,
            crate::prompts::PromptEngine::new("en").expect("prompts"),
            crate::identity::Identity::default(),
            crate::skills::SkillSet::default(),
        ));
        // Pin the timezone so occurrence math is deterministic regardless of
        // the host machine's locale.
        runtime_config
            .cron_timezone
            .store(Arc::new(Some("UTC".to_string())));

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");

        Harness {
            _dir: dir,
            defs: WakeDefStore::new(pool.clone()),
            events: WakeEventStore::new(pool),
            runtime_config,
        }
    }

    fn schedule_def(id: &str, interval_secs: u64) -> WakeDef {
        WakeDef {
            id: id.to_string(),
            name: format!("{id} name"),
            trigger: WakeTrigger::Schedule {
                cron_expr: None,
                interval_secs: Some(interval_secs),
            },
            instructions: format!("{id} instructions"),
            min_level: AutonomyLevel::Suggest,
            enabled: true,
            builtin: false,
            config_owned: false,
            delivery_target: None,
            webhook_token: None,
            active_hours: None,
            next_run_at: None,
            last_fired_at: None,
            consecutive_failures: 0,
            created_by: "user".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn initialization_pass_sets_cursor_without_enqueueing() {
        let h = harness().await;
        h.defs
            .upsert(&schedule_def("sched", 600))
            .await
            .expect("upsert");

        let now = Utc::now();
        let enqueued = run_schedule_pass(&h.defs, &h.events, &h.runtime_config, now).await;

        assert!(!enqueued);
        assert_eq!(h.events.pending_count().await.expect("count"), 0);
        let def = h.defs.get("sched").await.expect("get").expect("row");
        let cursor = parse_run_timestamp(def.next_run_at.as_deref().expect("cursor set"))
            .expect("cursor parses");
        assert!(cursor > now);
    }

    #[tokio::test]
    async fn due_wake_enqueues_once_and_advances_cursor() {
        let h = harness().await;
        h.defs
            .upsert(&schedule_def("sched", 600))
            .await
            .expect("upsert");
        let due_at = "2026-08-09T10:00:00.000Z";
        assert!(
            h.defs
                .claim_schedule_fire("sched", None, due_at)
                .await
                .expect("seed cursor")
        );

        let now = Utc::now();
        let enqueued = run_schedule_pass(&h.defs, &h.events, &h.runtime_config, now).await;

        assert!(enqueued);
        let pending = h.events.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].wake_id, "sched");
        assert_eq!(pending[0].dedupe_key, "");
        assert_eq!(pending[0].payload["scheduled_for"], due_at);

        let def = h.defs.get("sched").await.expect("get").expect("row");
        let cursor = parse_run_timestamp(def.next_run_at.as_deref().expect("cursor"))
            .expect("cursor parses");
        // The stale cursor fast-forwards to the next occurrence after now.
        assert!(cursor > now);
        assert!(def.last_fired_at.is_some());
    }

    #[tokio::test]
    async fn second_pass_same_tick_is_a_no_op() {
        let h = harness().await;
        h.defs
            .upsert(&schedule_def("sched", 600))
            .await
            .expect("upsert");
        assert!(
            h.defs
                .claim_schedule_fire("sched", None, "2026-08-09T10:00:00.000Z")
                .await
                .expect("seed cursor")
        );

        let now = Utc::now();
        assert!(run_schedule_pass(&h.defs, &h.events, &h.runtime_config, now).await);
        assert!(!run_schedule_pass(&h.defs, &h.events, &h.runtime_config, now).await);

        let pending = h.events.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_count, 1);
    }

    #[tokio::test]
    async fn cas_loser_does_not_enqueue() {
        let h = harness().await;
        h.defs
            .upsert(&schedule_def("sched", 600))
            .await
            .expect("upsert");
        assert!(
            h.defs
                .claim_schedule_fire("sched", None, "2026-08-09T10:00:00.000Z")
                .await
                .expect("seed cursor")
        );
        let stale = h.defs.get("sched").await.expect("get").expect("row");

        // A concurrent pass claims the fire first.
        assert!(
            h.defs
                .claim_schedule_fire(
                    "sched",
                    Some("2026-08-09T10:00:00.000Z"),
                    "2099-01-01T00:00:00.000Z"
                )
                .await
                .expect("winner claim")
        );

        let fired =
            fire_schedule_wake(&h.defs, &h.events, &h.runtime_config, &stale, Utc::now()).await;
        assert!(!fired);
        assert_eq!(h.events.pending_count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn outside_active_hours_skips_and_advances_cursor() {
        let h = harness().await;
        let mut def = schedule_def("sched", 600);
        // A one-hour window that never contains the current hour.
        let current_hour = chrono::Timelike::hour(&Utc::now()) as u8;
        def.active_hours = Some(((current_hour + 1) % 24, (current_hour + 2) % 24));
        h.defs.upsert(&def).await.expect("upsert");
        assert!(
            h.defs
                .claim_schedule_fire("sched", None, "2026-08-09T10:00:00.000Z")
                .await
                .expect("seed cursor")
        );

        let now = Utc::now();
        let enqueued = run_schedule_pass(&h.defs, &h.events, &h.runtime_config, now).await;

        assert!(!enqueued);
        assert_eq!(h.events.pending_count().await.expect("count"), 0);
        let def = h.defs.get("sched").await.expect("get").expect("row");
        let cursor = parse_run_timestamp(def.next_run_at.as_deref().expect("cursor"))
            .expect("cursor parses");
        assert!(cursor > now);
        assert!(def.last_fired_at.is_none());
    }
}
