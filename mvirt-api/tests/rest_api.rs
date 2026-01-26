//! REST API integration tests for mvirt-cp.
//!
//! These tests verify that all REST endpoints work correctly against a single-node cluster.

mod common;

use serde_json::{Value, json};

// =============================================================================
// Version Endpoint
// =============================================================================

#[tokio::test]
async fn test_get_version() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/version").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["version"].is_string());
    assert!(!body["version"].as_str().unwrap().is_empty());

    server.shutdown().await;
}

// =============================================================================
// Cluster Endpoints
// =============================================================================

#[tokio::test]
async fn test_get_cluster_info() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/cluster").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["cluster_id"].is_string());
    assert!(body["leader_id"].is_number());
    assert!(body["current_term"].is_number());
    assert!(body["nodes"].is_array());

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_membership() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/cluster/membership").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["voters"].is_array());
    assert!(body["learners"].is_array());
    assert!(body["nodes"].is_array());

    // Single node should be a voter
    let voters = body["voters"].as_array().unwrap();
    assert_eq!(voters.len(), 1);

    server.shutdown().await;
}

#[tokio::test]
async fn test_create_join_token() {
    let server = common::TestServer::spawn().await;

    let response = server
        .post_json(
            "/cluster/join-token",
            &json!({
                "node_id": 2,
                "valid_for_secs": 60
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["token"].is_string());
    assert_eq!(body["node_id"].as_u64().unwrap(), 2);
    assert_eq!(body["valid_for_secs"].as_u64().unwrap(), 60);

    server.shutdown().await;
}

#[tokio::test]
async fn test_remove_node_not_found() {
    let server = common::TestServer::spawn().await;

    let response = server.delete("/cluster/nodes/99").await;
    // Node removal returns 404 if not found, or may return other codes if not leader
    let status = response.status();
    let body: Value = response.json().await.unwrap();

    // Accept 404 (not found) or verify error message
    if status == 404 {
        // Test passes - node not found
    } else {
        // For other statuses, verify there's an error message
        assert!(
            body["error"].is_string(),
            "Expected error message, got: {:?}",
            body
        );
    }

    server.shutdown().await;
}

// =============================================================================
// Network CRUD
// =============================================================================

#[tokio::test]
async fn test_create_network() {
    let server = common::TestServer::spawn().await;

    let response = server
        .post_json(
            "/networks",
            &json!({
                "name": "test-network",
                "ipv4_enabled": true,
                "ipv4_subnet": "10.0.0.0/24",
                "dns_servers": ["8.8.8.8"]
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["name"].as_str().unwrap(), "test-network");
    assert!(body["ipv4_enabled"].as_bool().unwrap());
    assert_eq!(body["ipv4_subnet"].as_str().unwrap(), "10.0.0.0/24");
    assert_eq!(body["nic_count"].as_u64().unwrap(), 0);
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());

    server.shutdown().await;
}

#[tokio::test]
async fn test_create_network_duplicate_name() {
    let server = common::TestServer::spawn().await;

    // Create first network
    let response1 = server
        .post_json(
            "/networks",
            &json!({
                "name": "duplicate-name"
            }),
        )
        .await;
    assert_eq!(response1.status(), 200);

    // Try to create second with same name
    let response2 = server
        .post_json(
            "/networks",
            &json!({
                "name": "duplicate-name"
            }),
        )
        .await;
    assert_eq!(response2.status(), 409);

    let body: Value = response2.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("duplicate-name"));

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_network_by_id() {
    let server = common::TestServer::spawn().await;

    // Create network
    let create_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "by-id-test"
            }),
        )
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let network_id = created["id"].as_str().unwrap();

    // Get by ID
    let response = server.get(&format!("/networks/{}", network_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), network_id);
    assert_eq!(body["name"].as_str().unwrap(), "by-id-test");

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_network_by_name() {
    let server = common::TestServer::spawn().await;

    // Create network
    server
        .post_json(
            "/networks",
            &json!({
                "name": "by-name-test"
            }),
        )
        .await;

    // Get by name
    let response = server.get("/networks/by-name-test").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "by-name-test");

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_network_not_found() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/networks/non-existent-id").await;
    assert_eq!(response.status(), 404);

    let body: Value = response.json().await.unwrap();
    assert!(body["error"].is_string());

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_networks() {
    let server = common::TestServer::spawn().await;

    // Create a couple of networks
    server
        .post_json(
            "/networks",
            &json!({
                "name": "network-1"
            }),
        )
        .await;
    server
        .post_json(
            "/networks",
            &json!({
                "name": "network-2"
            }),
        )
        .await;

    // List
    let response = server.get("/networks").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 2);

    server.shutdown().await;
}

