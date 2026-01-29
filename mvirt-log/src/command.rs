use serde::{Deserialize, Serialize};

use crate::LogEntry;

/// Serde-friendly mirror of the proto LogEntry.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SerializableLogEntry {
    pub id: String,
    pub timestamp_ns: i64,
    pub message: String,
    pub level: i32,
    pub component: String,
    pub related_object_ids: Vec<String>,
}

impl From<LogEntry> for SerializableLogEntry {
    fn from(e: LogEntry) -> Self {
        Self {
            id: e.id,
            timestamp_ns: e.timestamp_ns,
            message: e.message,
            level: e.level,
            component: e.component,
            related_object_ids: e.related_object_ids,
        }
    }
}

impl From<SerializableLogEntry> for LogEntry {
    fn from(e: SerializableLogEntry) -> Self {
        Self {
            id: e.id,
            timestamp_ns: e.timestamp_ns,
            message: e.message,
            level: e.level,
            component: e.component,
            related_object_ids: e.related_object_ids,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LogCommand {
    AppendBatch(Vec<SerializableLogEntry>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum LogCommandResponse {
    #[default]
    Ok,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_log_entry() -> LogEntry {
        LogEntry {
            id: "01J0000000DEADBEEF000000".to_string(),
            timestamp_ns: 1_700_000_000_000_000_000,
            message: "VM created".to_string(),
            level: 1,
            component: "vmm".to_string(),
            related_object_ids: vec!["vm-1".to_string(), "nic-2".to_string()],
        }
    }

    #[test]
    fn serializable_entry_roundtrip() {
        let entry = sample_log_entry();
        let ser: SerializableLogEntry = entry.clone().into();
        let back: LogEntry = ser.into();

        assert_eq!(back.id, entry.id);
        assert_eq!(back.timestamp_ns, entry.timestamp_ns);
        assert_eq!(back.message, entry.message);
        assert_eq!(back.level, entry.level);
        assert_eq!(back.component, entry.component);
        assert_eq!(back.related_object_ids, entry.related_object_ids);
    }

    #[test]
    fn log_command_bincode_roundtrip() {
        let entry = sample_log_entry();
        let cmd = LogCommand::AppendBatch(vec![entry.into()]);

        let encoded = bincode::serialize(&cmd).unwrap();
        let decoded: LogCommand = bincode::deserialize(&encoded).unwrap();

        match decoded {
            LogCommand::AppendBatch(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].message, "VM created");
                assert_eq!(entries[0].related_object_ids, vec!["vm-1", "nic-2"]);
            }
        }
    }

    #[test]
    fn log_command_response_default() {
        assert!(matches!(
            LogCommandResponse::default(),
            LogCommandResponse::Ok
        ));
    }
}
