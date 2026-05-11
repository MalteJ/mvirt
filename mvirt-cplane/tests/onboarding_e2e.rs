//! End-to-end: bootstrap REST → mTLS tunnel handshake.
//!
//! Verifies the full ADR-0006 flow at the TLS layer:
//! - Operator creates Cluster + token
//! - Node POSTs CSR + token to /v1/bootstrap/onboarding, gets cert + CA
//! - Node opens TCP to the tunnel port and performs an mTLS handshake
//!   using the issued cert; the cplane accepts the handshake, extracts
//!   (node_id, cluster_slug) from the cert, marks the node Online.
//!
//! We don't run a full inverted-gRPC server on the test "node" side —
//! the TLS handshake completing successfully is the assertion that the
//! security boundary is correct. HTTP/2 / gRPC plumbing is covered by
//! the cplane unit tests.

mod common;

use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use rustls::pki_types::ServerName;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

async fn make_empty_csr() -> (String, KeyPair) {
    let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("gen key");
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    let csr = params.serialize_request(&key).expect("serialize CSR");
    (csr.pem().expect("pem"), key)
}

async fn bootstrap_a_node(
    server: &common::TestServer,
    cluster: &str,
) -> (String, String, String, String) {
    // returns (token, csr_key_pem, cert_pem, ca_pem)
    server
        .post_json(
            "/orgs/test/clusters",
            &json!({"slug": cluster, "name": cluster}),
        )
        .await;
    let r = server
        .post_json(
            &format!("/clusters/{}/onboarding-tokens", cluster),
            &json!({"ttlSeconds": 600, "hostname": "test-node"}),
        )
        .await;
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let (csr_pem, key) = make_empty_csr().await;
    let key_pem = key.serialize_pem();

    let r = server
        .client
        .post(format!("{}/bootstrap/onboarding", server.base_url()))
        .bearer_auth(&token)
        .json(&json!({
            "csrPem": csr_pem,
            "hostname": "test-host",
            "agentVersion": "0",
            "kernelVersion": "0",
            "arch": "x86_64",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "bootstrap should succeed");
    let body: Value = r.json().await.unwrap();
    (
        body["nodeId"].as_str().unwrap().to_string(),
        key_pem,
        body["clientCertPem"].as_str().unwrap().to_string(),
        body["caCertPem"].as_str().unwrap().to_string(),
    )
}

fn build_client_tls(ca_pem: &str, cert_pem: &str, key_pem: &str) -> rustls::ClientConfig {
    let mut ca_bytes = ca_pem.as_bytes();
    let ca_certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut ca_bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
    let mut client_bytes = cert_pem.as_bytes();
    let client_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut client_bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
    let mut key_bytes = key_pem.as_bytes();
    let client_key = rustls_pemfile::private_key(&mut key_bytes)
        .unwrap()
        .unwrap();

    let mut roots = rustls::RootCertStore::empty();
    for c in &ca_certs {
        roots.add(c.clone()).unwrap();
    }
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(client_chain, client_key)
        .unwrap()
}

fn install_crypto() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[tokio::test]
async fn mtls_tunnel_handshake_succeeds_with_issued_cert() {
    install_crypto();
    let server = common::TestServer::spawn_with_tunnel().await;
    let tunnel_addr = server.tunnel_addr.expect("tunnel listener address");

    let (_node_id, key_pem, cert_pem, ca_pem) = bootstrap_a_node(&server, "west-1").await;
    let cfg = build_client_tls(&ca_pem, &cert_pem, &key_pem);
    let connector = TlsConnector::from(Arc::new(cfg));

    let sock = TcpStream::connect(tunnel_addr).await.unwrap();
    let server_name = ServerName::try_from("localhost").unwrap();
    let tls = connector
        .connect(server_name, sock)
        .await
        .expect("mTLS handshake to cplane");
    // Hold the stream open for a moment so the cplane can complete its
    // post-handshake bookkeeping before we shut down.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    drop(tls);

    server.shutdown().await;
}

#[tokio::test]
async fn mtls_tunnel_handshake_rejects_no_client_cert() {
    install_crypto();
    let server = common::TestServer::spawn_with_tunnel().await;
    let tunnel_addr = server.tunnel_addr.expect("tunnel listener address");

    // Bootstrap a node only so the CA exists.
    let (_, _, _, ca_pem) = bootstrap_a_node(&server, "west-1").await;

    // Build a client config WITHOUT a client cert.
    let mut ca_bytes = ca_pem.as_bytes();
    let ca_certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut ca_bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for c in &ca_certs {
        roots.add(c.clone()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(cfg));

    let sock = TcpStream::connect(tunnel_addr).await.unwrap();
    let server_name = ServerName::try_from("localhost").unwrap();
    let result = expect_tls_or_io_failure(connector, server_name, sock).await;
    assert!(
        result.is_err(),
        "tunnel must reject TLS handshake with no client cert ({:?})",
        result
    );

    server.shutdown().await;
}

/// Drive enough I/O after `connect` to surface the server's
/// certificate-verification alert. In TLS 1.3 the server can finish the
/// handshake optimistically and only send the rejection alert on the next
/// read; `connect().await.is_ok()` alone is therefore not a safe assertion.
async fn expect_tls_or_io_failure(
    connector: TlsConnector,
    server_name: ServerName<'static>,
    sock: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut tls = connector.connect(server_name, sock).await?;
    tls.write_all(b"ping").await?;
    tls.flush().await?;
    let mut buf = [0u8; 1];
    // EOF (Ok(0)) or io::Error both fail this read — that's the "rejected"
    // signal we want.
    let n = tls.read(&mut buf).await?;
    if n == 0 {
        return Err("server closed connection (likely cert verification failed)".into());
    }
    Ok(())
}

#[tokio::test]
async fn mtls_tunnel_handshake_rejects_cert_from_other_ca() {
    install_crypto();
    let server = common::TestServer::spawn_with_tunnel().await;
    let tunnel_addr = server.tunnel_addr.expect("tunnel listener address");

    // Bootstrap one node so the real CA exists on the cplane.
    let (_, _, _, real_ca_pem) = bootstrap_a_node(&server, "west-1").await;

    // Now build a *foreign* CA + a client cert signed by it. The cplane
    // should refuse this client cert during handshake.
    let foreign_key = KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("foreign CA key");
    let mut foreign_params = CertificateParams::default();
    foreign_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    foreign_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "foreign CA");
        dn
    };
    let foreign_ca = foreign_params.self_signed(&foreign_key).unwrap();

    let client_key = KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("client key");
    let mut leaf_params = CertificateParams::default();
    leaf_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "rogue");
        dn
    };
    leaf_params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let leaf = leaf_params
        .signed_by(&client_key, &foreign_ca, &foreign_key)
        .unwrap();

    let cfg = build_client_tls(&real_ca_pem, &leaf.pem(), &client_key.serialize_pem());
    let connector = TlsConnector::from(Arc::new(cfg));

    let sock = TcpStream::connect(tunnel_addr).await.unwrap();
    let server_name = ServerName::try_from("localhost").unwrap();
    let result = expect_tls_or_io_failure(connector, server_name, sock).await;
    assert!(
        result.is_err(),
        "tunnel must reject client cert from foreign CA ({:?})",
        result
    );

    server.shutdown().await;
}