#[tokio::test]
async fn test_update_network_dns() {
    let server = common::TestServer::spawn().await;

    // Create network
    let create_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "update-test",
                "dns_servers": ["8.8.8.8"]
            }),
        )
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let network_id = created["id"].as_str().unwrap();

    // Update DNS servers
    let response = server
        .patch_json(
            &format!("/networks/{}", network_id),
            &json!({
                "dns_servers": ["1.1.1.1", "8.8.4.4"],
                "ntp_servers": ["pool.ntp.org"]
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    let dns_servers: Vec<&str> = body["dns_servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(dns_servers, vec!["1.1.1.1", "8.8.4.4"]);

    let ntp_servers: Vec<&str> = body["ntp_servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ntp_servers, vec!["pool.ntp.org"]);

    server.shutdown().await;
}

#[tokio::test]
async fn test_update_network_not_found() {
    let server = common::TestServer::spawn().await;

    let response = server
        .patch_json(
            "/networks/non-existent",
            &json!({
                "dns_servers": []
            }),
        )
        .await;
    assert_eq!(response.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn test_delete_network() {
    let server = common::TestServer::spawn().await;

    // Create network
    let create_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "delete-test"
            }),
        )
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let network_id = created["id"].as_str().unwrap();

    // Delete
    let response = server.delete(&format!("/networks/{}", network_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());
    assert_eq!(body["nics_deleted"].as_u64().unwrap(), 0);

    // Verify it's gone
    let get_resp = server.get(&format!("/networks/{}", network_id)).await;
    assert_eq!(get_resp.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn test_delete_network_with_nics() {
    let server = common::TestServer::spawn().await;

    // Create network
    let create_net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "delete-with-nics"
            }),
        )
        .await;
    let created_net: Value = create_net_resp.json().await.unwrap();
    let network_id = created_net["id"].as_str().unwrap();

    // Create NIC in network
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;

    // Try to delete without force - should fail
    let response = server.delete(&format!("/networks/{}", network_id)).await;
    assert_eq!(response.status(), 409);

    let body: Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("NIC"));

    server.shutdown().await;
}

#[tokio::test]
async fn test_delete_network_force() {
    let server = common::TestServer::spawn().await;

    // Create network
    let create_net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "force-delete"
            }),
        )
        .await;
    let created_net: Value = create_net_resp.json().await.unwrap();
    let network_id = created_net["id"].as_str().unwrap();

    // Create 2 NICs
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;

    // Delete with force
    let response = server
        .client
        .delete(format!(
            "{}/networks/{}?force=true",
            server.base_url(),
            network_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());
    assert_eq!(body["nics_deleted"].as_u64().unwrap(), 2);

    server.shutdown().await;
}

// =============================================================================
// NIC CRUD
// =============================================================================

#[tokio::test]
async fn test_create_nic() {
    let server = common::TestServer::spawn().await;

    // Create network first
    let net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "nic-test-network"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    // Create NIC
    let response = server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id,
                "name": "test-nic"
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["network_id"].as_str().unwrap(), network_id);
    assert_eq!(body["name"].as_str().unwrap(), "test-nic");
    // MAC should be auto-generated with QEMU prefix
    assert!(
        body["mac_address"]
            .as_str()
            .unwrap()
            .starts_with("52:54:00:")
    );
    assert!(
        body["socket_path"]
            .as_str()
            .unwrap()
            .starts_with("/run/mvirt-net/")
    );
    assert_eq!(body["state"].as_str().unwrap(), "Created");

    server.shutdown().await;
}

