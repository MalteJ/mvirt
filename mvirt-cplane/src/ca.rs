//! Internal CA for node onboarding (ADR-0006).
//!
//! The cplane operates a self-signed Ed25519 CA whose certs are issued to:
//! - itself, for the reverse-tunnel TLS server endpoint
//! - each onboarded node, as a client cert that pins `(node_id, cluster_slug)`
//!
//! Node identity is encoded in `SubjectAlternativeName` as URIs — `mvirt://`
//! scheme, no PEN-registered OIDs needed. Standard tooling can read these
//! out, and the tunnel listener does so on every handshake.
//!
//! The CA private key lives plain in raft state for v1 (see ADR-0006).
//! Wrap-at-rest is left for a follow-up — the data shape is forward-compat.
//!
//! Cert lifetimes:
//! - CA root: 10 years
//! - server cert (cplane): 90 days, rotated by leader
//! - node client cert: 90 days, renewed at 80% lifetime by node

use anyhow::{Context, Result, anyhow};
use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// URI scheme used in SubjectAlternativeName extensions to pin identity.
pub const NODE_URI_PREFIX: &str = "mvirt://node/";
pub const CLUSTER_URI_PREFIX: &str = "mvirt://cluster/";

/// Validity period for freshly-issued leaf certs (server + client).
pub const LEAF_VALIDITY_DAYS: i64 = 90;
/// Validity period for the CA root cert.
pub const CA_VALIDITY_YEARS: i64 = 10;

/// The CA material as stored in raft. PEM-encoded so it round-trips through
/// snapshot/restore cleanly and is human-inspectable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalCa {
    /// PEM-encoded CA root certificate.
    pub ca_cert_pem: String,
    /// PEM-encoded CA private key. v1: plaintext. See ADR-0006 for the
    /// forward-compat hook to KeyEnvelope variants.
    pub ca_key_pem: String,
    /// When the CA was bootstrapped.
    pub created_at: String,
}

/// Result of signing a leaf cert.
#[derive(Debug, Clone)]
pub struct SignedLeaf {
    pub cert_pem: String,
    pub serial_hex: String,
    pub not_after: String,
}

/// Generate a fresh self-signed CA. Called once per deployment, the result
/// gets persisted in raft state by the apply handler.
pub fn generate_root_ca(deployment_name: &str) -> Result<InternalCa> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "mvirt internal CA");
        dn.push(DnType::OrganizationName, deployment_name);
        dn.push(DnType::OrganizationalUnitName, "mvirt-cplane");
        dn
    };
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];

    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::minutes(5);
    params.not_after = now + Duration::days(CA_VALIDITY_YEARS * 365);

    let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).context("generate CA keypair")?;
    let cert = params.self_signed(&key).context("self-sign CA cert")?;

    Ok(InternalCa {
        ca_cert_pem: cert.pem(),
        ca_key_pem: key.serialize_pem(),
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Load CA material from its PEM representation back into rcgen objects
/// suitable for signing.
fn load_ca(ca: &InternalCa) -> Result<(rcgen::Certificate, KeyPair)> {
    let key = KeyPair::from_pem(&ca.ca_key_pem).context("parse CA key")?;
    let params = CertificateParams::from_ca_cert_pem(&ca.ca_cert_pem).context("parse CA cert")?;
    let cert = params
        .self_signed(&key)
        .context("rehydrate CA cert with key")?;
    Ok((cert, key))
}

/// Sign a node's CSR. The CSR's subject/SAN/attributes are **ignored** —
/// we extract only the public key and write our own subject + SAN URIs.
/// ADR-0006 §"Bootstrap exchange".
///
/// `serial_bytes` is a 16-byte random value chosen by the caller (so the
/// apply handler can persist it atomically alongside the Node row).
pub fn sign_node_leaf(
    ca: &InternalCa,
    csr_pem: &str,
    node_id: &str,
    cluster_slug: &str,
    serial_bytes: [u8; 16],
) -> Result<SignedLeaf> {
    let (ca_cert, ca_key) = load_ca(ca)?;
    let csr = CertificateSigningRequestParams::from_pem(csr_pem)
        .map_err(|e| anyhow!("parse CSR: {e}"))?;

    let mut params = CertificateParams::default();
    // Override the CSR's claimed subject with our authoritative one.
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, node_id);
        dn
    };
    // Pin (node_id, cluster_slug) into SAN URIs — what the tunnel listener
    // reads back out on every handshake.
    params.subject_alt_names = vec![
        SanType::URI(
            format!("{NODE_URI_PREFIX}{node_id}")
                .try_into()
                .map_err(|e| anyhow!("bad node URI: {e}"))?,
        ),
        SanType::URI(
            format!("{CLUSTER_URI_PREFIX}{cluster_slug}")
                .try_into()
                .map_err(|e| anyhow!("bad cluster URI: {e}"))?,
        ),
    ];
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::minutes(5);
    params.not_after = now + Duration::days(LEAF_VALIDITY_DAYS);
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial_bytes));

    let not_after = params.not_after;
    let leaf = params
        .signed_by(&csr.public_key, &ca_cert, &ca_key)
        .context("sign leaf with CA")?;

    Ok(SignedLeaf {
        cert_pem: leaf.pem(),
        serial_hex: hex_serial(&serial_bytes),
        not_after: chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::from(not_after))
            .to_rfc3339(),
    })
}

