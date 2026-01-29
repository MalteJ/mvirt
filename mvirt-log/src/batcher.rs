use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{error, info};

use mvirt_log::LogEntry;

use crate::storage::LogManager;

const BATCH_SIZE: usize = 100;
const FLUSH_TIMEOUT: Duration = Duration::from_millis(50);

pub struct Batcher {
    tx: mpsc::UnboundedSender<LogEntry>,
}

impl Batcher {
    pub fn new(storage: Arc<LogManager>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_loop(rx, storage));
        Self { tx }
    }

    pub fn submit(&self, entry: LogEntry) {
        let _ = self.tx.send(entry);
    }
}

async fn run_loop(mut rx: mpsc::UnboundedReceiver<LogEntry>, storage: Arc<LogManager>) {
    loop {
        // Wait for first entry
        let first = match rx.recv().await {
            Some(e) => e,
            None => break, // channel closed
        };

        let mut batch = Vec::with_capacity(BATCH_SIZE);
        batch.push(first);

        // Drain more until batch full or timeout
        loop {
            if batch.len() >= BATCH_SIZE {
                break;
            }
            match timeout(FLUSH_TIMEOUT, rx.recv()).await {
                Ok(Some(entry)) => batch.push(entry),
                Ok(None) => break, // channel closed, flush remaining
                Err(_) => break,   // timeout, flush
            }
        }

        let len = batch.len();
        let s = storage.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || s.append_batch(batch)).await {
            error!("Batch flush failed: {e}");
        } else {
            info!("Flushed {len} log entries");
        }

        // If channel closed after draining, exit
        if rx.is_closed() && rx.is_empty() {
            break;
        }
    }

    info!("Batcher shutdown");
}
