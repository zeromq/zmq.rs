//! Reconnection infrastructure for `ZeroMQ` sockets.
//!
//! This module provides auto-reconnection capability when connected endpoints
//! are disconnected. When a peer disconnects, a background task attempts to
//! reconnect with exponential backoff.

use crate::async_rt::task::{spawn, JoinHandle};
use crate::backend::DisconnectNotifier;
use crate::endpoint::Endpoint;
use crate::transport;
use crate::util::{greet_exchange, ready_exchange, PeerIdentity};
use crate::MultiPeerBackend;

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use rand::RngExt;

use std::sync::Arc;
use std::time::Duration;

/// Configuration for reconnection behavior.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial delay before first reconnection attempt (default: 100ms)
    pub initial_interval: Duration,
    /// Maximum delay between reconnection attempts (default: 30s)
    pub max_interval: Duration,
    /// Multiplier for exponential backoff (default: 2.0)
    pub backoff_multiplier: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_millis(100),
            max_interval: Duration::from_secs(30),
            backoff_multiplier: 2.0,
        }
    }
}

/// Handle to a running reconnection task.
///
/// When dropped, the reconnection task continues to run. Call `shutdown()` to
/// stop the task gracefully.
pub struct ReconnectHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    #[allow(dead_code)] // Kept to prevent task from being dropped prematurely
    task_handle: JoinHandle<()>,
}

impl ReconnectHandle {
    /// Request graceful shutdown of the reconnection task.
    ///
    /// This signals the task to stop and returns immediately. The task will
    /// finish its current iteration before stopping.
    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // task_handle is dropped, which is fine - we don't need to await it
    }
}

/// Type for a function that registers disconnect notifiers with a backend.
pub type RegisterDisconnectFn = Box<dyn Fn(PeerIdentity, DisconnectNotifier) + Send + Sync>;

