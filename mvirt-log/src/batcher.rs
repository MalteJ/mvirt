use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{error, info};
use ulid::Ulid;

use mvirt_log::distributed::DistributedLogStore;
use mvirt_log::LogEntry;

const BATCH_SIZE: usize = 100;
const FLUSH_TIMEOUT: Duration = Duration::from_millis(50);

pub struct Batcher {
    tx: mpsc::UnboundedSender<LogEntry>,
}

impl Batcher {
    pub fn new(store: Arc<DistributedLogStore>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_loop(rx, store));
        Self { tx }
    }

    pub fn submit(&self, entry: LogEntry) {
        let _ = self.tx.send(entry);
    }
}

async fn run_loop(mut rx: mpsc::UnboundedReceiver<LogEntry>, store: Arc<DistributedLogStore>) {
    loop {
        let first = match rx.recv().await {
            Some(e) => e,
            None => break,
        };

        let mut batch = Vec::with_capacity(BATCH_SIZE);
        batch.push(first);

        loop {
            if batch.len() >= BATCH_SIZE {
                break;
            }
            match timeout(FLUSH_TIMEOUT, rx.recv()).await {
                Ok(Some(entry)) => batch.push(entry),
                Ok(None) => break,
                Err(_) => break,
            }
        }

        // Assign timestamps and ULIDs before proposing to Raft
        for entry in &mut batch {
            if entry.timestamp_ns == 0 {
                if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                    entry.timestamp_ns = now.as_nanos() as i64;
                }
            }
            let ms = (entry.timestamp_ns / 1_000_000) as u64;
            let ulid = Ulid::from_parts(ms, rand::random());
            entry.id = ulid.to_string();
        }

        let len = batch.len();
        if let Err(e) = store.append_batch(batch).await {
            error!("Batch flush failed: {e}");
        } else {
            info!("Flushed {len} log entries");
        }

        if rx.is_closed() && rx.is_empty() {
            break;
        }
    }

    info!("Batcher shutdown");
}
