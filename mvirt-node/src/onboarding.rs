//! Node-side onboarding (ADR-0006).
//!
//! On first boot the node has only an onboarding token in its config. It
//! generates an Ed25519 keypair locally, builds a minimal CSR (empty subject,
//! no SANs — the cplane fills its own), and POSTs the CSR + token to the
//! cplane REST endpoint. The response carries a signed client cert + the
//! internal CA root, both of which are persisted to disk. The token is
//! consumed on the cplane side and useless from then on.
//!
//! On subsequent boots the node reads its existing key + cert + ca from
//! disk and skips the bootstrap exchange entirely.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// On-disk state directory layout:
///
/// ```text
/// <state_dir>/
///   key.pem       0600  client-cert private key (ed25519)
///   cert.pem      0644  signed client cert from the cplane
///   ca.pem        0644  internal CA root (server-cert trust anchor)
///   state.toml    0644  metadata sidecar (node_id, cluster_slug, ...)
/// ```
pub struct NodePki {
    pub key_pem: String,
    pub cert_pem: String,
    pub ca_pem: String,
    pub state: NodeState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub node_id: String,
    pub cluster_slug: String,
    pub tunnel_endpoint: String,
    pub cert_not_after: String,
    /// mvirt-log endpoints learned at bootstrap (cplane-side mvirt-log
    /// instances). Daemons read these from the sidecar env file the
    /// onboarding flow writes alongside state.toml. Older state.toml
    /// files without this field deserialize to an empty Vec.
    #[serde(default)]
    pub log_endpoints: Vec<String>,
}

/// Result of the bootstrap REST exchange. Field names match the cplane's
/// `UiBootstrapResponse` (camelCase on the wire).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapResponse {
    node_id: String,
    cluster_slug: String,
    client_cert_pem: String,
    ca_cert_pem: String,
    cert_not_after: String,
    #[serde(default)]
    log_endpoints: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapRequest<'a> {
    csr_pem: &'a str,
    hostname: &'a str,
    agent_version: &'a str,
    kernel_version: &'a str,
    arch: &'a str,
}

/// Load PKI material from disk if all four files are present; otherwise
/// return `Ok(None)` and let the caller decide whether to bootstrap.
pub fn load_from_disk(state_dir: &Path) -> Result<Option<NodePki>> {
    let key = state_dir.join("key.pem");
    let cert = state_dir.join("cert.pem");
    let ca = state_dir.join("ca.pem");
    let state_path = state_dir.join("state.toml");
    if !key.exists() || !cert.exists() || !ca.exists() || !state_path.exists() {
        return Ok(None);
    }
    let key_pem = std::fs::read_to_string(&key).context("read key.pem")?;
    let cert_pem = std::fs::read_to_string(&cert).context("read cert.pem")?;
    let ca_pem = std::fs::read_to_string(&ca).context("read ca.pem")?;
    let state_str = std::fs::read_to_string(&state_path).context("read state.toml")?;
    let state: NodeState = toml::from_str(&state_str).context("parse state.toml")?;
    Ok(Some(NodePki {
        key_pem,
        cert_pem,
        ca_pem,
        state,
    }))
}

/// Bootstrap by hitting the cplane REST endpoint. The bootstrap-token is
/// passed in `Authorization: Bearer ...`. Returns the freshly-issued PKI.
///
/// `cplane_endpoint` is the operator-facing REST URL (e.g.
/// `https://api.mvirt.io`). For `--insecure-skip-tls-verify` setups we use
/// a permissive HTTP client.
pub async fn bootstrap(
    cplane_endpoint: &str,
    onboarding_token: &str,
    tunnel_endpoint_hint: Option<String>,
    insecure_skip_tls_verify: bool,
    agent_version: &str,
) -> Result<NodePki> {
    // 1. Generate keypair + CSR locally.
    let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).context("generate node keypair")?;
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    let csr = params.serialize_request(&key).context("serialize CSR")?;
    let csr_pem = csr.pem().context("CSR -> PEM")?;
    let key_pem = key.serialize_pem();

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".into());
    let kernel_version = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let arch = std::env::consts::ARCH;

    info!(%hostname, "POSTing bootstrap CSR to {}", cplane_endpoint);

    let client = if insecure_skip_tls_verify {
        warn!("--insecure-skip-tls-verify enabled; not validating cplane server cert");
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?
    } else {
        reqwest::Client::new()
    };

    let url = format!(
        "{}/v1/bootstrap/onboarding",
        cplane_endpoint.trim_end_matches('/')
    );
    let resp = client
        .post(&url)
        .bearer_auth(onboarding_token)
        .json(&BootstrapRequest {
            csr_pem: &csr_pem,
            hostname: &hostname,
            agent_version,
            kernel_version: &kernel_version,
            arch,
        })
        .send()
        .await
        .context("POST /v1/bootstrap/onboarding")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "bootstrap rejected with status {}: {}",
            status,
            body
        ));
    }
    let body: BootstrapResponse = resp.json().await.context("decode bootstrap response")?;

    // tunnel_endpoint comes from the operator-supplied flag for v1 (no
    // separate field returned yet — the cplane and tunnel host are the same
    // deployment so the operator knows). Fall back to the REST endpoint.
    let tunnel_endpoint = tunnel_endpoint_hint.unwrap_or_else(|| {
        cplane_endpoint
            .trim_end_matches('/')
            .replace("https://", "")
            .replace("http://", "")
            + ":50056"
    });

    Ok(NodePki {
        key_pem,
        cert_pem: body.client_cert_pem,
        ca_pem: body.ca_cert_pem,
        state: NodeState {
            node_id: body.node_id,
            cluster_slug: body.cluster_slug,
            tunnel_endpoint,
            cert_not_after: body.cert_not_after,
            log_endpoints: body.log_endpoints,
        },
    })
}

/// Persist the PKI material to disk with restrictive permissions on the
/// private key.
pub fn persist(state_dir: &Path, pki: &NodePki) -> Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    write_secret(&state_dir.join("key.pem"), &pki.key_pem)?;
    std::fs::write(state_dir.join("cert.pem"), &pki.cert_pem).context("write cert.pem")?;
    std::fs::write(state_dir.join("ca.pem"), &pki.ca_pem).context("write ca.pem")?;
    let state_str = toml::to_string_pretty(&pki.state).context("serialize state.toml")?;
    std::fs::write(state_dir.join("state.toml"), state_str).context("write state.toml")?;
    write_env_sidecar(state_dir, &pki.state)?;
    Ok(())
}

/// Write `state_dir/env` for systemd `EnvironmentFile=`. Daemons (vmm,
/// zfs, ebpf, shipper) pick up the values from here so they don't have
/// to parse `state.toml` themselves. Re-written on every
/// onboarding/refresh so it always tracks the latest state.
fn write_env_sidecar(state_dir: &Path, state: &NodeState) -> Result<()> {
    let body = format!(
        "MVIRT_LOG_ENDPOINTS={}\nMVIRT_NODE_ID={}\n",
        state.log_endpoints.join(","),
        state.node_id,
    );
    std::fs::write(state_dir.join("env"), body).context("write env sidecar")?;
    Ok(())
}

#[cfg(unix)]
fn write_secret(path: &PathBuf, contents: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {} for write", path.display()))?;
    use std::io::Write;
    f.write_all(contents.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret(path: &PathBuf, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}
