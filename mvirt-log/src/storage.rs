use anyhow::Result;
use mraft::StateMachine;
use prost::Message;
use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tracing::error;
use ulid::Ulid;

use crate::LogEntry;

use crate::command::{LogCommand, LogCommandResponse};

const TABLE_LOGS: TableDefinition<u128, &[u8]> = TableDefinition::new("logs");
const TABLE_IDX_OBJECT: TableDefinition<(&str, u128), ()> = TableDefinition::new("idx_object");
const TABLE_IDX_COMPONENT: TableDefinition<(&str, i32, u128), ()> =
    TableDefinition::new("idx_component");

/// Global LogManager instance, set once at startup before Raft node creation.
static LOG_MANAGER: OnceLock<Arc<LogManager>> = OnceLock::new();

/// Initialize the global LogManager. Must be called before creating the RaftNode.
pub fn init_log_manager(manager: Arc<LogManager>) {
    assert!(
        LOG_MANAGER.set(manager).is_ok(),
        "LogManager already initialized"
    );
}

/// Get the global LogManager.
pub fn log_manager() -> &'static Arc<LogManager> {
    LOG_MANAGER
        .get()
        .expect("LogManager not initialized — call init_log_manager() first")
}

pub struct LogManager {
    db: Database,
}

impl LogManager {
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self> {
        let db_path = data_dir.as_ref().join("logs.redb");
        let db = Database::create(&db_path)?;

        let txn = db.begin_write()?;
        txn.open_table(TABLE_LOGS)?;
        txn.open_table(TABLE_IDX_OBJECT)?;
        txn.open_table(TABLE_IDX_COMPONENT)?;
        txn.commit()?;

        Ok(Self { db })
    }

    /// Insert entries that already have id and timestamp_ns set.
    pub fn append_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut logs = txn.open_table(TABLE_LOGS)?;
            let mut idx_obj = txn.open_table(TABLE_IDX_OBJECT)?;
            let mut idx_comp = txn.open_table(TABLE_IDX_COMPONENT)?;

