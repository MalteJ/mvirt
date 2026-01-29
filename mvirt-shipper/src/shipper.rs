use anyhow::Result;
use mvirt_log::{LogEntry, LogLevel, LogRequest, LogServiceClient};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::journal::JournalEntry;

/// Maps journald PRIORITY (0-7) to LogLevel
pub fn map_priority(priority: u8) -> LogLevel {
    match priority {
        0 => LogLevel::Emergency,
        1 => LogLevel::Alert,
        2 => LogLevel::Critical,
        3 => LogLevel::Error,
        4 => LogLevel::Warn,
        5 => LogLevel::Notice,
        6 => LogLevel::Info,
        7 => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

/// Strip "mvirt-" prefix from unit name to get component name
pub fn component_from_unit(unit: &str) -> String {
    unit.strip_prefix("mvirt-").unwrap_or(unit).to_string()
}

/// Forward journal entries to mvirt-log via gRPC
pub async fn run_shipper(
    log_endpoint: String,
    unit: String,
    mut receiver: mpsc::Receiver<JournalEntry>,
) -> Result<()> {
    let component = component_from_unit(&unit);

    let mut client: Option<LogServiceClient<tonic::transport::Channel>> = None;

    while let Some(entry) = receiver.recv().await {
        // Lazy connect / reconnect
        if client.is_none() {
            match LogServiceClient::connect(log_endpoint.clone()).await {
                Ok(c) => {
                    debug!(endpoint = %log_endpoint, "Connected to mvirt-log");
                    client = Some(c);
                }
                Err(e) => {
                    warn!(error = %e, "Failed to connect to mvirt-log, dropping entry");
                    continue;
                }
            }
        }

        let level = map_priority(entry.priority);
        let request = LogRequest {
            entry: Some(LogEntry {
                id: String::new(),
                timestamp_ns: 0,
                message: entry.message,
                level: level as i32,
                component: component.clone(),
                related_object_ids: vec![],
            }),
        };

        if let Some(ref mut c) = client {
            if let Err(e) = c.log(request).await {
                warn!(error = %e, "Failed to send log to mvirt-log, reconnecting");
                client = None;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_mapping() {
        assert_eq!(map_priority(0), LogLevel::Emergency);
        assert_eq!(map_priority(1), LogLevel::Alert);
        assert_eq!(map_priority(2), LogLevel::Critical);
        assert_eq!(map_priority(3), LogLevel::Error);
        assert_eq!(map_priority(4), LogLevel::Warn);
        assert_eq!(map_priority(5), LogLevel::Notice);
        assert_eq!(map_priority(6), LogLevel::Info);
        assert_eq!(map_priority(7), LogLevel::Debug);
    }

    #[test]
    fn unknown_priority_defaults_to_info() {
        assert_eq!(map_priority(255), LogLevel::Info);
        assert_eq!(map_priority(8), LogLevel::Info);
    }

    #[test]
    fn component_strips_prefix() {
        assert_eq!(component_from_unit("mvirt-vmm"), "vmm");
        assert_eq!(component_from_unit("mvirt-zfs"), "zfs");
        assert_eq!(component_from_unit("mvirt-ebpf"), "ebpf");
    }

    #[test]
    fn component_keeps_non_mvirt_units() {
        assert_eq!(component_from_unit("nginx"), "nginx");
        assert_eq!(component_from_unit("systemd-journald"), "systemd-journald");
    }
}
