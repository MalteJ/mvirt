use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// A single parsed journal entry
pub struct JournalEntry {
    pub message: String,
    pub priority: u8,
    pub cursor: String,
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
            format!("-u {}", self.unit),
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

    let message = v.get("MESSAGE")?.as_str()?.to_string();
    let cursor = v.get("__CURSOR")?.as_str()?.to_string();
    let priority = v
        .get("PRIORITY")
        .and_then(|p| p.as_str())
        .and_then(|p| p.parse::<u8>().ok())
        .unwrap_or(6); // default to INFO

    Some(JournalEntry {
        message,
        priority,
        cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_journal_line() {
        let line = r#"{"MESSAGE":"VM started","PRIORITY":"3","__CURSOR":"s=abc;i=1"}"#;
        let entry = parse_journal_line(line).unwrap();
        assert_eq!(entry.message, "VM started");
        assert_eq!(entry.priority, 3);
        assert_eq!(entry.cursor, "s=abc;i=1");
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
