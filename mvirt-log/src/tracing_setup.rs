//! Shared `tracing_subscriber` initialization for mvirt daemons.
//!
//! Picks the format based on whether stderr is a terminal:
//! - TTY (interactive `cargo run`, dev shells): human-friendly text with
//!   timestamps and colors.
//! - Non-TTY (systemd-managed services): JSON, so the shipper can lift
//!   `level`, `target`, and structured fields out of journald's
//!   `MESSAGE` field instead of regex-mauling text output.

use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

/// Initialize tracing with format auto-detection. Builds an
/// `EnvFilter` from `RUST_LOG`, falling back to `default_directive`
/// (e.g. `"mvirt_vmm=info"`).
///
/// Pass any additional directives via `extra_directives` (e.g.
/// `["h2=warn", "tonic=warn"]`).
pub fn init(default_directive: &str, extra_directives: &[&str]) {
    let mut filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive));
    for d in extra_directives {
        if let Ok(parsed) = d.parse() {
            filter = filter.add_directive(parsed);
        }
    }

    if std::io::stderr().is_terminal() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .init();
    }
}
