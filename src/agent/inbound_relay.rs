//! Order-preserving relay between the shared inbound router and a channel's
//! bounded message queue.
//!
//! The router's select loop must never await a single channel's queue: a
//! saturated channel would stall inbound delivery for every other
//! conversation. Each active channel gets a relay task that owns the awaited
//! send into the channel's bounded queue; the router enqueues through
//! [`InboundRelay::send`], which never blocks.
//!
//! Per-channel FIFO ordering is preserved because every message for a channel
//! flows through that channel's single relay task. Overflow beyond the
//! channel's bounded queue accumulates in the relay, where queue depth is
//! tracked and logged so a stalled channel is visible instead of silently
//! freezing the router.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;

use crate::InboundMessage;

/// Relay depth at which pressure warnings begin. Warnings repeat at each
/// doubling above this so a runaway channel logs O(log n) lines, not O(n).
const PRESSURE_WARN_DEPTH: usize = 128;

/// Non-blocking sender half handed to the router.
#[derive(Clone)]
pub struct InboundRelay {
    tx: mpsc::UnboundedSender<InboundMessage>,
    state: Arc<RelayState>,
}

struct RelayState {
    conversation_id: String,
    agent_id: String,
    queued: AtomicUsize,
    warn_at: AtomicUsize,
}

/// Returned when the relay task has stopped, which means the channel's event
/// loop is gone. Carries the message back (boxed — `InboundMessage` is large
/// and this is the cold path) so the caller can requeue it.
#[derive(Debug)]
pub struct RelaySendError(pub Box<InboundMessage>);

impl std::fmt::Display for RelaySendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "channel relay closed")
    }
}

impl std::error::Error for RelaySendError {}

impl InboundRelay {
    /// Enqueue a message for in-order delivery to the channel. Never blocks.
    /// Fails only when the relay task has exited (channel shut down).
    pub fn send(&self, message: InboundMessage) -> Result<(), RelaySendError> {
        self.tx
            .send(message)
            .map_err(|error| RelaySendError(Box::new(error.0)))?;

        let depth = self.state.queued.fetch_add(1, Ordering::Relaxed) + 1;
        let warn_at = self.state.warn_at.load(Ordering::Relaxed);
        if depth >= warn_at
            && self
                .state
                .warn_at
                .compare_exchange(
                    warn_at,
                    warn_at.saturating_mul(2),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            tracing::warn!(
                conversation_id = %self.state.conversation_id,
                agent_id = %self.state.agent_id,
                depth,
                "channel is not draining its message queue; relay is buffering"
            );
        }
        Ok(())
    }
}

/// Spawn a relay task that forwards messages into `channel_tx`, and return
/// the non-blocking sender for the router. The task exits when either side
/// closes: all `InboundRelay` clones dropped, or the channel's receiver gone.
pub fn spawn(
    conversation_id: &str,
    agent_id: &str,
    channel_tx: mpsc::Sender<InboundMessage>,
) -> InboundRelay {
    let (tx, mut rx) = mpsc::unbounded_channel::<InboundMessage>();
    let state = Arc::new(RelayState {
        conversation_id: conversation_id.to_string(),
        agent_id: agent_id.to_string(),
        queued: AtomicUsize::new(0),
        warn_at: AtomicUsize::new(PRESSURE_WARN_DEPTH),
    });

    let relay_state = state.clone();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if channel_tx.send(message).await.is_err() {
                tracing::debug!(
                    conversation_id = %relay_state.conversation_id,
                    agent_id = %relay_state.agent_id,
                    "channel receiver dropped; relay exiting"
                );
                break;
            }

            let depth = relay_state.queued.fetch_sub(1, Ordering::Relaxed) - 1;
            if depth == 0
                && relay_state
                    .warn_at
                    .swap(PRESSURE_WARN_DEPTH, Ordering::Relaxed)
                    > PRESSURE_WARN_DEPTH
            {
                tracing::info!(
                    conversation_id = %relay_state.conversation_id,
                    agent_id = %relay_state.agent_id,
                    "channel relay drained after pressure"
                );
            }
        }
    });

    InboundRelay { tx, state }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: usize) -> InboundMessage {
        let mut message = InboundMessage::empty();
        message.id = id.to_string();
        message
    }

    #[tokio::test]
    async fn enqueue_does_not_block_on_full_channel_queue() {
        let (channel_tx, _channel_rx) = mpsc::channel(1);
        let relay = spawn("conv", "agent", channel_tx);

        // Nothing consumes the channel queue, so anything past its capacity
        // buffers in the relay. All sends must complete without awaiting.
        for id in 0..200 {
            relay.send(message(id)).expect("relay accepts while alive");
        }
    }

    #[tokio::test]
    async fn preserves_fifo_order_through_pressure() {
        let (channel_tx, mut channel_rx) = mpsc::channel(2);
        let relay = spawn("conv", "agent", channel_tx);

        for id in 0..50 {
            relay.send(message(id)).expect("relay accepts while alive");
        }

        for expected in 0..50 {
            let received = channel_rx.recv().await.expect("message delivered");
            assert_eq!(received.id, expected.to_string());
        }
    }

    #[tokio::test]
    async fn send_fails_once_channel_receiver_is_gone() {
        let (channel_tx, channel_rx) = mpsc::channel(1);
        let relay = spawn("conv", "agent", channel_tx);
        drop(channel_rx);

        // The relay only notices on its next forward attempt; give it one
        // message to trip on, then wait for the task to exit.
        let _ = relay.send(message(0));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if relay.send(message(1)).is_err() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "relay should reject sends after the channel receiver dropped"
            );
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn returns_message_on_relay_shutdown() {
        let (channel_tx, channel_rx) = mpsc::channel(1);
        let relay = spawn("conv", "agent", channel_tx);
        drop(channel_rx);

        let _ = relay.send(message(0));
        loop {
            match relay.send(message(7)) {
                Ok(()) => tokio::task::yield_now().await,
                Err(RelaySendError(returned)) => {
                    assert_eq!(returned.id, "7");
                    break;
                }
            }
        }
    }
}
