//! mvirt-node: per-host agent.
//!
//! Bootstraps itself against the cplane via the onboarding token on first
//! boot (ADR-0006), persists its mTLS client cert to `state-dir`, and then
//! dials the cplane's reverse-tunnel endpoint on every start to host the
//! NodeAgent gRPC service + byte-level proxies for the local daemons
//! (vmm / zfs / net). Identity is pinned to the client cert; the cplane
//! drives reconciliation by calling those proxied services as a regular
//! gRPC client.

mod agent_impl;
mod onboarding;
mod proto;
mod proxy;
mod tunnel;

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use http::Uri;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::agent_impl::NodeAgentService;
use crate::proto::NodeResources;
use crate::proxy::DaemonProxy;
use crate::tunnel::ProxyBundle;

#[derive(Parser, Debug)]
#[command(name = "mvirt-node", version, about)]
struct Args {
    /// cplane REST endpoint, e.g. https://api.mvirt.io. Used only on first
    /// boot for the bootstrap exchange.
    #[arg(long, default_value = "http://[::1]:8080")]
    api_endpoint: String,

    /// cplane reverse-tunnel endpoint (host:port). The node dials TCP here
    /// and performs an mTLS handshake. Falls back to the value in
    /// `state.toml` after first boot.
    #[arg(long)]
    tunnel_endpoint: Option<String>,

    /// State dir for the persistent on-disk PKI material.
    #[arg(long, default_value = "/var/lib/mvirt-node")]
    state_dir: PathBuf,

    /// One-time onboarding token. Required on first boot; ignored once the
    /// state-dir contains a signed cert. Treat as a secret.
    #[arg(long, env = "MVIRT_NODE_ONBOARDING_TOKEN")]
    onboarding_token: Option<String>,

    /// Skip TLS verification on the bootstrap REST call. Dev/test only — the
    /// returned CA cert is what pins the long-lived tunnel anyway.
    #[arg(long)]
    insecure_skip_tls_verify: bool,

    /// Node display name (defaults to hostname)
    #[arg(long)]
    name: Option<String>,

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
    let agent_version = env!("CARGO_PKG_VERSION");

    // 1. PKI: load from disk, or bootstrap with the onboarding token.
    let pki = match onboarding::load_from_disk(&args.state_dir)? {
        Some(pki) => {
            info!(
                state_dir = %args.state_dir.display(),
                node_id = %pki.state.node_id,
                cluster_slug = %pki.state.cluster_slug,
                "loaded existing PKI material"
            );
            pki
        }
        None => {
            let token = args.onboarding_token.as_deref().ok_or_else(|| {
                anyhow!(
                    "no PKI in {} and --onboarding-token not provided; cannot bootstrap",
                    args.state_dir.display()
                )
            })?;
            let pki = onboarding::bootstrap(
                &args.api_endpoint,
                token,
                args.tunnel_endpoint.clone(),
                args.insecure_skip_tls_verify,
                agent_version,
            )
            .await
            .context("onboarding bootstrap")?;
            onboarding::persist(&args.state_dir, &pki).context("persist PKI to disk")?;
            info!(
                node_id = %pki.state.node_id,
                cluster_slug = %pki.state.cluster_slug,
                "bootstrap complete; token consumed"
            );
            pki
        }
    };

    // 2. Tunnel endpoint: arg wins, else fall back to state.toml's value.
    let tunnel_endpoint = args
        .tunnel_endpoint
        .clone()
        .unwrap_or_else(|| pki.state.tunnel_endpoint.clone());

    // 3. Identity is cert-pinned (cplane-assigned). Local display name is
    //    still useful for logs.
    let node_name = args.name.clone().unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string())
    });
    let cpu_cores = args.cpu_cores.unwrap_or_else(detect_cpu_cores);
    let memory_mb = args.memory_mb.unwrap_or_else(detect_memory_mb);

    info!(
        node_id = %pki.state.node_id,
        name = %node_name,
        cluster_slug = %pki.state.cluster_slug,
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
        node_id: pki.state.node_id.clone(),
        name: node_name,
        address: String::new(),
        resources,
        agent_version: agent_version.to_string(),
    };
    let proxies = ProxyBundle {
        vmm: DaemonProxy::new(parse_uri(&args.vmm_endpoint, "vmm_endpoint")?),
        zfs: DaemonProxy::new(parse_uri(&args.zfs_endpoint, "zfs_endpoint")?),
        net: DaemonProxy::new(parse_uri(&args.net_endpoint, "net_endpoint")?),
    };

    if let Err(e) = tunnel::run(tunnel_endpoint, pki, agent, proxies).await {
        warn!(error = %e, "tunnel loop terminated");
        return Err(e);
    }
    Ok(())
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
