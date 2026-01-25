//! Integration tests for mvirt-one container lifecycle.
//!
//! These tests require:
//! - Root privileges (for container operations)
//! - Network access (for image pull from Docker registry)
//! - Linux with namespace/cgroup support
//!
//! Run with: sudo cargo test -p mvirt-one --test integration_test -- --test-threads=1

mod common;

use common::{TestServer, check_port};
use mvirt_one::proto::{
    ContainerSpec, CreatePodRequest, DeletePodRequest, Empty, GetPodRequest, PodState,
    StartPodRequest, StopPodRequest, uos_service_client::UosServiceClient,
};

/// Test: Health Check - verify server is running and responds.
#[tokio::test]
async fn test_health() {
    let server = TestServer::start().await.expect("Failed to start server");
    let mut client = UosServiceClient::connect(server.addr.clone())
        .await
        .expect("Failed to connect to server");

    let response = client.health(Empty {}).await.expect("Health check failed");
    let health = response.into_inner();

    assert!(health.healthy);
    assert!(!health.version.is_empty());
}

/// Test: Full Pod Lifecycle - create, start, get, stop, delete.
#[tokio::test]
async fn test_pod_lifecycle() {
    let server = TestServer::start().await.expect("Failed to start server");
    let mut client = UosServiceClient::connect(server.addr.clone())
        .await
        .expect("Failed to connect to server");

    // 1. Create Pod with alpine
    let pod = client
        .create_pod(CreatePodRequest {
            id: "test-pod-1".into(),
            name: "lifecycle-test".into(),
            containers: vec![ContainerSpec {
                id: "container-1".into(),
                name: "alpine".into(),
                image: "docker.io/library/alpine:latest".into(),
                command: vec!["sleep".into()],
                args: vec!["30".into()],
                env: vec![],
                working_dir: String::new(),
            }],
        })
        .await
        .expect("CreatePod failed")
        .into_inner();

    assert_eq!(pod.state, PodState::Created as i32);

    // 2. Start Pod
    let pod = client
        .start_pod(StartPodRequest {
            id: "test-pod-1".into(),
        })
        .await
        .expect("StartPod failed")
        .into_inner();

    assert_eq!(pod.state, PodState::Running as i32);

    // 3. Get Pod - verify status
    let pod = client
        .get_pod(GetPodRequest {
            id: "test-pod-1".into(),
        })
        .await
        .expect("GetPod failed")
        .into_inner();

    assert_eq!(pod.state, PodState::Running as i32);
    assert!(!pod.containers.is_empty());

    // 4. Stop Pod
    let pod = client
        .stop_pod(StopPodRequest {
            id: "test-pod-1".into(),
            timeout_seconds: 5,
        })
        .await
        .expect("StopPod failed")
        .into_inner();

    assert_eq!(pod.state, PodState::Stopped as i32);

    // 5. Delete Pod
    client
        .delete_pod(DeletePodRequest {
            id: "test-pod-1".into(),
            force: false,
        })
        .await
        .expect("DeletePod failed");

    // Verify deleted
    let result = client
        .get_pod(GetPodRequest {
            id: "test-pod-1".into(),
        })
        .await;

    assert!(result.is_err(), "Pod should not exist after deletion");
}

/// Test: HTTP Server with Port Binding.
///
/// Uses busybox httpd to test port binding since it's simpler than nginx
/// and doesn't require config file changes for non-privileged ports.
#[tokio::test]
async fn test_httpd_port_binding() {
    let server = TestServer::start().await.expect("Failed to start server");
    let mut client = UosServiceClient::connect(server.addr.clone())
        .await
        .expect("Failed to connect to server");

    const TEST_PORT: u16 = 8080;

    // Create Pod with busybox httpd
    client
        .create_pod(CreatePodRequest {
            id: "httpd-test".into(),
            name: "httpd-port-test".into(),
            containers: vec![ContainerSpec {
                id: "httpd".into(),
                name: "httpd".into(),
                image: "docker.io/library/busybox:latest".into(),
                command: vec!["httpd".into()],
                args: vec![
                    "-f".into(), // foreground
                    "-p".into(),
                    TEST_PORT.to_string(), // port
                    "-h".into(),
                    "/tmp".into(), // document root
                ],
                env: vec![],
                working_dir: String::new(),
            }],
        })
        .await
        .expect("CreatePod failed");

    // Start Pod
    client
        .start_pod(StartPodRequest {
            id: "httpd-test".into(),
        })
        .await
        .expect("StartPod failed");

    // Wait for httpd to start
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Verify port is open
    assert!(
        check_port(TEST_PORT).await,
        "Port {} should be open",
        TEST_PORT
    );

    // Cleanup
    client
        .stop_pod(StopPodRequest {
            id: "httpd-test".into(),
            timeout_seconds: 5,
        })
        .await
        .expect("StopPod failed");

    client
        .delete_pod(DeletePodRequest {
            id: "httpd-test".into(),
            force: false,
        })
        .await
        .expect("DeletePod failed");

    // Verify port is closed after cleanup
    assert!(
        !check_port(TEST_PORT).await,
        "Port {} should be closed after cleanup",
        TEST_PORT
    );
}

/// Test: List Pods.
#[tokio::test]
async fn test_list_pods() {
    let server = TestServer::start().await.expect("Failed to start server");
    let mut client = UosServiceClient::connect(server.addr.clone())
        .await
        .expect("Failed to connect to server");

    // Initially: no pods
    let response = client.list_pods(Empty {}).await.expect("ListPods failed");
    assert!(response.into_inner().pods.is_empty());

    // Create a pod
    client
        .create_pod(CreatePodRequest {
            id: "list-test".into(),
            name: "list-test".into(),
            containers: vec![ContainerSpec {
                id: "c1".into(),
                name: "test".into(),
                image: "docker.io/library/alpine:latest".into(),
                command: vec!["true".into()],
                args: vec![],
                env: vec![],
                working_dir: String::new(),
            }],
        })
        .await
        .expect("CreatePod failed");

    // Now: 1 pod
    let response = client.list_pods(Empty {}).await.expect("ListPods failed");
    assert_eq!(response.into_inner().pods.len(), 1);

    // Cleanup
    client
        .delete_pod(DeletePodRequest {
            id: "list-test".into(),
            force: true,
        })
        .await
        .expect("DeletePod failed");
}

/// Test: Error handling for non-existent pod.
#[tokio::test]
async fn test_get_nonexistent_pod() {
    let server = TestServer::start().await.expect("Failed to start server");
    let mut client = UosServiceClient::connect(server.addr.clone())
        .await
        .expect("Failed to connect to server");

    let result = client
        .get_pod(GetPodRequest {
            id: "nonexistent-pod".into(),
        })
        .await;

    assert!(result.is_err(), "Should return error for nonexistent pod");
}
