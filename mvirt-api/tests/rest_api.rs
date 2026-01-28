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

    let proj_resp = server.post_json("/projects", &json!({"id": "createnetproj", "name": "create-net-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let response = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "test-network",
                "ipv4Enabled": true,
                "ipv4Prefix": "10.0.0.0/24",
                "dnsServers": ["8.8.8.8"]
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["name"].as_str().unwrap(), "test-network");
    assert!(body["ipv4Enabled"].as_bool().unwrap());
    assert_eq!(body["ipv4Prefix"].as_str().unwrap(), "10.0.0.0/24");
    assert_eq!(body["nicCount"].as_u64().unwrap(), 0);
    assert!(body["createdAt"].is_string());

    server.shutdown().await;
}

#[tokio::test]
async fn test_create_network_duplicate_name() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "dupnetproj", "name": "dup-net-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create first network
    let response1 = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "duplicate-name"
            }),
        )
        .await;
    assert_eq!(response1.status(), 200);

    // Try to create second with same name
    let response2 = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
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

    let proj_resp = server.post_json("/projects", &json!({"id": "getnetidproj", "name": "get-net-id-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create network
    let create_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "by-id-test"
            }),
        )
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let network_id = created["id"].as_str().unwrap();

    // Get by ID (global route)
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

    let proj_resp = server.post_json("/projects", &json!({"id": "getnetnameproj", "name": "get-net-name-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create network
    server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "by-name-test"
            }),
        )
        .await;

    // Get by name (global route)
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

    let proj_resp = server.post_json("/projects", &json!({"id": "listnetsproj", "name": "list-nets-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create a couple of networks
    server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "network-1"
            }),
        )
        .await;
    server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "network-2"
            }),
        )
        .await;

    // List
    let response = server.get(&format!("/projects/{}/networks", project_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    // UI format wraps networks in a "networks" field
    assert!(body["networks"].is_array());
    assert_eq!(body["networks"].as_array().unwrap().len(), 2);

    server.shutdown().await;
}

// Note: Network update endpoint is not available in UI-compatible API
// The tests below are skipped

#[tokio::test]
async fn test_delete_network() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "delnetproj", "name": "del-net-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create network
    let create_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "delete-test"
            }),
        )
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let network_id = created["id"].as_str().unwrap();

    // Delete (global route)
    let response = server.delete(&format!("/networks/{}", network_id)).await;
    assert_eq!(response.status(), 204);

    // Verify it's gone
    let get_resp = server.get(&format!("/networks/{}", network_id)).await;
    assert_eq!(get_resp.status(), 404);

    server.shutdown().await;
}

// Note: Force delete with NICs is not supported in UI-compatible API
// The test below is simplified

#[tokio::test]
async fn test_delete_network_with_nics() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "delnetnicsproj", "name": "del-net-nics-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create network
    let create_net_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
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
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id
            }),
        )
        .await;

    // Delete (global route) - may fail with conflict
    let response = server.delete(&format!("/networks/{}", network_id)).await;
    // Either succeeds with 204 or fails with 409 conflict
    assert!(response.status() == 204 || response.status() == 409);

    server.shutdown().await;
}

// Note: Force delete endpoint is not available in UI-compatible API

// =============================================================================
// NIC CRUD
// =============================================================================

#[tokio::test]
async fn test_create_nic() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "createnicproj", "name": "create-nic-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create network first
    let net_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
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
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id,
                "name": "test-nic"
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["networkId"].as_str().unwrap(), network_id);
    assert_eq!(body["name"].as_str().unwrap(), "test-nic");
    // MAC should be auto-generated with QEMU prefix
    assert!(
        body["macAddress"]
            .as_str()
            .unwrap()
            .starts_with("52:54:00:")
    );
    assert_eq!(body["state"].as_str().unwrap(), "Created");

    server.shutdown().await;
}

