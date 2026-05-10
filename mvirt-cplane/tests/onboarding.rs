//! End-to-end REST tests for the onboarding flow (ADR-0006).
//!
//! Covers: token create → bootstrap exchange → cert returned → token
//! consumed; plus the negative paths (bad token, replay, expired, unknown
//! cluster, malformed CSR).

mod common;

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use serde_json::{Value, json};

fn make_empty_csr() -> String {
    let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("gen key");
    let mut params = CertificateParams::default();
    // Empty subject + no SANs — the cplane fills its own.
    params.distinguished_name = DistinguishedName::new();
    let csr = params.serialize_request(&key).expect("serialize CSR");
    csr.pem().expect("pem")
}

async fn create_cluster(server: &common::TestServer, slug: &str) {
    let resp = server
        .post_json("/orgs/test/clusters", &json!({"slug": slug, "name": slug}))
        .await;
    assert_eq!(resp.status(), 200);
}

async fn create_token(server: &common::TestServer, cluster_slug: &str) -> (String, String) {
    // returns (bare_token, token_id)
    let resp = server
        .post_json(
            &format!("/clusters/{}/onboarding-tokens", cluster_slug),
            &json!({"ttlSeconds": 600, "description": "rack-3"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "token create failed");
    let body: Value = resp.json().await.unwrap();
    (
        body["token"].as_str().unwrap().to_string(),
        body["id"].as_str().unwrap().to_string(),
    )
}

async fn bootstrap_with_token(
    server: &common::TestServer,
    token: &str,
    csr_pem: &str,
) -> reqwest::Response {
    server
        .client
        .post(format!("{}/bootstrap/onboarding", server.base_url()))
        .bearer_auth(token)
        .json(&json!({
            "csrPem": csr_pem,
            "hostname": "rack3-node5",
            "agentVersion": "0.4.0",
            "kernelVersion": "6.10",
            "arch": "x86_64",
        }))
        .send()
        .await
        .expect("send")
}

#[tokio::test]
async fn full_bootstrap_flow_returns_signed_cert_and_consumes_token() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "west-1").await;
    let (token, token_id) = create_token(&server, "west-1").await;

    let csr_pem = make_empty_csr();
    let resp = bootstrap_with_token(&server, &token, &csr_pem).await;
    assert_eq!(resp.status(), 200, "bootstrap should succeed");
    let body: Value = resp.json().await.unwrap();
    assert!(body["nodeId"].as_str().unwrap().starts_with("node_"));
    assert_eq!(body["clusterSlug"], "west-1");
    assert!(
        body["clientCertPem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE")
    );
    assert!(
        body["caCertPem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE")
    );

    // Token list now shows used_at + used_by_node_id.
    let r = server.get("/clusters/west-1/onboarding-tokens").await;
    let body: Value = r.json().await.unwrap();
    let tokens = body["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 1);
    let t = &tokens[0];
    assert_eq!(t["id"], token_id);
    assert!(!t["usedAt"].is_null(), "token should be marked used");
    assert!(t["usedByNodeId"].as_str().unwrap().starts_with("node_"));

    // Cluster now has the node in node_ids.
    let r = server.get("/clusters/west-1").await;
    let body: Value = r.json().await.unwrap();
    let ids = body["nodeIds"].as_array().unwrap();
    assert_eq!(ids.len(), 1);

    // /nodes shows the new row with cluster_slug + cert metadata.
    let r = server.get("/nodes").await;
    let nodes: Value = r.json().await.unwrap();
    let arr = nodes.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let n = &arr[0];
    assert_eq!(n["clusterSlug"], "west-1");
    assert!(n["certSerialHex"].is_string());
    assert!(n["certExpiresAt"].is_string());

    server.shutdown().await;
}

#[tokio::test]
async fn bootstrap_token_is_single_use() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;
    let (token, _) = create_token(&server, "c1").await;

    let r1 = bootstrap_with_token(&server, &token, &make_empty_csr()).await;
    assert_eq!(r1.status(), 200);

    let r2 = bootstrap_with_token(&server, &token, &make_empty_csr()).await;
    assert_eq!(r2.status(), 401, "second redeem must fail");

    server.shutdown().await;
}

