use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// A single parsed journal entry
pub struct JournalEntry {
    pub message: String,
    pub priority: u8,
    pub cursor: String,
    /// Original event time in nanoseconds since Unix epoch, taken from
    /// journald's `__REALTIME_TIMESTAMP` (microseconds, multiplied here).
    /// Preserves the actual emit time across the shipper hop instead of
    /// letting the server stamp "now" on receipt.
    pub timestamp_ns: i64,
    /// Tracing-level string when MESSAGE was structured tracing JSON
    /// (DEBUG/INFO/WARN/ERROR/TRACE). None for non-mvirt units.
    /// Overrides `priority` mapping at the shipper layer.
    pub tracing_level: Option<String>,
}

/// Spawn journalctl for a unit and yield parsed entries
pub struct JournalStream {
    unit: String,
    cursor_path: PathBuf,
}

impl JournalStream {
    pub fn new(unit: &str, cursor_dir: &Path) -> Self {
        let cursor_path = cursor_dir.join(format!("{unit}.cursor"));
        Self {
            unit: unit.to_string(),
            cursor_path,
        }
    }

    pub fn load_cursor(&self) -> Option<String> {
        std::fs::read_to_string(&self.cursor_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn save_cursor(&self, cursor: &str) -> Result<()> {
        std::fs::write(&self.cursor_path, cursor)
            .with_context(|| format!("Failed to save cursor to {}", self.cursor_path.display()))
    }

    pub async fn run(&self, sender: tokio::sync::mpsc::Sender<JournalEntry>) -> Result<()> {
        let mut args = vec![
            "--output=json".to_string(),
            "-u".to_string(),
            self.unit.clone(),
            "--follow".to_string(),
        ];

        if let Some(cursor) = self.load_cursor() {
            args.push(format!("--after-cursor={cursor}"));
        }

        let mut child = Command::new("journalctl")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to spawn journalctl for {}", self.unit))?;

        let stdout = child.stdout.take().context("No stdout from journalctl")?;
        let mut lines = BufReader::new(stdout).lines();
        let mut line_count: u64 = 0;

        while let Some(line) = lines.next_line().await? {
            let entry = match parse_journal_line(&line) {
                Some(e) => e,
                None => continue,
            };

            let cursor = entry.cursor.clone();
            if sender.send(entry).await.is_err() {
                break;
            }

            line_count += 1;
            if line_count.is_multiple_of(100) {
                self.save_cursor(&cursor)?;
            }
        }

        // Save final cursor
        if let Ok(status) = child.wait().await {
            tracing::debug!(unit = %self.unit, status = %status, "journalctl exited");
        }

        Ok(())
    }
}

pub fn parse_journal_line(line: &str) -> Option<JournalEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;

    let raw_message = message_to_string(v.get("MESSAGE")?)?;
    let cursor = v.get("__CURSOR")?.as_str()?.to_string();
    let priority = v
        .get("PRIORITY")
        .and_then(|p| p.as_str())
        .and_then(|p| p.parse::<u8>().ok())
        .unwrap_or(6); // default to INFO

    let timestamp_ns = v
        .get("__REALTIME_TIMESTAMP")
        .and_then(|t| t.as_str())
        .and_then(|t| t.parse::<i64>().ok())
        .map(|us| us.saturating_mul(1000))
        .unwrap_or(0);

    // Two paths: daemons under systemd emit JSON via tracing_subscriber's
    // `.json()` formatter — we parse it for structured fields. Anything
    // else (systemd init messages, other units) goes through unchanged.
    let (message, tracing_level) = match parse_tracing_json(&raw_message) {
        Some(t) => (t.message, Some(t.level)),
        None => (raw_message, None),
    };

    Some(JournalEntry {
        message,
        priority,
        cursor,
        timestamp_ns,
        tracing_level,
    })
}

/// Subset of the `tracing_subscriber` JSON format we care about.
/// The format emits `{timestamp, level, fields:{message, ...}, target}`;
/// we want the human-readable message + any structured kv fields collapsed
/// inline (e.g. `addr=[::1]:50051`), preceded by the module target so
/// operators get the context they're used to from text-format logs.
struct TracingJson {
    message: String,
    level: String,
}

fn parse_tracing_json(raw: &str) -> Option<TracingJson> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = v.as_object()?;
    let level = obj.get("level")?.as_str()?.to_string();
    let target = obj.get("target").and_then(|t| t.as_str()).unwrap_or("");
    let fields = obj.get("fields")?.as_object()?;
    let message_text = fields.get("message").and_then(|m| m.as_str()).unwrap_or("");

    let mut extras: Vec<String> = Vec::new();
    for (k, val) in fields {
        if k == "message" {
            continue;
        }
        let rendered = match val {
            serde_json::Value::String(s) => s.clone(),
            _ => val.to_string(),
        };
        extras.push(format!("{k}={rendered}"));
    }

