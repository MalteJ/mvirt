//! End-to-end integration test
//!
//! Tests the full flow: mvirt-net → mvirt-vmm → mvirt-one → nginx container → curl
//!
//! Prerequisites:
//! - mvirt-net running on localhost:50054
//! - mvirt-vmm running on localhost:50051
//! - Base rootfs at /usr/share/mvirt/one/rootfs.raw
//! - Internet access for pulling nginx image
//!
//! Run with: cargo test --package mvirt-vmm --test e2e_integration -- --ignored --nocapture

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mvirt_net::grpc::proto::{
    CreateNetworkRequest, CreateNicRequest, DeleteNetworkRequest, DeleteNicRequest,
    GetNetworkRequest, get_network_request, net_service_client::NetServiceClient,
};
use mvirt_vmm::proto::{
    ContainerSpec, CreatePodRequest, DeletePodRequest, GetPodNetworkInfoRequest, PodResources,
    StartPodRequest, pod_service_client::PodServiceClient,
};
use tokio::time::sleep;

// Use tonic from mvirt-vmm for VMM client
use tonic::transport::Channel as VmmChannel;

const TEST_NETWORK_SUBNET: &str = "100.64.23.0/24";
const TEST_NETWORK_NAME: &str = "e2e-test-net";
const BASE_ROOTFS: &str = "/usr/share/mvirt/one/rootfs.raw";
const TEST_ROOTFS: &str = "/tmp/e2e-test-rootfs.raw";

async fn connect_vmm() -> VmmChannel {
    VmmChannel::from_static("http://[::1]:50051")
        .connect_timeout(Duration::from_secs(5))
        .connect()
        .await
        .expect("Failed to connect to mvirt-vmm")
}

async fn connect_net() -> mvirt_net::tonic::transport::Channel {
    mvirt_net::tonic::transport::Channel::from_static("http://[::1]:50054")
        .connect_timeout(Duration::from_secs(5))
        .connect()
        .await
        .expect("Failed to connect to mvirt-net")
}

