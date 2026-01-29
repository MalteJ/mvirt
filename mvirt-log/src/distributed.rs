use anyhow::Result;
use mraft::RaftNode;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::LogEntry;

use crate::command::{LogCommand, LogCommandResponse, SerializableLogEntry};
use crate::storage::{log_manager, LogStateMachine};

pub struct DistributedLogStore {
    node: Arc<RwLock<RaftNode<LogCommand, LogCommandResponse, LogStateMachine>>>,
}

impl DistributedLogStore {
    pub fn new(
        node: Arc<RwLock<RaftNode<LogCommand, LogCommandResponse, LogStateMachine>>>,
    ) -> Self {
        Self { node }
    }

    pub async fn append_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        let serializable: Vec<SerializableLogEntry> = entries.into_iter().map(Into::into).collect();
        let cmd = LogCommand::AppendBatch(serializable);
        let node = self.node.read().await;
        node.write_or_forward(cmd)
            .await
            .map_err(|e| anyhow::anyhow!("Raft write failed: {e}"))?;
        Ok(())
    }

    pub async fn query(
        &self,
        object_id: Option<String>,
        start_ns: Option<i64>,
        end_ns: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LogEntry>> {
        let manager = log_manager();
        manager.query(object_id, start_ns, end_ns, limit)
    }
}
