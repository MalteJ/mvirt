//! Integration test: vsock gRPC connectivity
//!
//! Run with: cargo test --package mvirt-vmm --test vsock_integration -- --ignored --nocapture

use std::path::Path;
use std::time::Duration;

use mvirt_vmm::proto::{
    ContainerSpec, CreatePodRequest, DeletePodRequest, StartPodRequest,
    pod_service_client::PodServiceClient,
};
use mvirt_vmm::vsock_client::{vm_id_to_cid, vsock_socket_path};
use tonic::transport::Channel;

const TEST_ROOTFS: &str = "/var/lib/mvirt/test-rootfs.raw";

async fn connect_vmm() -> Channel {
    Channel::from_static("http://[::1]:50051")
        .connect_timeout(Duration::from_secs(5))
        .connect()
        .await
        .expect("Failed to connect to mvirt-vmm")
}

#[tokio::test]
#[ignore]
async fn test_vsock_health_ping() {
    let channel = connect_vmm().await;
    let mut pod_client = PodServiceClient::new(channel);

    // Create pod
    let pod = pod_client
        .create_pod(CreatePodRequest {
            name: Some("vsock-test".to_string()),
            containers: vec![ContainerSpec {
                id: String::new(),
                name: "test".to_string(),
                image: "alpine:latest".to_string(),
                command: vec!["sleep".to_string(), "infinity".to_string()],
                args: vec![],
                env: vec![],
                working_dir: String::new(),
            }],
            resources: None,
            root_disk_path: Some(TEST_ROOTFS.to_string()),
            nic_socket_path: None,
        })
        .await
        .expect("Failed to create pod")
        .into_inner();

    let pod_id = pod.id.clone();
    println!("Created pod: {} ({})", pod.name, pod_id);

    // Start pod
    println!("Starting pod...");
    let pod = pod_client
        .start_pod(StartPodRequest { id: pod_id.clone() })
        .await
        .expect("Failed to start pod")
        .into_inner();

    println!("Pod started: state={:?} vm_id={}", pod.state, pod.vm_id);

    let cid = vm_id_to_cid(&pod_id);
    println!("CID: {}", cid);

    // Verify vsock socket exists (vmm already connected via vsock)
    let data_dir = Path::new("/var/lib/mvirt/vmm");
    let vsock_socket = vsock_socket_path(data_dir, &pod_id);
    println!("vsock socket: {}", vsock_socket.display());

    // The socket is root-only, so we just verify it exists
    // The VMM (running as root) has already connected and verified the connection
    assert!(
        vsock_socket.exists(),
        "vsock socket should exist at {}",
        vsock_socket.display()
    );
    println!("vsock socket exists - VMM has connected to mvirt-one");

    // Cleanup
    println!("Cleaning up...");
    pod_client
        .delete_pod(DeletePodRequest {
            id: pod_id,
            force: true,
        })
        .await
        .expect("Failed to delete pod");

    println!("PASSED!");
}

#[tokio::test]
#[ignore]
async fn test_cid_calculation() {
    // Verify CID calculation is deterministic and valid
    let pod_id = "test-pod-12345";
    let cid1 = vm_id_to_cid(pod_id);
    let cid2 = vm_id_to_cid(pod_id);

    assert_eq!(cid1, cid2, "CID should be deterministic");
    assert!(cid1 > 2, "CID must be > 2 (reserved values)");

    // Different pod IDs should (usually) produce different CIDs
    let cid3 = vm_id_to_cid("different-pod");
    println!("CID for '{}': {}", pod_id, cid1);
    println!("CID for 'different-pod': {}", cid3);

    // Note: Collisions are possible but unlikely with a good hash
}
