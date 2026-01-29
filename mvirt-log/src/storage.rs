use anyhow::Result;
use prost::Message;
use redb::{Database, TableDefinition};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use mvirt_log::LogEntry;

const TABLE_LOGS: TableDefinition<u128, &[u8]> = TableDefinition::new("logs");
const TABLE_IDX_OBJECT: TableDefinition<(&str, u128), ()> = TableDefinition::new("idx_object");
const TABLE_IDX_COMPONENT: TableDefinition<(&str, i32, u128), ()> =
    TableDefinition::new("idx_component");

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

    pub fn append_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut logs = txn.open_table(TABLE_LOGS)?;
            let mut idx_obj = txn.open_table(TABLE_IDX_OBJECT)?;
            let mut idx_comp = txn.open_table(TABLE_IDX_COMPONENT)?;

            for mut entry in entries {
                if entry.timestamp_ns == 0 {
                    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
                    entry.timestamp_ns = now.as_nanos() as i64;
                }

                let ms = (entry.timestamp_ns / 1_000_000) as u64;
                let ulid = Ulid::from_parts(ms, rand::random());
                entry.id = ulid.to_string();

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