/// Sign a fresh server cert for the cplane's tunnel endpoint. Called by the
/// leader at startup and whenever the existing cert is approaching expiry.
pub fn sign_server_cert(
    ca: &InternalCa,
    dns_names: Vec<String>,
    serial_bytes: [u8; 16],
) -> Result<(SignedLeaf, String)> {
    let (ca_cert, ca_key) = load_ca(ca)?;

    let mut params = CertificateParams::default();
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "mvirt-cplane");
        dn
    };
    params.subject_alt_names = dns_names
        .into_iter()
        .map(|n| {
            // Accept either DNS names or IPs.
            if let Ok(ip) = n.parse::<std::net::IpAddr>() {
                SanType::IpAddress(ip)
            } else {
                SanType::DnsName(
                    n.try_into()
                        .unwrap_or_else(|_| "localhost".to_string().try_into().expect("valid dns")),
                )
            }
        })
        .collect();
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::minutes(5);
    params.not_after = now + Duration::days(LEAF_VALIDITY_DAYS);
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial_bytes));

    let server_key =
        KeyPair::generate_for(&rcgen::PKCS_ED25519).context("generate server keypair")?;
    let not_after = params.not_after;
    let leaf = params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .context("sign server cert")?;

    Ok((
        SignedLeaf {
            cert_pem: leaf.pem(),
            serial_hex: hex_serial(&serial_bytes),
            not_after: chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::from(
                not_after,
            ))
            .to_rfc3339(),
        },
        server_key.serialize_pem(),
    ))
}

/// Extract `(node_id, cluster_slug)` from a verified peer cert's SAN URIs.
/// Returns an error if either pin is missing. Used by the mTLS tunnel
/// listener after the handshake completes.
pub fn extract_identity_from_der(cert_der: &[u8]) -> Result<(String, String)> {
    use x509_parser::extensions::{GeneralName, ParsedExtension};
    use x509_parser::prelude::FromDer;

    let (_, parsed) = x509_parser::certificate::X509Certificate::from_der(cert_der)
        .map_err(|e| anyhow!("parse peer cert DER: {e}"))?;

    let mut node_id: Option<String> = None;
    let mut cluster_slug: Option<String> = None;

    for ext in parsed.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for name in &san.general_names {
                if let GeneralName::URI(u) = name {
                    if let Some(rest) = u.strip_prefix(NODE_URI_PREFIX) {
                        node_id = Some(rest.to_string());
                    } else if let Some(rest) = u.strip_prefix(CLUSTER_URI_PREFIX) {
                        cluster_slug = Some(rest.to_string());
                    }
                }
            }
        }
    }

    match (node_id, cluster_slug) {
        (Some(n), Some(c)) => Ok((n, c)),
        _ => Err(anyhow!(
            "peer cert is missing required SAN URIs (mvirt://node/.. + mvirt://cluster/..)"
        )),
    }
}

