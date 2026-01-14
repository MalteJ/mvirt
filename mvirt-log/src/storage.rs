use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::mvirt::log::LogEntry;

pub struct LogManager {
    conn: Mutex<Connection>,
}

impl LogManager {
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self> {
        let db_path = data_dir.as_ref().join("logs.db");
        let conn = Connection::open(&db_path)?;

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS logs (
                id INTEGER PRIMARY KEY,
                timestamp_ns INTEGER NOT NULL,
                message TEXT NOT NULL,
                level INTEGER NOT NULL,
                component TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS log_objects (
                log_id INTEGER NOT NULL,
                object_id TEXT NOT NULL,
                PRIMARY KEY (log_id, object_id),
                FOREIGN KEY (log_id) REFERENCES logs(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_logs_timestamp ON logs(timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_log_objects_object ON log_objects(object_id);
            ",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn append(&self, mut entry: LogEntry) -> Result<String> {
        if entry.timestamp_ns == 0 {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
            entry.timestamp_ns = now.as_nanos() as i64;
        }

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO logs (timestamp_ns, message, level, component)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.timestamp_ns,
                entry.message,
                entry.level,
                entry.component,
            ],
        )?;

        let id = conn.last_insert_rowid();

        for obj_id in &entry.related_object_ids {
            conn.execute(
                "INSERT INTO log_objects (log_id, object_id) VALUES (?1, ?2)",
                params![id, obj_id],
            )?;
        }

        Ok(id.to_string())
    }

    pub fn query(
        &self,
        object_id: Option<String>,
        start_ns: Option<i64>,
        end_ns: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LogEntry>> {
        let conn = self.conn.lock().unwrap();
        let start = start_ns.unwrap_or(0);
        let end = end_ns.unwrap_or(i64::MAX);

        let rows: Vec<(i64, i64, String, i32, String)> = if let Some(obj) = object_id {
            let mut stmt = conn.prepare(
                "SELECT l.id, l.timestamp_ns, l.message, l.level, l.component
                 FROM logs l
                 JOIN log_objects lo ON l.id = lo.log_id
                 WHERE lo.object_id = ?1
                   AND l.timestamp_ns >= ?2
                   AND l.timestamp_ns <= ?3
                 ORDER BY l.timestamp_ns
                 LIMIT ?4",
            )?;
            let mapped = stmt.query_map(params![obj, start, end, limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp_ns, message, level, component
                 FROM logs
                 WHERE timestamp_ns >= ?1 AND timestamp_ns <= ?2
                 ORDER BY timestamp_ns
                 LIMIT ?3",
            )?;
            let mapped = stmt.query_map(params![start, end, limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };

        let mut results = Vec::with_capacity(rows.len());
        for (id, timestamp_ns, message, level, component) in rows {
            let mut obj_stmt =
                conn.prepare("SELECT object_id FROM log_objects WHERE log_id = ?1")?;
            let related: Vec<String> = obj_stmt
                .query_map(params![id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;

            results.push(LogEntry {
                id: id.to_string(),
                timestamp_ns,
                message,
                level,
                component,
                related_object_ids: related,
            });
        }

        Ok(results)
    }
}