/// Prepare test rootfs: copy base rootfs and expand to 2GB for container storage
async fn prepare_test_rootfs() -> PathBuf {
    let test_rootfs = PathBuf::from(TEST_ROOTFS);

    // Remove old test rootfs if exists
    if test_rootfs.exists() {
        tokio::fs::remove_file(&test_rootfs)
            .await
            .expect("Failed to remove old test rootfs");
    }

    // Copy base rootfs
    println!(
        "Copying base rootfs from {} to {}",
        BASE_ROOTFS, TEST_ROOTFS
    );
    tokio::fs::copy(BASE_ROOTFS, &test_rootfs)
        .await
        .expect("Failed to copy base rootfs");

    // Expand to 2GB
    println!("Expanding rootfs to 2GB...");
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&test_rootfs)
        .await
        .expect("Failed to open test rootfs");
    file.set_len(2 * 1024 * 1024 * 1024)
        .await
        .expect("Failed to expand rootfs");

    // Check and resize ext4 filesystem
    println!("Checking ext4 filesystem...");
    let output = Command::new("/usr/sbin/e2fsck")
        .args(["-f", "-y"])
        .arg(&test_rootfs)
        .output()
        .expect("Failed to run e2fsck");

    // e2fsck returns 0 (clean) or 1 (errors corrected) as success
    if !output.status.success() && output.status.code() != Some(1) {
        panic!("e2fsck failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Resizing ext4 filesystem...");
    let output = Command::new("/usr/sbin/resize2fs")
        .arg(&test_rootfs)
        .output()
        .expect("Failed to run resize2fs");

    if !output.status.success() {
        panic!(
            "resize2fs failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    test_rootfs
}

#[tokio::test]
#[ignore]
async fn test_e2e_nginx() {
    println!("\n=== E2E Integration Test: nginx container ===\n");

    // Connect to services (separate channels for different tonic versions)
    let vmm_channel = connect_vmm().await;
    let net_channel = connect_net().await;
    let mut pod_client = PodServiceClient::new(vmm_channel);
    let mut net_client = NetServiceClient::new(net_channel);

    // 0. Prepare test rootfs
    println!("Step 0: Preparing test rootfs...");
    let test_rootfs = prepare_test_rootfs().await;
    println!("  rootfs ready at: {}\n", test_rootfs.display());

    // 1. Create test network
    println!("Step 1: Creating network {}...", TEST_NETWORK_NAME);
    let network = net_client
        .create_network(CreateNetworkRequest {
            name: TEST_NETWORK_NAME.into(),
            ipv4_enabled: true,
            ipv4_subnet: TEST_NETWORK_SUBNET.into(),
            ipv6_enabled: false,
            ipv6_prefix: String::new(),
            dns_servers: vec!["1.1.1.1".into()],
            ntp_servers: vec![],
            is_public: true, // Enable internet access for pulling nginx image
        })
        .await
        .expect("Failed to create network")
        .into_inner();
    let network_id = network.id.clone();
    println!("  Network created: {} ({})\n", network.name, network.id);

    // 2. Create NIC
    println!("Step 2: Creating NIC in network...");
    let nic = net_client
        .create_nic(CreateNicRequest {
            network_id: network_id.clone(),
            name: "e2e-nginx-nic".into(),
            mac_address: String::new(),
            ipv4_address: String::new(),
            ipv6_address: String::new(),
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
        })
        .await
        .expect("Failed to create NIC")
        .into_inner();
    let nic_id = nic.id.clone();
    let nic_socket = nic.socket_path.clone();
    let nic_mac = nic.mac_address.clone();
    let nic_ip = nic.ipv4_address.clone();
    println!("  NIC created: {} ({})", nic.name, nic.id);
    println!("  Socket: {}", nic_socket);
    println!("  MAC: {}", nic_mac);
    println!("  IP: {}\n", nic_ip);

    // 3. Create pod with nginx container
    // Note: We explicitly specify the command to bypass docker-entrypoint.sh
    // because the entrypoint script uses features not available in the minimal MicroVM.
    // This is equivalent to: docker run nginx:alpine nginx -g "daemon off;"
    println!("Step 3: Creating pod with nginx container...");
    let pod = pod_client
        .create_pod(CreatePodRequest {
            name: Some("e2e-nginx".into()),
            containers: vec![ContainerSpec {
                id: String::new(),
                name: "nginx".into(),
                image: "nginx:alpine".into(),
                command: vec!["nginx".into()],
                args: vec!["-g".into(), "daemon off;".into()],
                env: vec![],
                working_dir: String::new(),
            }],
            resources: Some(PodResources {
                vcpus: 1,
                memory_mb: 512,
                disk_size_gb: 0,
            }),
            root_disk_path: Some(test_rootfs.to_string_lossy().into()),
            nic_socket_path: Some(nic_socket),
            nic_mac_address: Some(nic_mac),
        })
        .await
        .expect("Failed to create pod")
        .into_inner();
    let pod_id = pod.id.clone();
    println!("  Pod created: {} ({})\n", pod.name, pod_id);

    // 4. Start pod
    println!("Step 4: Starting pod...");
    let pod = pod_client
        .start_pod(StartPodRequest { id: pod_id.clone() })
        .await
        .expect("Failed to start pod")
        .into_inner();
    println!("  Pod started: state={:?}\n", pod.state);

    // 5. Wait for nginx to start (container pull + nginx startup)
    println!("Step 5: Waiting for nginx to start (60s)...");
    sleep(Duration::from_secs(60)).await;
    println!("  Wait complete\n");

    // 6. Curl nginx
    println!("Step 6: Testing HTTP connection to nginx at {}...", nic_ip);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    let url = format!("http://{}/", nic_ip);
    let response = client.get(&url).send().await.expect("Failed to curl nginx");

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    println!("  HTTP Status: {}", status);
    println!("  Response contains 'nginx': {}", body.contains("nginx"));

    assert!(status.is_success(), "Expected successful HTTP response");
    assert!(
        body.contains("nginx") || body.contains("Welcome"),
        "Expected nginx welcome page"
    );
    println!("\n  E2E test passed: nginx reachable at {}\n", nic_ip);

    // 7. Cleanup
    println!("Step 7: Cleaning up...");
    pod_client
        .delete_pod(DeletePodRequest {
            id: pod_id,
            force: true,
        })
        .await
        .expect("Failed to delete pod");
    println!("  Pod deleted");

    net_client
        .delete_nic(DeleteNicRequest { id: nic_id })
        .await
        .expect("Failed to delete NIC");
    println!("  NIC deleted");

    net_client
        .delete_network(DeleteNetworkRequest {
            id: network_id,
            force: false,
        })
        .await
        .expect("Failed to delete network");
    println!("  Network deleted");

    tokio::fs::remove_file(&test_rootfs).await.ok();
    println!("  Test rootfs deleted");

    println!("\n=== E2E Test PASSED ===\n");
}

/// Test DHCP/NDP configuration: verify that mvirt-one receives correct IP from mvirt-net.
///
/// This is a simpler test than the full nginx E2E test - it only checks that the
/// network configuration works correctly without pulling container images.
#[tokio::test]
#[ignore]
async fn test_dhcp_network_info() {
    println!("\n=== DHCP Network Info Test ===\n");

    // Connect to services
    let vmm_channel = connect_vmm().await;
    let net_channel = connect_net().await;
    let mut pod_client = PodServiceClient::new(vmm_channel);
    let mut net_client = NetServiceClient::new(net_channel);

    // 0. Prepare test rootfs
    println!("Step 0: Preparing test rootfs...");
    let test_rootfs = prepare_test_rootfs().await;
    println!("  rootfs ready at: {}\n", test_rootfs.display());

    // 1. Create test network (clean up existing one first)
    const DHCP_TEST_NETWORK: &str = "dhcp-test-net";
    const DHCP_TEST_SUBNET: &str = "100.64.99.0/24";

    // Try to delete existing network first (ignore errors)
    println!("Step 1: Creating network {}...", DHCP_TEST_NETWORK);
    if let Ok(existing) = net_client
        .get_network(GetNetworkRequest {
            identifier: Some(get_network_request::Identifier::Name(
                DHCP_TEST_NETWORK.into(),
            )),
        })
        .await
    {
        println!("  Cleaning up existing network...");
        let _ = net_client
            .delete_network(DeleteNetworkRequest {
                id: existing.into_inner().id,
                force: true,
            })
            .await;
    }
    let network = net_client
        .create_network(CreateNetworkRequest {
            name: DHCP_TEST_NETWORK.into(),
            ipv4_enabled: true,
            ipv4_subnet: DHCP_TEST_SUBNET.into(),
            ipv6_enabled: false,
            ipv6_prefix: String::new(),
            dns_servers: vec!["1.1.1.1".into(), "8.8.8.8".into()],
            ntp_servers: vec![],
            is_public: false, // No internet needed for this test
        })
        .await
        .expect("Failed to create network")
        .into_inner();
    let network_id = network.id.clone();
    println!("  Network created: {} ({})\n", network.name, network.id);

    // 2. Create NIC
    println!("Step 2: Creating NIC in network...");
    let nic = net_client
        .create_nic(CreateNicRequest {
            network_id: network_id.clone(),
            name: "dhcp-test-nic".into(),
            mac_address: String::new(),
            ipv4_address: String::new(),
            ipv6_address: String::new(),
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
        })
        .await
        .expect("Failed to create NIC")
        .into_inner();
    let nic_id = nic.id.clone();
    let nic_socket = nic.socket_path.clone();
    let nic_mac = nic.mac_address.clone();
    let expected_ip = nic.ipv4_address.clone();
    println!("  NIC created: {} ({})", nic.name, nic.id);
    println!("  Socket: {}", nic_socket);
    println!("  MAC: {}", nic_mac);
    println!("  Expected IP (from mvirt-net): {}\n", expected_ip);

    // 3. Create pod with simple sleep container (no image pull needed if cached)
    println!("Step 3: Creating pod with sleep container...");
    let pod = pod_client
        .create_pod(CreatePodRequest {
            name: Some("dhcp-test-pod".into()),
            containers: vec![ContainerSpec {
                id: String::new(),
                name: "sleeper".into(),
                image: "alpine:latest".into(),
                command: vec!["sleep".into(), "infinity".into()],
                args: vec![],
                env: vec![],
                working_dir: String::new(),
            }],
            resources: Some(PodResources {
                vcpus: 1,
                memory_mb: 256,
                disk_size_gb: 0,
            }),
            root_disk_path: Some(test_rootfs.to_string_lossy().into()),
            nic_socket_path: Some(nic_socket),
            nic_mac_address: Some(nic_mac),
        })
        .await
        .expect("Failed to create pod")
        .into_inner();
    let pod_id = pod.id.clone();
    println!("  Pod created: {} ({})\n", pod.name, pod_id);

    // 4. Start pod
    println!("Step 4: Starting pod...");
    let pod = pod_client
        .start_pod(StartPodRequest { id: pod_id.clone() })
        .await
        .expect("Failed to start pod")
        .into_inner();
    println!("  Pod started: state={:?}\n", pod.state);

    // 5. Wait for MicroVM boot + DHCP to complete
    println!("Step 5: Waiting for DHCP configuration (15s)...");
    sleep(Duration::from_secs(15)).await;
    println!("  Wait complete\n");

    // 6. Get network info from mvirt-one
    println!("Step 6: Getting network info from pod...");
    let net_info = pod_client
        .get_pod_network_info(GetPodNetworkInfoRequest {
            pod_id: pod_id.clone(),
        })
        .await
        .expect("Failed to get network info")
        .into_inner();

    println!("  Interfaces found: {}", net_info.interfaces.len());
    for iface in &net_info.interfaces {
        println!("  Interface: {}", iface.name);
        println!("    MAC: {}", iface.mac_address);
        println!("    IPv4: {}", iface.ipv4_address);
        println!("    Netmask: {}", iface.ipv4_netmask);
        println!("    Gateway: {}", iface.ipv4_gateway);
        println!("    DNS: {:?}", iface.ipv4_dns);
        if !iface.ipv6_address.is_empty() {
            println!("    IPv6: {}", iface.ipv6_address);
            println!("    IPv6 Gateway: {}", iface.ipv6_gateway);
        }
    }

    // 7. Verify IP address matches
    println!("\nStep 7: Verifying IP address...");
    let actual_ip = net_info
        .interfaces
        .iter()
        .find(|iface| !iface.ipv4_address.is_empty())
        .map(|iface| iface.ipv4_address.as_str())
        .unwrap_or("");

    println!("  Expected IP: {}", expected_ip);
    println!("  Actual IP:   {}", actual_ip);

    assert_eq!(
        actual_ip, expected_ip,
        "Pod IP should match NIC IP assigned by mvirt-net"
    );
    println!("  IP verification PASSED!\n");

    // 8. Cleanup
    println!("Step 8: Cleaning up...");
    pod_client
        .delete_pod(DeletePodRequest {
            id: pod_id,
            force: true,
        })
        .await
        .expect("Failed to delete pod");
    println!("  Pod deleted");

    net_client
        .delete_nic(DeleteNicRequest { id: nic_id })
        .await
        .expect("Failed to delete NIC");
    println!("  NIC deleted");

    net_client
        .delete_network(DeleteNetworkRequest {
            id: network_id,
            force: false,
        })
        .await
        .expect("Failed to delete network");
    println!("  Network deleted");

    tokio::fs::remove_file(&test_rootfs).await.ok();
    println!("  Test rootfs deleted");

    println!("\n=== DHCP Network Info Test PASSED ===\n");
}
