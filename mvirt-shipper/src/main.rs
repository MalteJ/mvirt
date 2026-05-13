mod journal;
mod shipper;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tokio::signal;
use tracing::info;

#[derive(Parser)]
#[command(name = "mvirt-shipper", about = "Ship journald logs to mvirt-log")]
struct Args {
    /// Comma-separated list of systemd units to follow
    #[arg(long)]
    units: String,

    /// mvirt-log gRPC endpoints (comma-separated). Multi-endpoint failover
    /// via `Channel::balance_list`. Reads from `MVIRT_LOG_ENDPOINTS` if set.
    #[arg(
        long,
        env = "MVIRT_LOG_ENDPOINTS",
        default_value = "https://[::1]:50052",
        value_delimiter = ','
    )]
    log_endpoint: Vec<String>,

    /// Directory to persist journal cursors
    #[arg(long, default_value = "/var/lib/mvirt/shipper")]
    cursor_dir: PathBuf,

    /// Path to the internal CA cert (PEM).
    #[arg(
        long,
        env = "MVIRT_TLS_CA",
        default_value = "/var/lib/mvirt-node/ca.pem"
    )]
    tls_ca: PathBuf,

    /// Path to this daemon's client cert (PEM).
    #[arg(
        long,
        env = "MVIRT_TLS_CERT",
        default_value = "/var/lib/mvirt-node/cert.pem"
    )]
    tls_cert: PathBuf,

    /// Path to this daemon's client key (PEM).
    #[arg(
        long,
        env = "MVIRT_TLS_KEY",
        default_value = "/var/lib/mvirt-node/key.pem"
    )]
    tls_key: PathBuf,

    /// Disable mTLS to mvirt-log (talk plain h2c). Dev/loopback only.
    #[arg(long, env = "MVIRT_LOG_INSECURE")]
    log_insecure: bool,

    /// Node identifier to attach as `related_object_ids` on every shipped
    /// entry. Picked up from `MVIRT_NODE_ID` (written by mvirt-node's env
    /// sidecar) so the UI can filter logs to a specific node.
    #[arg(long, env = "MVIRT_NODE_ID")]
    node_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    mvirt_log::tracing_setup::init("info", &[]);

    let args = Args::parse();

    // Ensure cursor directory exists
    tokio::fs::create_dir_all(&args.cursor_dir).await?;

    let units: Vec<String> = args
        .units
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!(units = ?units, endpoints = ?args.log_endpoint, "Starting mvirt-shipper");

    let tls = if args.log_insecure {
        None
    } else {
        Some(
            mvirt_log::tls_config_from_paths(&args.tls_ca, &args.tls_cert, &args.tls_key)
                .map_err(|e| anyhow::anyhow!("TLS config: {e}"))?,
        )
    };

    let mut tasks = Vec::new();

    for unit in &units {
        let (tx, rx) = tokio::sync::mpsc::channel(256);

        let stream = journal::JournalStream::new(unit, &args.cursor_dir);
        let unit_clone = unit.clone();

        // Spawn journal reader
        tasks.push(tokio::spawn(async move {
            loop {
                if let Err(e) = stream.run(tx.clone()).await {
                    tracing::error!(unit = %unit_clone, error = %e, "Journal stream failed, restarting");
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }));

        // Spawn shipper (gRPC forwarder)
        let endpoints = args.log_endpoint.clone();
        let unit_name = unit.clone();
        let tls = tls.clone();
        let node_id = args.node_id.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = shipper::run_shipper(endpoints, tls, unit_name, node_id, rx).await {
                tracing::error!(error = %e, "Shipper failed");
            }
        }));
    }

    // Wait for shutdown signal
    signal::ctrl_c().await?;
    info!("Shutting down");

    for task in tasks {
        task.abort();
    }

    Ok(())
}
