//! Supervision control plane for channel cancellation.

use crate::agent::channel::WeakChannelControlHandle;
use crate::{BranchId, ChannelId, WorkerId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlActionResult {
    Cancelled,
    NotFound,
    AlreadyTerminal,
}

#[derive(Clone)]
struct ChannelControlEntry {
    handle: WeakChannelControlHandle,
    registration_id: u64,
}

enum ChannelLookupResult {
    Found(crate::agent::channel::ChannelControlHandle),
    Stale(u64),
    Missing,
}

pub struct ProcessControlRegistry {
    channels: tokio::sync::RwLock<HashMap<ChannelId, ChannelControlEntry>>,
    next_channel_registration: AtomicU64,
}

impl Default for ProcessControlRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessControlRegistry {
    pub fn new() -> Self {
        Self {
            channels: tokio::sync::RwLock::new(HashMap::new()),
            next_channel_registration: AtomicU64::new(1),
        }
    }

    pub async fn register_channel(
        &self,
        channel_id: ChannelId,
        handle: WeakChannelControlHandle,
    ) -> u64 {
        let registration_id = self
            .next_channel_registration
            .fetch_add(1, Ordering::AcqRel);
        self.channels.write().await.insert(
            channel_id,
            ChannelControlEntry {
                handle,
                registration_id,
            },
        );
        registration_id
    }

    pub async fn unregister_channel(&self, channel_id: &ChannelId, registration_id: u64) -> bool {
        let mut channels = self.channels.write().await;
        let should_remove = channels
            .get(channel_id)
            .is_some_and(|entry| entry.registration_id == registration_id);
        if should_remove {
            channels.remove(channel_id);
        }
        should_remove
    }

    pub async fn prune_dead_channels(&self) -> usize {
        let mut channels = self.channels.write().await;
        let before = channels.len();
        channels.retain(|_, entry| entry.handle.upgrade().is_some());
        before.saturating_sub(channels.len())
    }

    /// Live control handle for a channel, when one is running. Prunes a
    /// stale registration on the way.
    pub async fn channel_handle(
        &self,
        channel_id: &ChannelId,
    ) -> Option<crate::agent::channel::ChannelControlHandle> {
        match self.lookup_channel_handle(channel_id).await {
            ChannelLookupResult::Found(handle) => Some(handle),
            ChannelLookupResult::Stale(registration_id) => {
                self.remove_stale_channel_if_matches(channel_id, registration_id)
                    .await;
                None
            }
            ChannelLookupResult::Missing => None,
        }
    }

    async fn lookup_channel_handle(&self, channel_id: &ChannelId) -> ChannelLookupResult {
        let handle_entry = {
            let channels = self.channels.read().await;
            let Some(handle_entry) = channels.get(channel_id).cloned() else {
                return ChannelLookupResult::Missing;
            };

            handle_entry
        };

        match handle_entry.handle.upgrade() {
            Some(handle) => ChannelLookupResult::Found(handle),
            None => ChannelLookupResult::Stale(handle_entry.registration_id),
        }
    }

    pub async fn cancel_channel_worker(
        &self,
        channel_id: &ChannelId,
        worker_id: WorkerId,
        reason: &str,
    ) -> ControlActionResult {
        for _ in 0..2 {
            match self.lookup_channel_handle(channel_id).await {
                ChannelLookupResult::Found(handle) => {
                    return handle.cancel_worker_with_reason(worker_id, reason).await;
                }
                ChannelLookupResult::Stale(registration_id) => {
                    self.remove_stale_channel_if_matches(channel_id, registration_id)
                        .await;
                }
                ChannelLookupResult::Missing => return ControlActionResult::NotFound,
            }
        }
        ControlActionResult::NotFound
    }

    pub async fn cancel_channel_branch(
        &self,
        channel_id: &ChannelId,
        branch_id: BranchId,
        reason: &str,
    ) -> ControlActionResult {
        for _ in 0..2 {
            match self.lookup_channel_handle(channel_id).await {
                ChannelLookupResult::Found(handle) => {
                    return handle.cancel_branch_with_reason(branch_id, reason).await;
                }
                ChannelLookupResult::Stale(registration_id) => {
                    self.remove_stale_channel_if_matches(channel_id, registration_id)
                        .await;
                }
                ChannelLookupResult::Missing => return ControlActionResult::NotFound,
            }
        }
        ControlActionResult::NotFound
    }

    async fn remove_stale_channel_if_matches(
        &self,
        channel_id: &ChannelId,
        expected_registration_id: u64,
    ) -> bool {
        let mut channels = self.channels.write().await;
        let should_remove = channels
            .get(channel_id)
            .is_some_and(|current| current.registration_id == expected_registration_id);

        if should_remove {
            channels.remove(channel_id);
        }

        should_remove
    }
}

#[cfg(test)]
mod tests {
    use super::{ControlActionResult, ProcessControlRegistry};
    use crate::agent::channel::WeakChannelControlHandle;
    use std::sync::Arc;

    #[tokio::test]
    async fn prune_dead_channels_removes_stale_entries() {
        let registry = ProcessControlRegistry::new();
        let channel_id: crate::ChannelId = Arc::from("channel-1");
        let registration_id = registry
            .register_channel(
                channel_id.clone(),
                crate::agent::channel::WeakChannelControlHandle::dangling(),
            )
            .await;

        let pruned = registry.prune_dead_channels().await;

        assert_eq!(pruned, 1);
        assert!(
            !registry
                .unregister_channel(&channel_id, registration_id)
                .await
        );
    }

    #[tokio::test]
    async fn stale_channel_entry_cleanup_only_removes_matching_registration_id() {
        let registry = ProcessControlRegistry::new();
        let channel_id: crate::ChannelId = Arc::from("channel-stale-race");
        let stale_handle = WeakChannelControlHandle::dangling();

        let stale_registration_id = registry
            .register_channel(channel_id.clone(), stale_handle)
            .await;

        let active_registration_id = registry
            .register_channel(channel_id.clone(), WeakChannelControlHandle::dangling())
            .await;

        assert!(
            !registry
                .remove_stale_channel_if_matches(&channel_id, stale_registration_id)
                .await
        );
        assert!(
            !registry
                .unregister_channel(&channel_id, stale_registration_id)
                .await
        );
        assert!(
            registry
                .unregister_channel(&channel_id, active_registration_id)
                .await
        );
    }

    #[tokio::test]
    async fn cancel_missing_entries_is_idempotent_not_found() {
        let registry = ProcessControlRegistry::new();
        let channel_id: crate::ChannelId = Arc::from("missing-channel");
        let worker_id = uuid::Uuid::new_v4();
        let branch_id = uuid::Uuid::new_v4();

        assert_eq!(
            registry
                .cancel_channel_worker(&channel_id, worker_id, "test")
                .await,
            ControlActionResult::NotFound
        );
        assert_eq!(
            registry
                .cancel_channel_branch(&channel_id, branch_id, "test")
                .await,
            ControlActionResult::NotFound
        );
    }

    #[tokio::test]
    async fn cancel_stale_channel_entry_prunes_then_returns_not_found() {
        let registry = ProcessControlRegistry::new();
        let channel_id: crate::ChannelId = Arc::from("stale-channel");
        let worker_id = uuid::Uuid::new_v4();

        let registration_id = registry
            .register_channel(channel_id.clone(), WeakChannelControlHandle::dangling())
            .await;

        assert_eq!(
            registry
                .cancel_channel_worker(&channel_id, worker_id, "test")
                .await,
            ControlActionResult::NotFound
        );
        assert!(
            !registry
                .unregister_channel(&channel_id, registration_id)
                .await,
            "stale entry should be pruned during cancellation retry path"
        );
    }
}
