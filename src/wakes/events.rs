//! Typed internal system events that wake definitions subscribe to.
//!
//! The vocabulary is a closed enum with a string round-trip, so unknown event
//! names in wake configuration are a load-time error rather than a silent
//! no-op. There is deliberately no broadcast channel behind these: producers
//! call the wake queue directly at their mutation sites, and durability comes
//! from the `wake_events` table.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub enum SystemEvent {
    #[serde(rename = "task.approved")]
    TaskApproved,
    #[serde(rename = "task.commented")]
    TaskCommented,
    #[serde(rename = "goal.created")]
    GoalCreated,
    #[serde(rename = "goal.updated")]
    GoalUpdated,
    #[serde(rename = "worker.completed")]
    WorkerCompleted,
    #[serde(rename = "worker.failed")]
    WorkerFailed,
    #[serde(rename = "agent.message")]
    AgentMessage,
    #[serde(rename = "cortex.observation")]
    CortexObservation,
    #[serde(rename = "ingest.file_added")]
    IngestFileAdded,
}

impl SystemEvent {
    pub const ALL: [SystemEvent; 9] = [
        SystemEvent::TaskApproved,
        SystemEvent::TaskCommented,
        SystemEvent::GoalCreated,
        SystemEvent::GoalUpdated,
        SystemEvent::WorkerCompleted,
        SystemEvent::WorkerFailed,
        SystemEvent::AgentMessage,
        SystemEvent::CortexObservation,
        SystemEvent::IngestFileAdded,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SystemEvent::TaskApproved => "task.approved",
            SystemEvent::TaskCommented => "task.commented",
            SystemEvent::GoalCreated => "goal.created",
            SystemEvent::GoalUpdated => "goal.updated",
            SystemEvent::WorkerCompleted => "worker.completed",
            SystemEvent::WorkerFailed => "worker.failed",
            SystemEvent::AgentMessage => "agent.message",
            SystemEvent::CortexObservation => "cortex.observation",
            SystemEvent::IngestFileAdded => "ingest.file_added",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "task.approved" => Some(SystemEvent::TaskApproved),
            "task.commented" => Some(SystemEvent::TaskCommented),
            "goal.created" => Some(SystemEvent::GoalCreated),
            "goal.updated" => Some(SystemEvent::GoalUpdated),
            "worker.completed" => Some(SystemEvent::WorkerCompleted),
            "worker.failed" => Some(SystemEvent::WorkerFailed),
            "agent.message" => Some(SystemEvent::AgentMessage),
            "cortex.observation" => Some(SystemEvent::CortexObservation),
            "ingest.file_added" => Some(SystemEvent::IngestFileAdded),
            _ => None,
        }
    }
}

impl std::fmt::Display for SystemEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_variant() {
        for event in SystemEvent::ALL {
            assert_eq!(SystemEvent::parse(event.as_str()), Some(event));
        }
    }

    #[test]
    fn rejects_unknown_names() {
        assert_eq!(SystemEvent::parse("task.deleted"), None);
        assert_eq!(SystemEvent::parse(""), None);
    }

    #[test]
    fn serde_names_match_as_str() {
        for event in SystemEvent::ALL {
            let serialized = serde_json::to_value(event).expect("serialize");
            assert_eq!(serialized, serde_json::Value::from(event.as_str()));
            let deserialized: SystemEvent =
                serde_json::from_value(serialized).expect("deserialize");
            assert_eq!(deserialized, event);
        }
    }
}
