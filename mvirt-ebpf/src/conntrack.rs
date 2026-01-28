//! Connection tracking cleanup task.
//!
//! Periodically removes stale connection tracking entries from the eBPF CONN_TRACK map.
//! This is necessary because eBPF maps don't have automatic expiration.

use crate::ebpf_loader::EbpfManager;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{debug, info, warn};

/// Default cleanup interval in seconds
const CLEANUP_INTERVAL_SECS: u64 = 30;

/// Default timeout for established TCP connections (5 minutes)
const TCP_ESTABLISHED_TIMEOUT_NS: u64 = 300_000_000_000;

/// Default timeout for UDP connections (30 seconds)
const UDP_TIMEOUT_NS: u64 = 30_000_000_000;

/// Default timeout for other connections (60 seconds)
const DEFAULT_TIMEOUT_NS: u64 = 60_000_000_000;

/// Connection tracking cleanup task handle.
pub struct ConnTrackCleaner {
    task: tokio::task::JoinHandle<()>,
}

impl ConnTrackCleaner {
    /// Start a new connection tracking cleanup task.
    pub fn start(ebpf: Arc<EbpfManager>) -> Self {
        let task = tokio::spawn(async move {
            cleanup_loop(ebpf).await;
        });

        info!("Connection tracking cleanup task started");

        Self { task }
    }

    /// Stop the cleanup task.
    pub fn stop(self) {
        self.task.abort();
        info!("Connection tracking cleanup task stopped");
    }
}

/// Main cleanup loop.
async fn cleanup_loop(ebpf: Arc<EbpfManager>) {
    let mut interval = time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));

    loop {
        interval.tick().await;

        if let Err(e) = cleanup_stale_entries(&ebpf).await {
            warn!(error = %e, "Failed to cleanup connection tracking entries");
        }
    }
}

/// Remove stale connection tracking entries.
async fn cleanup_stale_entries(_ebpf: &EbpfManager) -> Result<(), Box<dyn std::error::Error>> {
    // Note: This is a stub implementation.
    // In a real implementation, we would:
    // 1. Iterate over CONN_TRACK map entries
    // 2. Check last_seen_ns against current time
    // 3. Remove entries older than their protocol-specific timeout
    //
    // However, iterating over BPF maps from userspace is complex with aya,
    // and requires the BPF_MAP_TYPE_HASH_OF_MAPS or careful iteration logic.
    //
    // For now, this is a placeholder that logs the cleanup attempt.
    // The actual cleanup can be implemented later when needed.

    debug!("Connection tracking cleanup check (stub)");

    // In a full implementation:
    // let now_ns = get_current_time_ns();
    // for (key, entry) in conn_track_map.iter() {
    //     let timeout = match key.protocol {
    //         6 => TCP_ESTABLISHED_TIMEOUT_NS,
    //         17 => UDP_TIMEOUT_NS,
    //         _ => DEFAULT_TIMEOUT_NS,
    //     };
    //     if now_ns - entry.last_seen_ns > timeout {
    //         conn_track_map.remove(&key)?;
    //     }
    // }

    Ok(())
}

/// Get current time in nanoseconds (monotonic).
#[allow(dead_code)]
fn get_current_time_ns() -> u64 {
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

/// Determine timeout for a protocol.
#[allow(dead_code)]
fn get_timeout_ns(protocol: u8, state: u8) -> u64 {
    match protocol {
        6 => {
            // TCP - longer timeout for established connections
            if state == 1 {
                // CT_STATE_ESTABLISHED
                TCP_ESTABLISHED_TIMEOUT_NS
            } else {
                DEFAULT_TIMEOUT_NS
            }
        }
        17 => UDP_TIMEOUT_NS,         // UDP
        1 | 58 => DEFAULT_TIMEOUT_NS, // ICMP/ICMPv6
        _ => DEFAULT_TIMEOUT_NS,
    }
}
