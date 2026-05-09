//! Reverse-tunnel dialer.
//!
//! mvirt-node TCP-connects to the api, then serves the NodeAgent gRPC service
//! plus the daemon proxies on the dialed socket. The gRPC roles invert at this
//! point: the api is the client, the node is the server, but the TCP direction
//! is outbound (NAT-friendly).

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tonic::transport::Server;
use tracing::{info, warn};

use crate::agent_impl::NodeAgentService;
use crate::proto::node::node_agent_server::NodeAgentServer;
use crate::proxy::{
    DaemonProxy, NetServiceProxy, PodServiceProxy, VmServiceProxy, ZfsServiceProxy,
};

#[derive(Clone)]
pub struct ProxyBundle {
    pub vmm: DaemonProxy,
    pub zfs: DaemonProxy,
    pub net: DaemonProxy,
}

/// Reconnect loop: dial the api, host gRPC services on the dialed socket
/// until it breaks, then back off and retry.
pub async fn run(
    api_endpoint: String,
    agent: NodeAgentService,
    proxies: ProxyBundle,
) -> Result<()> {
    let backoff = Duration::from_secs(5);
    loop {
        match dial_and_serve(&api_endpoint, agent.clone(), proxies.clone()).await {
            Ok(()) => info!("tunnel closed by api, reconnecting..."),
            Err(e) => warn!(error = %e, "tunnel connection failed; retrying"),
        }
        tokio::time::sleep(backoff).await;
    }
}

async fn dial_and_serve(
    api_endpoint: &str,
    agent: NodeAgentService,
    proxies: ProxyBundle,
) -> Result<()> {
    let target = api_endpoint
        .strip_prefix("http://")
        .or_else(|| api_endpoint.strip_prefix("https://"))
        .unwrap_or(api_endpoint);

    info!(%target, "dialing api for reverse tunnel");
    let sock = TcpStream::connect(target)
        .await
        .with_context(|| format!("connecting to {target}"))?;
    sock.set_nodelay(true).ok();
    info!(peer = %sock.peer_addr()?, "tunnel established, serving NodeAgent + daemon proxies");

    // Yield the dialed socket once, then keep the stream open forever. If we
    // used `stream::once` alone, tonic's accept loop would exit as soon as the
    // stream is exhausted (returning Ok immediately) even though the spawned
    // serve_connection task is still driving the H/2 connection — the dialer
    // would then loop back and reconnect every iteration.
    use futures::StreamExt;
    let incoming = futures::stream::once(async move { Ok::<_, std::io::Error>(sock) })
        .chain(futures::stream::pending());

    Server::builder()
        .add_service(NodeAgentServer::new(agent))
        .add_service(VmServiceProxy(proxies.vmm.clone()))
        .add_service(PodServiceProxy(proxies.vmm))
        .add_service(ZfsServiceProxy(proxies.zfs))
        .add_service(NetServiceProxy(proxies.net))
        .serve_with_incoming(incoming)
        .await
        .context("serving inverted tunnel")?;

    Ok(())
}