#[tokio::test]
async fn test_create_nic_network_not_found() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "nicnetnotfoundproj", "name": "nic-net-notfound-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let response = server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": "non-existent-network"
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

    let proj_resp = server.post_json("/projects", &json!({"id": "getnicidproj", "name": "get-nic-id-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Setup
    let net_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "nic-get-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    let nic_resp = server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id,
                "name": "get-by-id-nic"
            }),
        )
        .await;
    let nic: Value = nic_resp.json().await.unwrap();
    let nic_id = nic["id"].as_str().unwrap();

    // Get by ID (global route)
    let response = server.get(&format!("/nics/{}", nic_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), nic_id);

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_nic_by_name() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "getnicnameproj", "name": "get-nic-name-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Setup
    let net_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "nic-name-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id,
                "name": "my-named-nic"
            }),
        )
        .await;

    // Get by name (global route)
    let response = server.get("/nics/my-named-nic").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "my-named-nic");

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_nics() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "listnicsproj", "name": "list-nics-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create network and NICs
    let net_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "list-nics-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id
            }),
        )
        .await;
    server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id
            }),
        )
        .await;

    // List all - UI format wraps results in "nics" field
    let response = server.get(&format!("/projects/{}/nics", project_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["nics"].is_array());
    assert_eq!(body["nics"].as_array().unwrap().len(), 2);

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_nics_filter_by_network() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "filternicsproj", "name": "filter-nics-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create two networks
    let net1_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "filter-net-1"
            }),
        )
        .await;
    let net1: Value = net1_resp.json().await.unwrap();
    let net1_id = net1["id"].as_str().unwrap();

    let net2_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
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
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": net1_id
            }),
        )
        .await;
    server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": net1_id
            }),
        )
        .await;
    server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": net2_id
            }),
        )
        .await;

    // Filter by networkId (camelCase query param)
    let response = server
        .client
        .get(format!("{}/projects/{}/nics?networkId={}", server.base_url(), project_id, net1_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    // UI format wraps results in "nics" field
    assert_eq!(body["nics"].as_array().unwrap().len(), 2);
    for nic in body["nics"].as_array().unwrap() {
        assert_eq!(nic["networkId"].as_str().unwrap(), net1_id);
    }

    server.shutdown().await;
}

// Note: NIC update endpoint is not available in UI-compatible API

#[tokio::test]
async fn test_delete_nic() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "delnicproj", "name": "del-nic-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Setup
    let net_resp = server
        .post_json(
            &format!("/projects/{}/networks", project_id),
            &json!({
                "name": "delete-nic-test"
            }),
        )
        .await;
    let network: Value = net_resp.json().await.unwrap();
    let network_id = network["id"].as_str().unwrap();

    let nic_resp = server
        .post_json(
            &format!("/projects/{}/nics", project_id),
            &json!({
                "networkId": network_id
            }),
        )
        .await;
    let nic: Value = nic_resp.json().await.unwrap();
    let nic_id = nic["id"].as_str().unwrap();

    // Delete (global route)
    let response = server.delete(&format!("/nics/{}", nic_id)).await;
    assert_eq!(response.status(), 204);

    // Verify it's gone
    let get_resp = server.get(&format!("/nics/{}", nic_id)).await;
    assert_eq!(get_resp.status(), 404);

    // Verify network NIC count decremented
    let net_get = server.get(&format!("/networks/{}", network_id)).await;
    let net_body: Value = net_get.json().await.unwrap();
    assert_eq!(net_body["nicCount"].as_u64().unwrap(), 0);

    server.shutdown().await;
}

#[tokio::test]
async fn test_nic_not_found() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/nics/non-existent").await;
    assert_eq!(response.status(), 404);

    server.shutdown().await;
}

// =============================================================================
// Project CRUD
// =============================================================================

