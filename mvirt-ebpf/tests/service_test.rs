//! gRPC service integration tests.
//!
//! Tests the EbpfNetService CRUD operations for networks and NICs.
//!
//! These tests don't require CAP_NET_ADMIN as they use in-memory storage
//! and don't create actual TAP devices.

use mvirt_ebpf::grpc::{NetworkData, NicData, NicState, Storage};
use std::net::{IpAddr, Ipv4Addr};
use uuid::Uuid;

/// Create a test network in storage
fn create_test_network(storage: &Storage) -> NetworkData {
    let network = NetworkData {
        id: Uuid::new_v4(),
        name: format!("test-net-{}", &Uuid::new_v4().to_string()[..8]),
        ipv4_enabled: true,
        ipv4_subnet: Some("10.0.0.0/24".parse().unwrap()),
        ipv6_enabled: false,
        ipv6_prefix: None,
        dns_servers: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
        ntp_servers: vec![],
        is_public: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage
        .create_network(&network)
        .expect("Failed to create network");
    network
}

/// Create a test NIC in storage
fn create_test_nic(storage: &Storage, network_id: Uuid) -> NicData {
    let nic = NicData {
        id: Uuid::new_v4(),
        network_id,
        name: Some(format!("test-nic-{}", &Uuid::new_v4().to_string()[..8])),
        mac_address: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 100)),
        ipv6_address: None,
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        tap_name: format!("tap_{}", &Uuid::new_v4().to_string()[..7]),
        state: NicState::Created,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_nic(&nic).expect("Failed to create NIC");
    nic
}

#[test]
fn test_storage_create_network() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Verify network was created
    let fetched = storage.get_network_by_id(&network.id).expect("Get failed");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.name, network.name);
    assert!(fetched.ipv4_enabled);
    assert_eq!(fetched.ipv4_subnet, network.ipv4_subnet);
}

#[test]
fn test_storage_create_nic() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);

    // Verify NIC was created
    let fetched = storage.get_nic_by_id(&nic.id).expect("Get failed");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.name, nic.name);
    assert_eq!(fetched.network_id, network.id);
    assert_eq!(fetched.ipv4_address, nic.ipv4_address);
}

#[test]
fn test_storage_list_networks() {
    let storage = Storage::in_memory().expect("Failed to create storage");

    // Create multiple networks
    for _ in 0..3 {
        create_test_network(&storage);
    }

    let networks = storage.list_networks().expect("List failed");
    assert_eq!(networks.len(), 3);
}

#[test]
fn test_storage_list_nics_in_network() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Create multiple NICs
    for i in 0u8..3 {
        let nic = NicData {
            id: Uuid::new_v4(),
            network_id: network.id,
            name: Some(format!("nic-{}", i)),
            mac_address: [0x52, 0x54, 0x00, 0x00, 0x00, i],
            ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 100 + i)),
            ipv6_address: None,
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
            tap_name: format!("tap_{}", i),
            state: NicState::Created,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage.create_nic(&nic).expect("Failed to create NIC");
    }

    let nics = storage
        .list_nics_in_network(&network.id)
        .expect("List failed");
    assert_eq!(nics.len(), 3);
}

#[test]
fn test_storage_delete_network() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Delete network
    let deleted = storage.delete_network(&network.id).expect("Delete failed");
    assert!(deleted);

    // Verify it's gone
    let fetched = storage.get_network_by_id(&network.id).expect("Get failed");
    assert!(fetched.is_none());
}

#[test]
fn test_storage_delete_nic() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);

    // Delete NIC
    let deleted = storage.delete_nic(&nic.id).expect("Delete failed");
    assert!(deleted);

    // Verify it's gone
    let fetched = storage.get_nic_by_id(&nic.id).expect("Get failed");
    assert!(fetched.is_none());
}

#[test]
fn test_storage_update_nic_state() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);

    // Update state
    storage
        .update_nic_state(&nic.id, NicState::Active)
        .expect("Update failed");

    // Verify state changed
    let fetched = storage.get_nic_by_id(&nic.id).expect("Get failed").unwrap();
    assert_eq!(fetched.state, NicState::Active);
}