    let mut out = String::new();
    if !target.is_empty() {
        out.push_str(target);
        out.push_str(": ");
    }
    out.push_str(message_text);
    if !extras.is_empty() {
        out.push(' ');
        out.push_str(&extras.join(" "));
    }

    Some(TracingJson { message: out, level })
}

/// journald serializes `MESSAGE` as a JSON string when it's valid UTF-8 and
/// printable, but as `[u8, ...]` array when it contains control characters
/// (notably ANSI color escapes from tracing-subscriber). Both forms are
/// real entries we want to ship — strip ANSI and decode as UTF-8.
fn message_to_string(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(strip_ansi(s));
    }
    let arr = v.as_array()?;
    let bytes: Vec<u8> = arr.iter().filter_map(|n| n.as_u64().map(|x| x as u8)).collect();
    String::from_utf8(bytes).ok().map(|s| strip_ansi(&s))
}

/// Strip CSI escape sequences (ESC[...). Cheap regex-free pass; doesn't
/// claim to be a full terminfo parser, just enough to clean up tracing
/// output before it lands in audit logs.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for term in chars.by_ref() {
                if term.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_journal_line() {
        let line = r#"{"MESSAGE":"VM started","PRIORITY":"3","__CURSOR":"s=abc;i=1","__REALTIME_TIMESTAMP":"1778679565708539"}"#;
        let entry = parse_journal_line(line).unwrap();
        assert_eq!(entry.message, "VM started");
        assert_eq!(entry.priority, 3);
        assert_eq!(entry.cursor, "s=abc;i=1");
        assert_eq!(entry.timestamp_ns, 1_778_679_565_708_539_000);
    }

    #[test]
    fn parse_leaves_non_tracing_messages_alone() {
        let line = r#"{"MESSAGE":"Started mvirt-shipper.service","PRIORITY":"6","__CURSOR":"s=x;i=1"}"#;
        let entry = parse_journal_line(line).unwrap();
        assert_eq!(entry.message, "Started mvirt-shipper.service");
        assert!(entry.tracing_level.is_none());
    }

    #[test]
    fn parse_extracts_tracing_json_message_and_level() {
        // MESSAGE is itself a JSON string (escaped) produced by
        // tracing_subscriber::fmt().json() under systemd.
        let inner = r#"{"timestamp":"2026-05-13T13:59:15Z","level":"INFO","fields":{"message":"Starting gRPC server","addr":"[::1]:50051"},"target":"mvirt_vmm"}"#;
        let outer = serde_json::json!({
            "MESSAGE": inner,
            "PRIORITY": "6",
            "__CURSOR": "s=x;i=1",
        })
        .to_string();
        let entry = parse_journal_line(&outer).unwrap();
        assert_eq!(
            entry.message,
            "mvirt_vmm: Starting gRPC server addr=[::1]:50051"
        );
        assert_eq!(entry.tracing_level.as_deref(), Some("INFO"));
    }

    #[test]
    fn parse_missing_priority_defaults_to_info() {
        let line = r#"{"MESSAGE":"hello","__CURSOR":"s=x;i=2"}"#;
        let entry = parse_journal_line(line).unwrap();
        assert_eq!(entry.priority, 6);
    }

    #[test]
    fn parse_missing_message_returns_none() {
        let line = r#"{"PRIORITY":"3","__CURSOR":"s=x;i=2"}"#;
        assert!(parse_journal_line(line).is_none());
    }

    #[test]
    fn parse_message_as_byte_array() {
        // journald serializes MESSAGE as bytes when it contains control
        // characters (e.g. ANSI color escapes from tracing-subscriber).
        // "hello" in bytes:
        let line = r#"{"MESSAGE":[104,101,108,108,111],"__CURSOR":"s=x;i=1","PRIORITY":"6"}"#;
        let entry = parse_journal_line(line).unwrap();
        assert_eq!(entry.message, "hello");
    }

    #[test]
    fn strip_ansi_csi() {
        let s = "\x1b[32m INFO\x1b[0m hello";
        assert_eq!(strip_ansi(s), " INFO hello");
    }

    #[test]
    fn parse_missing_cursor_returns_none() {
        let line = r#"{"MESSAGE":"hello","PRIORITY":"3"}"#;
        assert!(parse_journal_line(line).is_none());
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_journal_line("not json").is_none());
    }

    #[test]
    fn cursor_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let stream = JournalStream::new("mvirt-vmm", dir.path());

        assert!(stream.load_cursor().is_none());

        stream.save_cursor("s=abc;i=42").unwrap();
        assert_eq!(stream.load_cursor().unwrap(), "s=abc;i=42");
    }

    #[test]
    fn cursor_path_uses_unit_name() {
        let dir = tempfile::tempdir().unwrap();
        let stream = JournalStream::new("mvirt-zfs", dir.path());
        assert_eq!(stream.cursor_path, dir.path().join("mvirt-zfs.cursor"));
    }
}
