//! Reverse-tunnel dialer — mTLS edition (ADR-0006).
//!
//! Dials the cplane on TCP, performs an mTLS handshake using the node's
//! client cert + the internal CA root, then hosts the NodeAgent gRPC
//! service + daemon proxies on the TLS stream. The TCP direction is still
//! outbound (NAT-friendly per ADR-0003); only the bytes are wrapped in TLS
//! and the cplane authenticates us via our client cert.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as StdContext, Poll};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tonic::transport::server::Connected;
use tonic::transport::Server;
use tracing::{info, warn};

/// Newtype around a client-side `TlsStream<TcpStream>` so we can `impl
/// Connected` for it — tonic's `serve_with_incoming` requires that trait
/// on the IO type, and the orphan rules block implementing it on the
/// foreign `TlsStream` directly.
struct TunnelStream(tokio_rustls::client::TlsStream<TcpStream>);

impl Connected for TunnelStream {
    type ConnectInfo = ();
    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut StdContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut StdContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut StdContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut StdContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

use crate::agent_impl::NodeAgentService;
use crate::onboarding::NodePki;
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

/// Reconnect loop: dial the api, host gRPC services on the TLS stream until
/// it breaks, then back off and retry.
pub async fn run(
    tunnel_endpoint: String,
    pki: NodePki,
    agent: NodeAgentService,
    proxies: ProxyBundle,
) -> Result<()> {
    install_default_crypto_provider();
    let tls_config = build_client_tls(&pki)?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let backoff = Duration::from_secs(5);
    loop {
        match dial_and_serve(&tunnel_endpoint, &connector, agent.clone(), proxies.clone()).await {
            Ok(()) => info!("tunnel closed by api, reconnecting..."),
            Err(e) => warn!(error = %e, "tunnel connection failed; retrying"),
        }
        tokio::time::sleep(backoff).await;
    }
}

async fn dial_and_serve(
    tunnel_endpoint: &str,
    connector: &TlsConnector,
    agent: NodeAgentService,
    proxies: ProxyBundle,
) -> Result<()> {
    let target = tunnel_endpoint
        .strip_prefix("http://")
        .or_else(|| tunnel_endpoint.strip_prefix("https://"))
        .unwrap_or(tunnel_endpoint);

    let host_for_sni = host_only(target);

    info!(%target, sni = %host_for_sni, "dialing api for reverse tunnel (mTLS)");
    let sock = TcpStream::connect(target)
        .await
        .with_context(|| format!("connecting to {target}"))?;
    sock.set_nodelay(true).ok();

    let server_name = ServerName::try_from(host_for_sni.clone())
        .map_err(|e| anyhow!("invalid SNI hostname '{}': {e}", host_for_sni))?;
    let tls = connector
        .connect(server_name, sock)
        .await
        .context("TLS handshake to cplane")?;
    info!("tunnel established, serving NodeAgent + daemon proxies");

    use futures::StreamExt;
    let stream = TunnelStream(tls);
    let incoming = futures::stream::once(async move { Ok::<_, std::io::Error>(stream) })
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

fn build_client_tls(pki: &NodePki) -> Result<rustls::ClientConfig> {
    let mut ca_bytes = pki.ca_pem.as_bytes();
    let ca_certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut ca_bytes)
            .collect::<Result<Vec<_>, _>>()
            .context("parse ca.pem")?;
    let mut client_bytes = pki.cert_pem.as_bytes();
    let client_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut client_bytes)
            .collect::<Result<Vec<_>, _>>()
            .context("parse cert.pem")?;
    let mut key_bytes = pki.key_pem.as_bytes();
    let client_key = rustls_pemfile::private_key(&mut key_bytes)
        .context("parse key.pem")?
        .ok_or_else(|| anyhow!("no private key in key.pem"))?;

    let mut roots = rustls::RootCertStore::empty();
    for c in &ca_certs {
        roots.add(c.clone()).context("add CA to root store")?;
    }
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(client_chain, client_key)
        .context("client config")
}

fn install_default_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn host_only(target: &str) -> String {
    // Strip ":port" suffix for SNI. IPv6 literals in brackets are common
    // here ("[::1]:50056"); peel them.
    if let Some(end) = target.find(']') {
        let host = &target[..=end];
        return host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
    }
    match target.rfind(':') {
        Some(i) => target[..i].to_string(),
        None => target.to_string(),
    }
}
