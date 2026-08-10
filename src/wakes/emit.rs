//! Producer entry point for internal system events.
//!
//! Mutation sites call these helpers instead of touching the wake stores
//! directly: look up subscribed wake definitions, enqueue one durable row per
//! subscriber, and ring the wake doorbell when anything landed. The doorbell
//! is best-effort; durability comes from the `wake_events` table.

use crate::AgentDeps;
use crate::error::Result;
use crate::wakes::{SystemEvent, WakeDefStore, WakeEventStore};

use serde_json::Value;

/// Fan an event out to every enabled wake definition subscribed to it.
/// Enqueues one row per subscriber (coalesced arrivals count as enqueued)
/// and touches each definition's `last_fired_at`. Returns how many
/// subscribers were enqueued.
pub async fn emit_to_stores(
    def_store: &WakeDefStore,
    event_store: &WakeEventStore,
    event: SystemEvent,
    dedupe_key: &str,
    payload: &Value,
) -> Result<usize> {
    let subscribers = def_store.event_subscribers(event).await?;
    for def in &subscribers {
        event_store.enqueue(&def.id, dedupe_key, payload).await?;
        def_store.touch_last_fired(&def.id).await?;
    }
    Ok(subscribers.len())
}

/// Emit an event through an agent's deps, ringing the wake doorbell when
/// anything was enqueued. Emission failures are logged rather than
/// propagated so a caller's mutation never fails on wake plumbing.
pub async fn emit_system_event(
    deps: &AgentDeps,
    event: SystemEvent,
    dedupe_key: &str,
    payload: &Value,
) {
    match emit_to_stores(
        &deps.wake_def_store,
        &deps.wake_event_store,
        event,
        dedupe_key,
        payload,
    )
    .await
    {
        Ok(0) => {}
        Ok(_) => {
            if let Some(wake_tx) = &deps.wake_tx {
                crate::agent::wake::fire_wake(wake_tx, &deps.agent_id);
            }
        }
        Err(error) => {
            tracing::warn!(
                agent_id = %deps.agent_id,
                %event,
                dedupe_key,
                %error,
                "failed to emit system event",
            );
        }
    }
}

/// Emit an instance-level event to every registered agent. Per-agent wake
/// definitions filter: agents with no subscribed wake enqueue nothing, so
/// the sweep is cheap.
pub async fn emit_to_all_agents(
    registry: &tokio::sync::RwLock<std::collections::HashMap<crate::AgentId, AgentDeps>>,
    event: SystemEvent,
    dedupe_key: &str,
    payload: &Value,
) {
    // Clone deps out so the registry lock is not held across store writes.
    let all_deps: Vec<AgentDeps> = registry.read().await.values().cloned().collect();
    for deps in &all_deps {
        emit_system_event(deps, event, dedupe_key, payload).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutonomyLevel;
    use crate::wakes::{WakeDef, WakeTrigger};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn stores() -> (WakeDefStore, WakeEventStore) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        (WakeDefStore::new(pool.clone()), WakeEventStore::new(pool))
    }

    fn event_def(id: &str, event: SystemEvent) -> WakeDef {
        WakeDef {
            id: id.to_string(),
            name: format!("{id} name"),
            trigger: WakeTrigger::Event { event },
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
    async fn subscriber_match_enqueues_and_touches_last_fired() {
        let (defs, events) = stores().await;
        defs.upsert(&event_def("on-approve", SystemEvent::TaskApproved))
            .await
            .expect("upsert");

        let count = emit_to_stores(
            &defs,
            &events,
            SystemEvent::TaskApproved,
            "task:7",
            &serde_json::json!({"task_number": 7}),
        )
        .await
        .expect("emit");
        assert_eq!(count, 1);

        let pending = events.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].wake_id, "on-approve");
        assert_eq!(pending[0].dedupe_key, "task:7");
        assert_eq!(pending[0].payload["task_number"], 7);

        let def = defs.get("on-approve").await.expect("get").expect("row");
        assert!(def.last_fired_at.is_some());
    }

    #[tokio::test]
    async fn no_subscribers_enqueues_nothing() {
        let (defs, events) = stores().await;
        defs.upsert(&event_def("on-goal", SystemEvent::GoalCreated))
            .await
            .expect("upsert");

        let count = emit_to_stores(
            &defs,
            &events,
            SystemEvent::TaskApproved,
            "task:7",
            &serde_json::json!({}),
        )
        .await
        .expect("emit");
        assert_eq!(count, 0);
        assert_eq!(events.pending_count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn repeated_emission_coalesces_on_dedupe_key() {
        let (defs, events) = stores().await;
        defs.upsert(&event_def("on-worker", SystemEvent::WorkerCompleted))
            .await
            .expect("upsert");

        for attempt in 1..=2 {
            let count = emit_to_stores(
                &defs,
                &events,
                SystemEvent::WorkerCompleted,
                "worker:abc",
                &serde_json::json!({"attempt": attempt}),
            )
            .await
            .expect("emit");
            assert_eq!(count, 1);
        }

        let pending = events.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_count, 2);
        assert_eq!(pending[0].payload["attempt"], 2);
    }

    #[tokio::test]
    async fn fans_out_to_every_subscriber() {
        let (defs, events) = stores().await;
        defs.upsert(&event_def("first", SystemEvent::GoalUpdated))
            .await
            .expect("upsert");
        defs.upsert(&event_def("second", SystemEvent::GoalUpdated))
            .await
            .expect("upsert");

        let count = emit_to_stores(
            &defs,
            &events,
            SystemEvent::GoalUpdated,
            "goal:g1",
            &serde_json::json!({}),
        )
        .await
        .expect("emit");
        assert_eq!(count, 2);
        assert_eq!(events.pending_count().await.expect("count"), 2);
    }
}