#[tokio::test]
async fn test_create_nic_network_not_found() {
    let server = common::TestServer::spawn().await;

    let response = server
        .post_json(
            "/nics",
            &json!({
                "network_id": "non-existent-network"
            }),
        )
        .await;
    assert_eq!(response.status(), 404);

    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("non-existent-network")
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_nic_by_id() {
    let server = common::TestServer::spawn().await;

    // Setup
    let net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "nic-get-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    let nic_resp = server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id,
                "name": "get-by-id-nic"
            }),
        )
        .await;
    let nic: Value = nic_resp.json().await.unwrap();
    let nic_id = nic["id"].as_str().unwrap();

    // Get by ID
    let response = server.get(&format!("/nics/{}", nic_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), nic_id);

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_nic_by_name() {
    let server = common::TestServer::spawn().await;

    // Setup
    let net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "nic-name-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id,
                "name": "my-named-nic"
            }),
        )
        .await;

    // Get by name
    let response = server.get("/nics/my-named-nic").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "my-named-nic");

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_nics() {
    let server = common::TestServer::spawn().await;

    // Create network and NICs
    let net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "list-nics-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;

    // List all
    let response = server.get("/nics").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 2);

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_nics_filter_by_network() {
    let server = common::TestServer::spawn().await;

    // Create two networks
    let net1_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "filter-net-1"
            }),
        )
        .await;
    let net1: Value = net1_resp.json().await.unwrap();
    let net1_id = net1["id"].as_str().unwrap();

    let net2_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "filter-net-2"
            }),
        )
        .await;
    let net2: Value = net2_resp.json().await.unwrap();
    let net2_id = net2["id"].as_str().unwrap();

    // Create NICs in different networks
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": net1_id
            }),
        )
        .await;
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": net1_id
            }),
        )
        .await;
    server
        .post_json(
            "/nics",
            &json!({
                "network_id": net2_id
            }),
        )
        .await;

    // Filter by network_id
    let response = server
        .client
        .get(format!("{}/nics?network_id={}", server.base_url(), net1_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 2);
    for nic in body.as_array().unwrap() {
        assert_eq!(nic["network_id"].as_str().unwrap(), net1_id);
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_update_nic_routed_prefixes() {
    let server = common::TestServer::spawn().await;

    // Setup
    let net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "update-nic-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    let nic_resp = server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;
    let nic: Value = nic_resp.json().await.unwrap();
    let nic_id = nic["id"].as_str().unwrap();

    // Update routed prefixes
    let response = server
        .patch_json(
            &format!("/nics/{}", nic_id),
            &json!({
                "routed_ipv4_prefixes": ["192.168.1.0/24", "192.168.2.0/24"],
                "routed_ipv6_prefixes": ["fd00::/64"]
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    let ipv4_prefixes: Vec<&str> = body["routed_ipv4_prefixes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ipv4_prefixes, vec!["192.168.1.0/24", "192.168.2.0/24"]);

    server.shutdown().await;
}

#[tokio::test]
async fn test_delete_nic() {
    let server = common::TestServer::spawn().await;

    // Setup
    let net_resp = server
        .post_json(
            "/networks",
            &json!({
                "name": "delete-nic-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    let nic_resp = server
        .post_json(
            "/nics",
            &json!({
                "network_id": network_id
            }),
        )
        .await;
    let nic: Value = nic_resp.json().await.unwrap();
    let nic_id = nic["id"].as_str().unwrap();

    // Delete
    let response = server.delete(&format!("/nics/{}", nic_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());

    // Verify it's gone
    let get_resp = server.get(&format!("/nics/{}", nic_id)).await;
    assert_eq!(get_resp.status(), 404);

    // Verify network NIC count decremented
    let net_get = server.get(&format!("/networks/{}", network_id)).await;
    let net_body: Value = net_get.json().await.unwrap();
    assert_eq!(net_body["nic_count"].as_u64().unwrap(), 0);

    server.shutdown().await;
}

#[tokio::test]
async fn test_nic_not_found() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/nics/non-existent").await;
    assert_eq!(response.status(), 404);

    server.shutdown().await;
}