#[test]
fn test_storage_network_name_unique() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Try to create another network with same name
    let dup_network = NetworkData {
        id: Uuid::new_v4(),
        name: network.name.clone(),
        ..network.clone()
    };
    let result = storage.create_network(&dup_network);
    assert!(result.is_err());
}

#[test]
fn test_storage_ipv4_in_use() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);

    let ip = nic.ipv4_address.unwrap();

    // Check IP is in use
    let in_use = storage
        .is_ipv4_in_use(&network.id, ip)
        .expect("Check failed");
    assert!(in_use);

    // Check unused IP
    let unused_ip = Ipv4Addr::new(10, 0, 0, 200);
    let in_use = storage
        .is_ipv4_in_use(&network.id, unused_ip)
        .expect("Check failed");
    assert!(!in_use);
}

#[test]
fn test_storage_get_used_ipv4_addresses() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Create NICs with different IPs
    for i in 0u8..3 {
        let nic = NicData {
            id: Uuid::new_v4(),
            network_id: network.id,
            name: Some(format!("nic-{}", i)),
            mac_address: [0x52, 0x54, 0x00, 0x00, 0x00, i],
            ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 100 + i)),
            ipv6_address: None,
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
            tap_name: format!("tap_{}", i),
            state: NicState::Created,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage.create_nic(&nic).expect("Failed to create NIC");
    }

    let used = storage
        .get_used_ipv4_addresses(&network.id)
        .expect("Get failed");
    assert_eq!(used.len(), 3);
    assert!(used.contains(&Ipv4Addr::new(10, 0, 0, 100)));
    assert!(used.contains(&Ipv4Addr::new(10, 0, 0, 101)));
    assert!(used.contains(&Ipv4Addr::new(10, 0, 0, 102)));
}

#[test]
fn test_storage_count_nics_in_network() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Initially no NICs
    let count = storage
        .count_nics_in_network(&network.id)
        .expect("Count failed");
    assert_eq!(count, 0);

    // Add NICs
    for i in 0u8..3 {
        let nic = NicData {
            id: Uuid::new_v4(),
            network_id: network.id,
            name: Some(format!("nic-{}", i)),
            mac_address: [0x52, 0x54, 0x00, 0x00, 0x00, i],
            ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 100 + i)),
            ipv6_address: None,
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
            tap_name: format!("tap_{}", i),
            state: NicState::Created,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage.create_nic(&nic).expect("Failed to create NIC");
    }

    let count = storage
        .count_nics_in_network(&network.id)
        .expect("Count failed");
    assert_eq!(count, 3);
}

#[test]
fn test_storage_network_gateway() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Gateway should be first usable address in subnet (10.0.0.1)
    let gateway = network.ipv4_gateway();
    assert!(gateway.is_some());
    assert_eq!(gateway.unwrap(), Ipv4Addr::new(10, 0, 0, 1));
}

#[test]
fn test_storage_update_network_dns() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Update DNS servers
    let new_dns: Vec<IpAddr> = vec![
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1)),
    ];
    storage
        .update_network(&network.id, &new_dns, &[])
        .expect("Update failed");

    // Verify DNS changed
    let fetched = storage
        .get_network_by_id(&network.id)
        .expect("Get failed")
        .unwrap();
    assert_eq!(fetched.dns_servers.len(), 2);
    assert!(
        fetched
            .dns_servers
            .contains(&IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
    );
}

#[test]
fn test_nic_mac_string() {
    let nic = NicData {
        id: Uuid::new_v4(),
        network_id: Uuid::new_v4(),
        name: None,
        mac_address: [0x02, 0xab, 0xcd, 0xef, 0x12, 0x34],
        ipv4_address: None,
        ipv6_address: None,
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        tap_name: "tap_test".into(),
        state: NicState::Created,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    assert_eq!(nic.mac_string(), "02:ab:cd:ef:12:34");
}