#[tokio::test]
async fn bootstrap_rejects_bad_token() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;
    create_token(&server, "c1").await; // produce a legit one so the CA exists

    let r = bootstrap_with_token(&server, "not-a-real-token", &make_empty_csr()).await;
    assert_eq!(r.status(), 401);

    server.shutdown().await;
}

#[tokio::test]
async fn bootstrap_rejects_missing_bearer() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;

    let resp = server
        .client
        .post(format!("{}/bootstrap/onboarding", server.base_url()))
        .json(&json!({
            "csrPem": make_empty_csr(),
            "hostname": "x",
            "agentVersion": "0",
            "kernelVersion": "0",
            "arch": "x86_64",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    server.shutdown().await;
}

#[tokio::test]
async fn bootstrap_rejects_malformed_csr() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;
    let (token, _) = create_token(&server, "c1").await;

    let r = bootstrap_with_token(&server, &token, "this is not a CSR").await;
    assert_eq!(r.status(), 400);

    server.shutdown().await;
}

#[tokio::test]
async fn bootstrap_410_when_cluster_deleted() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "tmp").await;
    let (token, _) = create_token(&server, "tmp").await;
    let del = server.delete("/clusters/tmp").await;
    assert_eq!(del.status(), 204);

    let r = bootstrap_with_token(&server, &token, &make_empty_csr()).await;
    assert_eq!(r.status(), 410);

    server.shutdown().await;
}

#[tokio::test]
async fn create_token_returns_404_for_unknown_cluster() {
    let server = common::TestServer::spawn().await;
    let r = server
        .post_json(
            "/clusters/no-such-cluster/onboarding-tokens",
            &json!({"ttlSeconds": 600}),
        )
        .await;
    assert_eq!(r.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn delete_token_idempotent_per_cluster() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;
    let (_, id) = create_token(&server, "c1").await;

    let r = server
        .delete(&format!("/clusters/c1/onboarding-tokens/{}", id))
        .await;
    assert_eq!(r.status(), 204);

    let r = server
        .delete(&format!("/clusters/c1/onboarding-tokens/{}", id))
        .await;
    assert_eq!(r.status(), 404, "second delete must 404");

    server.shutdown().await;
}

#[tokio::test]
async fn revoke_node_compromise_keeps_row() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;
    let (token, _) = create_token(&server, "c1").await;
    let resp = bootstrap_with_token(&server, &token, &make_empty_csr()).await;
    let body: Value = resp.json().await.unwrap();
    let node_id = body["nodeId"].as_str().unwrap().to_string();

    let r = server
        .post_json(
            &format!("/nodes/{}/revoke", node_id),
            &json!({"reason": "compromise"}),
        )
        .await;
    assert_eq!(r.status(), 204);

    // Node row still present, status = Revoked.
    let r = server.get(&format!("/nodes/{}", node_id)).await;
    assert_eq!(r.status(), 200);
    let n: Value = r.json().await.unwrap();
    assert_eq!(n["status"].as_str().unwrap().to_lowercase(), "revoked");

    server.shutdown().await;
}

#[tokio::test]
async fn revoke_node_decommission_drops_row_and_membership() {
    let server = common::TestServer::spawn().await;
    create_cluster(&server, "c1").await;
    let (token, _) = create_token(&server, "c1").await;
    let resp = bootstrap_with_token(&server, &token, &make_empty_csr()).await;
    let body: Value = resp.json().await.unwrap();
    let node_id = body["nodeId"].as_str().unwrap().to_string();

    let r = server
        .post_json(
            &format!("/nodes/{}/revoke", node_id),
            &json!({"reason": "decommission"}),
        )
        .await;
    assert_eq!(r.status(), 204);

    let r = server.get(&format!("/nodes/{}", node_id)).await;
    assert_eq!(r.status(), 404, "node row must be gone");

    let r = server.get("/clusters/c1").await;
    let c: Value = r.json().await.unwrap();
    assert!(
        c["nodeIds"].as_array().unwrap().is_empty(),
        "decommission must drop from cluster"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn revoke_node_400_for_unknown_reason() {
    let server = common::TestServer::spawn().await;
    let r = server
        .post_json("/nodes/whatever/revoke", &json!({"reason": "bad-reason"}))
        .await;
    assert_eq!(r.status(), 400);

    server.shutdown().await;
}
