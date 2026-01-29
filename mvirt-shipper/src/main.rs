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

    /// mvirt-log gRPC endpoint
    #[arg(long, default_value = "http://[::1]:50052")]
    log_endpoint: String,

    /// Directory to persist journal cursors
    #[arg(long, default_value = "/var/lib/mvirt/shipper")]
    cursor_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    // Ensure cursor directory exists
    tokio::fs::create_dir_all(&args.cursor_dir).await?;

    let units: Vec<String> = args
        .units
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!(units = ?units, endpoint = %args.log_endpoint, "Starting mvirt-shipper");

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
        let endpoint = args.log_endpoint.clone();
        let unit_name = unit.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = shipper::run_shipper(endpoint, unit_name, rx).await {
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
