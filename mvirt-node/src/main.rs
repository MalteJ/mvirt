//! mvirt-node: per-host agent.
//!
//! Dials out to mvirt-cplane over plain TCP and hosts the NodeAgent gRPC service
//! plus byte-level forwarding proxies for the local daemons (vmm/zfs/net) on
//! the dialed socket. The api drives reconciliation by calling those proxied
//! services as a regular gRPC client; the node runs no reconciler logic.

mod agent_impl;
mod proto;
mod proxy;
mod tunnel;

use anyhow::{Context, Result};
use clap::Parser;
use http::Uri;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::agent_impl::NodeAgentService;
use crate::proto::NodeResources;
use crate::proxy::DaemonProxy;
use crate::tunnel::ProxyBundle;

#[derive(Parser, Debug)]
#[command(name = "mvirt-node", version, about)]
struct Args {
    /// API server tunnel endpoint (host:port; the node dials TCP here)
    #[arg(long, default_value = "[::1]:50056")]
    api_endpoint: String,

    /// Stable node id (must match what the api expects). Sent in Identify.
    #[arg(long)]
    node_id: String,

    /// Node display name (defaults to hostname)
    #[arg(long)]
    name: Option<String>,

    /// Advertised address for this node
    #[arg(long, default_value = "0.0.0.0")]
    address: String,

    /// Local mvirt-vmm gRPC endpoint (proxied for VmService + PodService)
    #[arg(long, default_value = "http://[::1]:50051")]
    vmm_endpoint: String,

    /// Local mvirt-zfs gRPC endpoint (proxied for ZfsService)
    #[arg(long, default_value = "http://[::1]:50053")]
    zfs_endpoint: String,

    /// Local mvirt-net/ebpf gRPC endpoint (proxied for NetService)
    #[arg(long, default_value = "http://[::1]:50054")]
    net_endpoint: String,

    /// CPU cores available on this node (auto-detected if absent)
    #[arg(long)]
    cpu_cores: Option<u32>,

    /// Memory in MB available on this node (auto-detected if absent)
    #[arg(long)]
    memory_mb: Option<u64>,

    /// Storage in GB available on this node
    #[arg(long, default_value_t = 0)]
    storage_gb: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mvirt_node=info,tonic=warn,tower=warn,hyper=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();

    let node_name = args.name.unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string())
    });

    let cpu_cores = args.cpu_cores.unwrap_or_else(detect_cpu_cores);
    let memory_mb = args.memory_mb.unwrap_or_else(detect_memory_mb);

    info!(
        node_id = %args.node_id,
        name = %node_name,
        cpu_cores, memory_mb, storage_gb = args.storage_gb,
        "starting mvirt-node"
    );

    let resources = NodeResources {
        cpu_cores,
        memory_mb,
        storage_gb: args.storage_gb,
        available_cpu_cores: cpu_cores,
        available_memory_mb: memory_mb,
        available_storage_gb: args.storage_gb,
    };

    let agent = NodeAgentService {
        node_id: args.node_id.clone(),
        name: node_name,
        address: args.address,
        resources,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let proxies = ProxyBundle {
        vmm: DaemonProxy::new(parse_uri(&args.vmm_endpoint, "vmm_endpoint")?),
        zfs: DaemonProxy::new(parse_uri(&args.zfs_endpoint, "zfs_endpoint")?),
        net: DaemonProxy::new(parse_uri(&args.net_endpoint, "net_endpoint")?),
    };

    tunnel::run(args.api_endpoint, agent, proxies).await
}

fn parse_uri(s: &str, label: &str) -> Result<Uri> {
    s.parse::<Uri>()
        .with_context(|| format!("invalid {label}: {s}"))
}

fn detect_cpu_cores() -> u32 {
    std::fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.matches("processor\t:").count() as u32)
        .unwrap_or(1)
}

fn detect_memory_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| {
                    l.split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<u64>().ok())
                })
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}