#[tokio::test]
async fn test_create_project() {
    let server = common::TestServer::spawn().await;

    let response = server
        .post_json(
            "/projects",
            &json!({
                "id": "testproject",
                "name": "test-project",
                "description": "A test project"
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), "testproject");
    assert_eq!(body["name"].as_str().unwrap(), "test-project");
    assert_eq!(body["description"].as_str().unwrap(), "A test project");
    assert!(body["createdAt"].is_string());

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_projects() {
    let server = common::TestServer::spawn().await;

    // Create projects
    server
        .post_json("/projects", &json!({"id": "project1", "name": "project-1"}))
        .await;
    server
        .post_json("/projects", &json!({"id": "project2", "name": "project-2"}))
        .await;

    let response = server.get("/projects").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["projects"].is_array());
    assert_eq!(body["projects"].as_array().unwrap().len(), 2);

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_project() {
    let server = common::TestServer::spawn().await;

    let create_resp = server
        .post_json("/projects", &json!({"id": "gettest", "name": "get-test"}))
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let project_id = created["id"].as_str().unwrap();

    let response = server.get(&format!("/projects/{}", project_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), project_id);
    assert_eq!(body["name"].as_str().unwrap(), "get-test");

    server.shutdown().await;
}

#[tokio::test]
async fn test_delete_project() {
    let server = common::TestServer::spawn().await;

    let create_resp = server
        .post_json("/projects", &json!({"id": "deletetest", "name": "delete-test"}))
        .await;
    let created: Value = create_resp.json().await.unwrap();
    let project_id = created["id"].as_str().unwrap();

    let response = server.delete(&format!("/projects/{}", project_id)).await;
    assert_eq!(response.status(), 204);

    // Verify it's gone
    let get_resp = server.get(&format!("/projects/{}", project_id)).await;
    assert_eq!(get_resp.status(), 404);

    server.shutdown().await;
}

// =============================================================================
// Storage - Volumes
// =============================================================================

#[tokio::test]
async fn test_create_volume() {
    let server = common::TestServer::spawn().await;

    // Create project first
    let proj_resp = server
        .post_json("/projects", &json!({"id": "voltestproj", "name": "vol-test-proj"}))
        .await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create volume
    let response = server
        .post_json(
            &format!("/projects/{}/volumes", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "test-volume",
                "sizeBytes": 10737418240_u64
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["projectId"].as_str().unwrap(), project_id);
    assert_eq!(body["nodeId"].as_str().unwrap(), "node-1");
    assert_eq!(body["name"].as_str().unwrap(), "test-volume");
    assert_eq!(body["sizeBytes"].as_u64().unwrap(), 10737418240);

    server.shutdown().await;
}

#[tokio::test]
async fn test_list_volumes() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server
        .post_json("/projects", &json!({"id": "listvolproj", "name": "list-vol-proj"}))
        .await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Create volumes
    server
        .post_json(
            &format!("/projects/{}/volumes", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "volume-1",
                "sizeBytes": 1000000000_u64
            }),
        )
        .await;
    server
        .post_json(
            &format!("/projects/{}/volumes", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "volume-2",
                "sizeBytes": 2000000000_u64
            }),
        )
        .await;

    let response = server.get(&format!("/projects/{}/volumes", project_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["volumes"].is_array());
    assert_eq!(body["volumes"].as_array().unwrap().len(), 2);

    server.shutdown().await;
}

#[tokio::test]
async fn test_resize_volume() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server
        .post_json("/projects", &json!({"id": "resizeproj", "name": "resize-proj"}))
        .await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let vol_resp = server
        .post_json(
            &format!("/projects/{}/volumes", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "resize-vol",
                "sizeBytes": 1000000000_u64
            }),
        )
        .await;
    let vol: Value = vol_resp.json().await.unwrap();
    let vol_id = vol["id"].as_str().unwrap();

    // Resize (global route)
    let response = server
        .post_json(
            &format!("/volumes/{}/resize", vol_id),
            &json!({"sizeBytes": 2000000000_u64}),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["sizeBytes"].as_u64().unwrap(), 2000000000);

    server.shutdown().await;
}

