//! Wakes: named conditions under which the agent stirs without a user
//! message, paired with instructions for what to do when they fire.
//!
//! See docs/design-docs/wakes.md for the full model. This module holds the
//! typed event vocabulary and the persisted wake-event queue; producers and
//! the consuming autonomy channel plug in around them.

mod config;
mod defs;
mod emit;
mod events;
mod runs;
mod schedule;
mod store;

pub use config::{WakeConfig, reconcile_config_wakes};
pub use defs::{TASK_APPROVED_WAKE_ID, WakeDef, WakeDefStore, WakeTrigger, seed_builtin_wakes};
pub use emit::{emit_system_event, emit_to_all_agents, emit_to_stores};
pub use events::SystemEvent;
pub(crate) use runs::parse_run_timestamp;
pub use runs::{AutonomyAction, AutonomyRun, AutonomyRunStatus, AutonomyRunStore};
pub use schedule::fire_due_schedule_wakes;
pub use store::{EnqueueOutcome, WakeEvent, WakeEventStore};