/// Spawns a reconnection task for a single endpoint.
///
/// The task monitors for disconnect notifications. When a disconnect is received,
/// it attempts to reconnect with exponential backoff.
///
/// On successful reconnection:
/// - The handshake (greeting + ready exchange) is performed
/// - `backend.peer_connected()` is called, which triggers subscription resync for SUB sockets
/// - The new `peer_id` is registered for future disconnect notifications via `register_disconnect_fn`
///
/// # Arguments
/// * `endpoint` - The endpoint to reconnect to
/// * `backend` - The socket backend (used for handshake and peer registration)
/// * `initial_peer_id` - The `peer_id` from the initial connection
/// * `register_disconnect_fn` - Callback to register disconnect notifiers with the backend
/// * `config` - Reconnection configuration (intervals, backoff)
///
/// # Returns
/// A `ReconnectHandle` to control the task.
pub fn spawn_reconnect_task(
    endpoint: Endpoint,
    backend: Arc<dyn MultiPeerBackend>,
    initial_peer_id: PeerIdentity,
    register_disconnect_fn: RegisterDisconnectFn,
    config: ReconnectConfig,
) -> ReconnectHandle {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    // Create the disconnect notification channel - this task owns the receiver
    let (disconnect_tx, mut disconnect_rx) = mpsc::channel::<PeerIdentity>(1);

    // Register the initial peer_id
    register_disconnect_fn(initial_peer_id.clone(), disconnect_tx.clone());

    let task_handle = spawn(async move {
        log::debug!("Reconnect task started for endpoint: {}", endpoint);

        // Fuse shutdown_rx so it can be polled multiple times after completion
        let mut shutdown_rx = shutdown_rx.fuse();

        loop {
            // Wait for a disconnect notification or shutdown signal
            let peer_id = futures::select! {
                peer_id = disconnect_rx.next() => {
                    if let Some(id) = peer_id {
                        id
                    } else {
                        log::debug!("Disconnect channel closed, stopping reconnect task");
                        return;
                    }
                }
                _ = shutdown_rx => {
                    log::debug!("Shutdown received, stopping reconnect task");
                    return;
                }
            };

            log::info!(
                "Peer {:?} disconnected from {}, starting reconnection",
                peer_id,
                endpoint
            );

            // Attempt reconnection with exponential backoff
            let mut current_interval = config.initial_interval;
            let mut attempt = 0u32;

            'retry: loop {
                attempt += 1;
                log::debug!(
                    "Reconnection attempt {} to {} (waiting {:?})",
                    attempt,
                    endpoint,
                    current_interval
                );

                // Wait before attempting reconnection, but check for shutdown
                let sleep_fut = crate::async_rt::task::sleep(current_interval).fuse();
                futures::pin_mut!(sleep_fut);

                futures::select! {
                    _ = sleep_fut => {
                        // Sleep completed, proceed to reconnection attempt
                    }
                    _ = shutdown_rx => {
                        log::debug!("Shutdown received during backoff, stopping reconnect task");
                        return;
                    }
                }

                // Try to connect
                match try_reconnect(&endpoint, backend.clone()).await {
                    Ok((new_peer_id, resolved_endpoint)) => {
                        log::info!(
                            "Successfully reconnected to {} (peer {:?})",
                            endpoint,
                            new_peer_id
                        );

                        // Emit Connected event for monitor consumers
                        if let Some(monitor) = backend.monitor().lock().as_mut() {
                            let _ = monitor.try_send(crate::SocketEvent::Connected(
                                resolved_endpoint,
                                new_peer_id.clone(),
                            ));
                        }

                        // Register the new peer_id for future disconnect notifications
                        register_disconnect_fn(new_peer_id, disconnect_tx.clone());
                        // Reconnection successful, go back to waiting for disconnects
                        break 'retry;
                    }
                    Err(e) => {
                        log::warn!(
                            "Reconnection attempt {} to {} failed: {:?}",
                            attempt,
                            endpoint,
                            e
                        );

                        // Calculate next backoff interval with jitter
                        let jitter = {
                            let mut rng = rand::rng();
                            rng.random_range(0.0..0.1)
                        };
                        let next_interval_secs =
                            current_interval.as_secs_f64() * config.backoff_multiplier + jitter;
                        current_interval = Duration::from_secs_f64(
                            next_interval_secs.min(config.max_interval.as_secs_f64()),
                        );
                    }
                }
            }
        }
    });

    ReconnectHandle {
        shutdown_tx: Some(shutdown_tx),
        task_handle,
    }
}

/// Attempts a single reconnection to the endpoint.
///
/// This performs the full connection sequence:
/// 1. TCP/IPC connection
/// 2. ZMTP greeting exchange
/// 3. Ready command exchange
/// 4. Peer registration via `backend.peer_connected()`
///
/// Returns the new `peer_id` and resolved endpoint on success.
async fn try_reconnect(
    endpoint: &Endpoint,
    backend: Arc<dyn MultiPeerBackend>,
) -> crate::ZmqResult<(PeerIdentity, Endpoint)> {
    // Attempt transport-level connection
    let (mut raw_socket, resolved_endpoint) = transport::connect(endpoint).await?;

    // Perform ZMTP handshake
    greet_exchange(&mut raw_socket).await?;

    // Build properties for ready exchange (include identity if configured)
    let mut props = None;
    if let Some(identity) = &backend.socket_options().peer_id {
        let mut connect_ops = std::collections::HashMap::new();
        connect_ops.insert("Identity".to_string(), identity.clone().into());
        props = Some(connect_ops);
    }

    // Exchange ready commands
    let peer_id = ready_exchange(&mut raw_socket, backend.socket_type(), props).await?;

    // Register the peer with the backend
    // This triggers subscription resync for SUB sockets
    backend.peer_connected(&peer_id, raw_socket).await;

    Ok((peer_id, resolved_endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_config_default() {
        let config = ReconnectConfig::default();
        assert_eq!(config.initial_interval, Duration::from_millis(100));
        assert_eq!(config.max_interval, Duration::from_secs(30));
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }
}