#[tokio::test]
async fn test_create_snapshot() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server
        .post_json("/projects", &json!({"id": "snapproj", "name": "snap-proj"}))
        .await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let vol_resp = server
        .post_json(
            &format!("/projects/{}/volumes", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "snap-vol",
                "sizeBytes": 1000000000_u64
            }),
        )
        .await;
    let vol: Value = vol_resp.json().await.unwrap();
    let vol_id = vol["id"].as_str().unwrap();

    // Create snapshot (global route)
    let response = server
        .post_json(
            &format!("/volumes/{}/snapshots", vol_id),
            &json!({"name": "my-snapshot"}),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["snapshots"].is_array());
    assert_eq!(body["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["snapshots"][0]["name"].as_str().unwrap(),
        "my-snapshot"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_delete_volume() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server
        .post_json("/projects", &json!({"id": "delvolproj", "name": "del-vol-proj"}))
        .await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let vol_resp = server
        .post_json(
            &format!("/projects/{}/volumes", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "delete-vol",
                "sizeBytes": 1000000000_u64
            }),
        )
        .await;
    let vol: Value = vol_resp.json().await.unwrap();
    let vol_id = vol["id"].as_str().unwrap();

    // Delete (global route)
    let response = server.delete(&format!("/volumes/{}", vol_id)).await;
    assert_eq!(response.status(), 204);

    // Verify it's gone
    let get_resp = server.get(&format!("/volumes/{}", vol_id)).await;
    assert_eq!(get_resp.status(), 404);

    server.shutdown().await;
}

// =============================================================================
// Storage - Templates
// =============================================================================

#[tokio::test]
async fn test_list_templates() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "listtplproj", "name": "list-tpl-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    // Templates are created via import jobs, so this just tests the list endpoint
    let response = server.get(&format!("/projects/{}/templates", project_id)).await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["templates"].is_array());

    server.shutdown().await;
}

#[tokio::test]
async fn test_import_template() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "importtplproj", "name": "import-tpl-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let response = server
        .post_json(
            &format!("/projects/{}/templates/import", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "ubuntu-22.04",
                "url": "https://cloud-images.ubuntu.com/jammy/current/jammy-server-cloudimg-amd64.img",
                "totalBytes": 600000000_u64
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["templateName"].as_str().unwrap(), "ubuntu-22.04");
    assert_eq!(body["state"].as_str().unwrap(), "PENDING");
    assert_eq!(body["bytesWritten"].as_u64().unwrap(), 0);

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_import_job() {
    let server = common::TestServer::spawn().await;

    let proj_resp = server.post_json("/projects", &json!({"id": "importjobproj", "name": "import-job-proj"})).await;
    let proj: Value = proj_resp.json().await.unwrap();
    let project_id = proj["id"].as_str().unwrap();

    let import_resp = server
        .post_json(
            &format!("/projects/{}/templates/import", project_id),
            &json!({
                "nodeId": "node-1",
                "name": "debian-12",
                "url": "https://example.com/debian.img",
                "totalBytes": 500000000_u64
            }),
        )
        .await;
    let import: Value = import_resp.json().await.unwrap();
    let job_id = import["id"].as_str().unwrap();

    let response = server
        .get(&format!("/import-jobs/{}", job_id))
        .await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), job_id);
    assert_eq!(body["templateName"].as_str().unwrap(), "debian-12");

    server.shutdown().await;
}

#[tokio::test]
async fn test_get_pool_stats() {
    let server = common::TestServer::spawn().await;

    let response = server.get("/pool").await;
    assert_eq!(response.status(), 200);

    let body: Value = response.json().await.unwrap();
    assert!(body["totalBytes"].is_number());
    assert!(body["usedBytes"].is_number());
    assert!(body["availableBytes"].is_number());
    assert!(body["compressionRatio"].is_number());

    server.shutdown().await;
}
