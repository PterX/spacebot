//! Wakes: named conditions under which the agent stirs without a user
//! message, paired with instructions for what to do when they fire.
//!
//! See docs/design-docs/wakes.md for the full model. This module holds the
//! typed event vocabulary and the persisted wake-event queue; producers and
//! the consuming autonomy channel plug in around them.

mod events;
mod store;

pub use events::SystemEvent;
pub use store::{EnqueueOutcome, WakeEvent, WakeEventStore};