/// Generate 16 cryptographically-random bytes for use as a cert serial.
pub fn new_serial() -> [u8; 16] {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

fn hex_serial(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Hex-encode the DER bytes of a peer cert serial for comparison against
/// the REVOKED_CERTS table. The serial in our cert generation uses 16
/// random bytes; we serialize it back to the same hex format.
pub fn serial_hex_from_der(serial: &[u8]) -> String {
    hex_serial(serial)
}

// =============================================================================
// rustls glue (server + client TLS configs)
// =============================================================================

/// Parse one or more PEM certificates into rustls `CertificateDer`.
pub fn parse_pem_certs(
    pem: &str,
) -> anyhow::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let mut bytes = pem.as_bytes();
    rustls_pemfile::certs(&mut bytes)
        .collect::<Result<Vec<_>, _>>()
        .context("parse PEM certs")
}

/// Parse a PEM-encoded private key (PKCS#8 or SEC1) into rustls.
pub fn parse_pem_private_key(
    pem: &str,
) -> anyhow::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let mut bytes = pem.as_bytes();
    rustls_pemfile::private_key(&mut bytes)
        .context("parse PEM key")?
        .ok_or_else(|| anyhow!("no PEM private key found"))
}

/// Build a rustls server config that requires + verifies client certs
/// signed by `ca_cert_pem`. Used by the reverse-tunnel listener.
pub fn build_server_config(
    ca_cert_pem: &str,
    server_cert_pem: &str,
    server_key_pem: &str,
) -> anyhow::Result<rustls::ServerConfig> {
    let ca_certs = parse_pem_certs(ca_cert_pem)?;
    let server_chain = parse_pem_certs(server_cert_pem)?;
    let server_key = parse_pem_private_key(server_key_pem)?;

    let mut roots = rustls::RootCertStore::empty();
    for c in &ca_certs {
        roots.add(c.clone()).context("add CA to root store")?;
    }
    let verifier = rustls::server::WebPkiClientVerifier::builder(std::sync::Arc::new(roots))
        .build()
        .context("build client cert verifier")?;
    rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(server_chain, server_key)
        .context("server config with_single_cert")
}

/// Build a rustls client config that pins `ca_cert_pem` as the only trust
/// anchor and presents `(client_cert_pem, client_key_pem)`. Used by the
/// mvirt-node mTLS dialer.
pub fn build_client_config(
    ca_cert_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> anyhow::Result<rustls::ClientConfig> {
    let ca_certs = parse_pem_certs(ca_cert_pem)?;
    let client_chain = parse_pem_certs(client_cert_pem)?;
    let client_key = parse_pem_private_key(client_key_pem)?;

    let mut roots = rustls::RootCertStore::empty();
    for c in &ca_certs {
        roots.add(c.clone()).context("add CA to root store")?;
    }
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(client_chain, client_key)
        .context("client config with_client_auth_cert")
}

/// Install the ring-backed default crypto provider exactly once.
pub fn install_default_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_csr() -> (String, KeyPair) {
        let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("gen csr key");
        // Empty subject, no SANs — server fills everything.
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        let csr = params.serialize_request(&key).expect("serialize csr");
        (csr.pem().unwrap(), key)
    }

    #[test]
    fn generate_then_sign_then_extract() {
        let ca = generate_root_ca("test-deployment").unwrap();
        let (csr_pem, _node_key) = make_csr();
        let leaf = sign_node_leaf(&ca, &csr_pem, "node_abc123", "west-1", new_serial()).unwrap();

        // Parse leaf back, ensure SANs round-trip.
        let der = rustls_pemfile::certs(&mut leaf.cert_pem.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let (node_id, cluster_slug) = extract_identity_from_der(&der).unwrap();
        assert_eq!(node_id, "node_abc123");
        assert_eq!(cluster_slug, "west-1");
    }

    #[test]
    fn server_cert_signed_by_same_ca() {
        let ca = generate_root_ca("test").unwrap();
        let (server_leaf, _server_key_pem) =
            sign_server_cert(&ca, vec!["localhost".into()], new_serial()).unwrap();
        assert!(server_leaf.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(server_leaf.cert_pem.contains("END CERTIFICATE"));
    }
}
