//! REST integration tests for Cluster CRUD + Node membership (ADR-0005).

mod common;

use serde_json::{Value, json};

/// Create a hypervisor Node row by hitting the internal register endpoint.
/// Returns the cplane-assigned node id.
async fn register_node(server: &common::TestServer, name: &str) -> String {
    let resp = server
        .post_json(
            "/nodes",
            &json!({
                "name": name,
                "address": "127.0.0.1:50051",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "node register failed");
    let body: Value = resp.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn create_cluster_under_org() {
    let server = common::TestServer::spawn().await;

    let resp = server
        .post_json(
            "/orgs/test/clusters",
            &json!({
                "slug": "west-1",
                "name": "West-1",
                "location": "frankfurt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["slug"], "west-1");
    assert_eq!(body["orgSlug"], "test");
    assert_eq!(body["location"], "frankfurt");
    assert!(body["nodeIds"].as_array().unwrap().is_empty());

    server.shutdown().await;
}

#[tokio::test]
async fn create_cluster_rejects_duplicate_slug_across_orgs() {
    let server = common::TestServer::spawn().await;

    server
        .post_json("/orgs", &json!({"slug": "other", "name": "Other"}))
        .await;

    let r1 = server
        .post_json(
            "/orgs/test/clusters",
            &json!({"slug": "shared", "name": "first"}),
        )
        .await;
    assert_eq!(r1.status(), 200);

    let r2 = server
        .post_json(
            "/orgs/other/clusters",
            &json!({"slug": "shared", "name": "second"}),
        )
        .await;
    assert_eq!(r2.status(), 409, "platform-wide unique slug must reject");

    server.shutdown().await;
}

#[tokio::test]
async fn create_cluster_rejects_invalid_slug() {
    let server = common::TestServer::spawn().await;

    let resp = server
        .post_json(
            "/orgs/test/clusters",
            &json!({"slug": "Bad_Slug", "name": "x"}),
        )
        .await;
    assert_eq!(resp.status(), 400);

    server.shutdown().await;
}

#[tokio::test]
async fn create_cluster_rejects_unknown_org() {
    let server = common::TestServer::spawn().await;

    let resp = server
        .post_json(
            "/orgs/no-such-org/clusters",
            &json!({"slug": "x", "name": "x"}),
        )
        .await;
    assert_eq!(resp.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn list_clusters_in_org_filters_by_org() {
    let server = common::TestServer::spawn().await;
    server
        .post_json("/orgs", &json!({"slug": "alpha", "name": "Alpha"}))
        .await;

    server
        .post_json(
            "/orgs/test/clusters",
            &json!({"slug": "test-c1", "name": "test-c1"}),
        )
        .await;
    server
        .post_json(
            "/orgs/alpha/clusters",
            &json!({"slug": "alpha-c1", "name": "alpha-c1"}),
        )
        .await;

    let resp = server.get("/orgs/test/clusters").await;
    let body: Value = resp.json().await.unwrap();
    let slugs: Vec<&str> = body["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["test-c1"]);

    server.shutdown().await;
}

#[tokio::test]
async fn update_cluster_patches_fields() {
    let server = common::TestServer::spawn().await;
    server
        .post_json(
            "/orgs/test/clusters",
            &json!({
                "slug": "patch-me",
                "name": "old name",
                "description": "old desc",
                "location": "old loc",
            }),
        )
        .await;

    // Patch name only.
    let r = server
        .patch_json("/clusters/patch-me", &json!({"name": "new name"}))
        .await;
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["name"], "new name");
    assert_eq!(body["description"], "old desc", "untouched fields stay");

    // Clear description with explicit null.
    let r = server
        .patch_json("/clusters/patch-me", &json!({"description": null}))
        .await;
    let body: Value = r.json().await.unwrap();
    assert!(
        body.get("description").is_none() || body["description"].is_null(),
        "description should be cleared"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn delete_cluster_removes_it() {
    let server = common::TestServer::spawn().await;
    server
        .post_json(
            "/orgs/test/clusters",
            &json!({"slug": "doomed", "name": "doomed"}),
        )
        .await;

    let r = server.delete("/clusters/doomed").await;
    assert_eq!(r.status(), 204);

    let r = server.get("/clusters/doomed").await;
    assert_eq!(r.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn add_node_to_cluster_then_remove() {
    let server = common::TestServer::spawn().await;
    server
        .post_json(
            "/orgs/test/clusters",
            &json!({"slug": "members", "name": "members"}),
        )
        .await;
    let node_id = register_node(&server, "rack3-node5").await;

    // Add.
    let r = server
        .post_json(&format!("/clusters/members/nodes/{}", node_id), &json!({}))
        .await;
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    let ids: Vec<&str> = body["nodeIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![node_id.as_str()]);

    // Adding the same node twice is idempotent.
    let r = server
        .post_json(&format!("/clusters/members/nodes/{}", node_id), &json!({}))
        .await;
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["nodeIds"].as_array().unwrap().len(), 1);

    // Remove.
    let r = server
        .delete(&format!("/clusters/members/nodes/{}", node_id))
        .await;
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert!(body["nodeIds"].as_array().unwrap().is_empty());

    // Removing again is idempotent.
    let r = server
        .delete(&format!("/clusters/members/nodes/{}", node_id))
        .await;
    assert_eq!(r.status(), 200);

    server.shutdown().await;
}

#[tokio::test]
async fn add_node_to_cluster_rejects_unknown_node() {
    let server = common::TestServer::spawn().await;
    server
        .post_json("/orgs/test/clusters", &json!({"slug": "c1", "name": "c1"}))
        .await;

    let r = server
        .post_json("/clusters/c1/nodes/no-such-node", &json!({}))
        .await;
    assert_eq!(r.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn add_node_to_cluster_rejects_unknown_cluster() {
    let server = common::TestServer::spawn().await;
    let node_id = register_node(&server, "n1").await;

    let r = server
        .post_json(
            &format!("/clusters/no-such-cluster/nodes/{}", node_id),
            &json!({}),
        )
        .await;
    assert_eq!(r.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn node_can_be_in_multiple_clusters() {
    // ADR-0005: explicit list, a Node may legitimately appear in more than
    // one Cluster (gpu-pool + nvme-pool).
    let server = common::TestServer::spawn().await;
    server
        .post_json("/orgs/test/clusters", &json!({"slug": "a", "name": "a"}))
        .await;
    server
        .post_json("/orgs/test/clusters", &json!({"slug": "b", "name": "b"}))
        .await;
    let node_id = register_node(&server, "dual-role").await;

    server
        .post_json(&format!("/clusters/a/nodes/{}", node_id), &json!({}))
        .await;
    server
        .post_json(&format!("/clusters/b/nodes/{}", node_id), &json!({}))
        .await;

    let a: Value = server.get("/clusters/a").await.json().await.unwrap();
    let b: Value = server.get("/clusters/b").await.json().await.unwrap();
    assert_eq!(a["nodeIds"][0], node_id);
    assert_eq!(b["nodeIds"][0], node_id);

    server.shutdown().await;
}
