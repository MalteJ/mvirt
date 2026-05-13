use anyhow::{anyhow, Result};
use mvirt_log::{LogEntry, LogLevel, LogRequest, LogServiceClient};
use tokio::sync::mpsc;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tracing::warn;

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

/// Map a tracing-subscriber `level` string (case-insensitive) to LogLevel.
/// TRACE is folded into DEBUG since the proto enum has no separate trace
/// rung.
pub fn map_tracing_level(s: &str) -> Option<LogLevel> {
    match s.to_ascii_uppercase().as_str() {
        "ERROR" => Some(LogLevel::Error),
        "WARN" => Some(LogLevel::Warn),
        "INFO" => Some(LogLevel::Info),
        "DEBUG" | "TRACE" => Some(LogLevel::Debug),
        _ => None,
    }
}

/// Strip "mvirt-" prefix from unit name to get component name
pub fn component_from_unit(unit: &str) -> String {
    unit.strip_prefix("mvirt-").unwrap_or(unit).to_string()
}

fn build_channel(endpoints: &[String], tls: &Option<ClientTlsConfig>) -> Result<Channel> {
    if endpoints.is_empty() {
        return Err(anyhow!("no log endpoints configured"));
    }
    let mut parsed: Vec<Endpoint> = Vec::with_capacity(endpoints.len());
    for url in endpoints {
        let mut ep = Endpoint::from_shared(url.clone())
            .map_err(|e| anyhow!("invalid endpoint {url}: {e}"))?;
        if let Some(t) = tls.as_ref() {
            ep = ep
                .tls_config(t.clone())
                .map_err(|e| anyhow!("tls config for {url}: {e}"))?;
        }
        parsed.push(ep);
    }
    Ok(Channel::balance_list(parsed.into_iter()))
}

/// Forward journal entries to mvirt-log via gRPC.
///
/// Channel is load-balanced across `endpoints` and handles reconnect
/// internally — the shipper just retries the RPC if it returns an error.
/// Every entry carries `node_id` (if present) in `related_object_ids` so
/// the UI can filter logs to a specific node.
pub async fn run_shipper(
    endpoints: Vec<String>,
    tls: Option<ClientTlsConfig>,
    unit: String,
    node_id: Option<String>,
    mut receiver: mpsc::Receiver<JournalEntry>,
) -> Result<()> {
    let component = component_from_unit(&unit);
    let channel = build_channel(&endpoints, &tls)?;
    let mut client = LogServiceClient::new(channel);

    while let Some(entry) = receiver.recv().await {
        // tracing-level (from JSON-formatted daemon logs) beats syslog
        // PRIORITY because journald narrows tracing's DEBUG→INFO mapping
        // before shipper sees it; trust the daemon's own level when present.
        let level = entry
            .tracing_level
            .as_deref()
            .and_then(map_tracing_level)
            .unwrap_or_else(|| map_priority(entry.priority));
        let related_object_ids = match &node_id {
            Some(id) => vec![id.clone()],
            None => vec![],
        };
        let request = LogRequest {
            entry: Some(LogEntry {
                id: String::new(),
                timestamp_ns: entry.timestamp_ns,
                message: entry.message,
                level: level as i32,
                component: component.clone(),
                related_object_ids,
            }),
        };
        if let Err(e) = client.log(request).await {
            warn!(error = %e, "Failed to send log to mvirt-log");
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
