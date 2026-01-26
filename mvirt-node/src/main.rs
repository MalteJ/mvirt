//! mvirt-node: Node agent for mvirt hypervisor reconciliation.
//!
//! This daemon runs on each hypervisor host and:
//! - Registers with the mvirt-api cluster
//! - Sends periodic heartbeats
//! - Watches for spec changes via gRPC streaming
//! - Reconciles desired state with local daemons (mvirt-net, mvirt-vmm, mvirt-zfs)
//! - Reports status updates back to the API

// Allow unused code for now - reconcilers and clients are stubs
#![allow(dead_code, unused_imports, unused_variables)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tokio::sync::RwLock;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod agent;
mod clients;
mod proto;
mod reconciler;

use agent::NodeAgent;

/// mvirt Node Agent
#[derive(Parser, Debug)]
#[command(name = "mvirt-node", version, about)]
struct Args {
    /// API server endpoint (e.g., http://[::1]:50056)
    #[arg(long, default_value = "http://[::1]:50056")]
    api_endpoint: String,

    /// Node name (defaults to hostname)
    #[arg(long)]
    name: Option<String>,

    /// Node ID (auto-generated if not provided)
    #[arg(long)]
    node_id: Option<String>,

    /// Heartbeat interval in seconds
    #[arg(long, default_value = "10")]
    heartbeat_interval: u64,

    /// mvirt-net gRPC endpoint
    #[arg(long, default_value = "http://[::1]:50051")]
    net_endpoint: String,

    /// mvirt-vmm gRPC endpoint
    #[arg(long, default_value = "http://[::1]:50053")]
    vmm_endpoint: String,

    /// mvirt-zfs gRPC endpoint
    #[arg(long, default_value = "http://[::1]:50054")]
    zfs_endpoint: String,

    /// mvirt-log endpoint for audit logging
    #[arg(long, default_value = "http://[::1]:50052")]
    log_endpoint: String,

    /// CPU cores available on this node
    #[arg(long)]
    cpu_cores: Option<u32>,

    /// Memory in MB available on this node
    #[arg(long)]
    memory_mb: Option<u64>,

    /// Storage in GB available on this node
    #[arg(long)]
    storage_gb: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mvirt_node=info,tonic=warn,tower=warn,hyper=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();

    // Get node name from args or hostname
    let node_name = args.name.unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string())
    });

    info!("Starting mvirt-node agent: {}", node_name);
    info!("API endpoint: {}", args.api_endpoint);

    // Create audit logger
    let audit = Arc::new(crate::agent::NodeAuditLogger::new(&args.log_endpoint));

    // Create the agent
    let agent = Arc::new(RwLock::new(NodeAgent::new(
        args.api_endpoint.clone(),
        node_name.clone(),
        args.node_id,
        Duration::from_secs(args.heartbeat_interval),
        agent::NodeResources {
            cpu_cores: args.cpu_cores.unwrap_or(0),
            memory_mb: args.memory_mb.unwrap_or(0),
            storage_gb: args.storage_gb.unwrap_or(0),
            available_cpu_cores: args.cpu_cores.unwrap_or(0),
            available_memory_mb: args.memory_mb.unwrap_or(0),
            available_storage_gb: args.storage_gb.unwrap_or(0),
        },
        audit,
    )));

    // Run the agent loop with reconnection
    loop {
        let agent_clone = Arc::clone(&agent);

        match run_agent(agent_clone).await {
            Ok(()) => {
                info!("Agent loop completed normally");
                break;
            }
            Err(e) => {
                error!("Agent error: {}. Reconnecting in 5 seconds...", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    Ok(())
}

async fn run_agent(agent: Arc<RwLock<NodeAgent>>) -> Result<()> {
    let mut agent = agent.write().await;
    agent.run().await
}