            for entry in entries {
                let ulid: Ulid = entry
                    .id
                    .parse()
                    .map_err(|e| anyhow::anyhow!("Invalid ULID in entry: {e}"))?;

                let key = ulid.0;
                let encoded = entry.encode_to_vec();
                logs.insert(key, encoded.as_slice())?;

                for obj_id in &entry.related_object_ids {
                    idx_obj.insert((obj_id.as_str(), key), ())?;
                }

                idx_comp.insert((entry.component.as_str(), entry.level, key), ())?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    pub fn query(
        &self,
        object_id: Option<String>,
        start_ns: Option<i64>,
        end_ns: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LogEntry>> {
        let start_ms = start_ns.unwrap_or(0) / 1_000_000;
        let end_ms = end_ns.unwrap_or(i64::MAX) / 1_000_000;
        let min_ulid = Ulid::from_parts(start_ms as u64, 0).0;
        let max_ulid = Ulid::from_parts(end_ms as u64, u128::MAX).0;

        let txn = self.db.begin_read()?;
        let logs = txn.open_table(TABLE_LOGS)?;
        let mut results = Vec::new();

        if let Some(obj) = object_id {
            let idx_obj = txn.open_table(TABLE_IDX_OBJECT)?;
            let range = idx_obj.range((obj.as_str(), min_ulid)..=(obj.as_str(), max_ulid))?;

            for item in range {
                if results.len() >= limit {
                    break;
                }
                let (key, _) = item?;
                let (_, ulid_key) = key.value();
                if let Some(access) = logs.get(ulid_key)? {
                    let entry = LogEntry::decode(access.value())?;
                    results.push(entry);
                }
            }
        } else {
            let range = logs.range(min_ulid..=max_ulid)?;

            for item in range {
                if results.len() >= limit {
                    break;
                }
                let (_, value) = item?;
                let entry = LogEntry::decode(value.value())?;
                results.push(entry);
            }
        }

        Ok(results)
    }
}

/// Raft state machine. Accesses the global LogManager for writes.
/// Serialization is stubbed — snapshot/restore deferred for append-only log data.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogStateMachine;

impl StateMachine<LogCommand, LogCommandResponse> for LogStateMachine {
    type Event = ();

    fn apply(&mut self, cmd: LogCommand) -> (LogCommandResponse, Vec<Self::Event>) {
        match cmd {
            LogCommand::AppendBatch(entries) => {
                let manager = log_manager();
                let log_entries: Vec<LogEntry> = entries.into_iter().map(Into::into).collect();
                if let Err(e) = manager.append_batch(log_entries) {
                    error!("Failed to apply AppendBatch: {e}");
                }
                (LogCommandResponse::Ok, vec![])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::SerializableLogEntry;
    use tempfile::TempDir;

    fn make_entry(id: &str, timestamp_ns: i64, message: &str, objects: Vec<&str>) -> LogEntry {
        LogEntry {
            id: id.to_string(),
            timestamp_ns,
            message: message.to_string(),
            level: 1,
            component: "test".to_string(),
            related_object_ids: objects.into_iter().map(String::from).collect(),
        }
    }

    fn ulid_at_ms(ms: u64) -> String {
        Ulid::from_parts(ms, rand::random()).to_string()
    }

    #[test]
    fn append_and_query_all() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let ts = 1_700_000_000_000_000_000i64; // ns
        let id = ulid_at_ms((ts / 1_000_000) as u64);
        let entry = make_entry(&id, ts, "test log", vec![]);

        mgr.append_batch(vec![entry]).unwrap();

        let results = mgr.query(None, None, None, 100).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "test log");
        assert_eq!(results[0].id, id);
    }

    #[test]
    fn query_by_object_id() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let ts = 1_700_000_000_000_000_000i64;
        let id1 = ulid_at_ms((ts / 1_000_000) as u64);
        let id2 = ulid_at_ms((ts / 1_000_000) as u64);

        let e1 = make_entry(&id1, ts, "with obj", vec!["vm-1"]);
        let e2 = make_entry(&id2, ts, "no obj", vec!["vm-2"]);

        mgr.append_batch(vec![e1, e2]).unwrap();

        let results = mgr
            .query(Some("vm-1".to_string()), None, None, 100)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "with obj");
    }

    #[test]
    fn query_time_range() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let ts_early = 1_000_000_000_000_000i64; // 1000s in ns
        let ts_late = 2_000_000_000_000_000i64; // 2000s in ns

        let id1 = ulid_at_ms((ts_early / 1_000_000) as u64);
        let id2 = ulid_at_ms((ts_late / 1_000_000) as u64);

        let e1 = make_entry(&id1, ts_early, "early", vec![]);
        let e2 = make_entry(&id2, ts_late, "late", vec![]);

        mgr.append_batch(vec![e1, e2]).unwrap();

        // Query only the later time range
        let results = mgr
            .query(None, Some(1_500_000_000_000_000), None, 100)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "late");
    }

    #[test]
    fn query_respects_limit() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let entries: Vec<LogEntry> = (0..10)
            .map(|i| {
                let ts = (1_700_000_000_000_000_000i64) + i * 1_000_000;
                let id = ulid_at_ms((ts / 1_000_000) as u64);
                make_entry(&id, ts, &format!("log {i}"), vec![])
            })
            .collect();

        mgr.append_batch(entries).unwrap();

        let results = mgr.query(None, None, None, 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn append_batch_rejects_invalid_ulid() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let entry = make_entry("not-a-ulid", 1_000_000_000, "bad", vec![]);
        let result = mgr.append_batch(vec![entry]);
        assert!(result.is_err());
    }

    #[test]
    fn empty_query_returns_empty() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let results = mgr.query(None, None, None, 100).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn multiple_objects_indexed() {
        let dir = TempDir::new().unwrap();
        let mgr = LogManager::new(dir.path()).unwrap();

        let ts = 1_700_000_000_000_000_000i64;
        let id = ulid_at_ms((ts / 1_000_000) as u64);
        let entry = make_entry(&id, ts, "multi-obj", vec!["vm-1", "nic-2"]);

        mgr.append_batch(vec![entry]).unwrap();

        let r1 = mgr
            .query(Some("vm-1".to_string()), None, None, 100)
            .unwrap();
        let r2 = mgr
            .query(Some("nic-2".to_string()), None, None, 100)
            .unwrap();
        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1);
        assert_eq!(r1[0].message, "multi-obj");
        assert_eq!(r2[0].message, "multi-obj");
    }

    #[test]
    fn state_machine_apply() {
        let dir = TempDir::new().unwrap();
        let mgr = Arc::new(LogManager::new(dir.path()).unwrap());

        // Set global (only works once per process — this test must run in isolation
        // or be the first to call init_log_manager in the test binary)
        let _ = LOG_MANAGER.set(mgr.clone());

        let ts = 1_700_000_000_000_000_000i64;
        let id = ulid_at_ms((ts / 1_000_000) as u64);
        let ser_entry = SerializableLogEntry {
            id: id.clone(),
            timestamp_ns: ts,
            message: "via state machine".to_string(),
            level: 1,
            component: "test".to_string(),
            related_object_ids: vec![],
        };

        let mut sm = LogStateMachine;
        let (resp, events) = sm.apply(LogCommand::AppendBatch(vec![ser_entry]));

        assert!(matches!(resp, LogCommandResponse::Ok));
        assert!(events.is_empty());

        let results = mgr.query(None, None, None, 100).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "via state machine");
    }

    #[test]
    fn log_state_machine_serde_roundtrip() {
        let sm = LogStateMachine;
        let encoded = bincode::serialize(&sm).unwrap();
        let decoded: LogStateMachine = bincode::deserialize(&encoded).unwrap();
        // Just verify it doesn't panic — the struct is a unit type
        let _ = decoded;
    }
}
