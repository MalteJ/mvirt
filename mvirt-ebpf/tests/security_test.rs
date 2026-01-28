//! Security group integration tests.
//!
//! Tests the Security Group CRUD operations and rule validation.
//!
//! These tests don't require CAP_NET_ADMIN as they use in-memory storage
//! and don't create actual TAP devices.

use mvirt_ebpf::grpc::{
    NetworkData, NicData, NicState, RuleDirection, RuleProtocol, SecurityGroupData,
    SecurityGroupRuleData, Storage,
};
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

/// Create a test security group
fn create_test_security_group(storage: &Storage, name: &str) -> SecurityGroupData {
    let sg = SecurityGroupData {
        id: Uuid::new_v4(),
        name: name.to_string(),
        description: Some("Test security group".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage
        .create_security_group(&sg)
        .expect("Failed to create security group");
    sg
}

// ============================================================================
// Security Group CRUD Tests
// ============================================================================

#[test]
fn test_create_security_group() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "test-sg");

    // Verify it was created
    let fetched = storage
        .get_security_group_by_id(&sg.id)
        .expect("Get failed");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.name, "test-sg");
    assert_eq!(fetched.description, Some("Test security group".to_string()));
}

#[test]
fn test_get_security_group_by_name() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "my-sg");

    let fetched = storage
        .get_security_group_by_name("my-sg")
        .expect("Get by name failed");
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().id, sg.id);
}

#[test]
fn test_list_security_groups() {
    let storage = Storage::in_memory().expect("Failed to create storage");

    // Create multiple security groups
    for i in 0..3 {
        create_test_security_group(&storage, &format!("sg-{}", i));
    }

    let groups = storage.list_security_groups().expect("List failed");
    assert_eq!(groups.len(), 3);
}

#[test]
fn test_delete_security_group() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "delete-me");

    // Delete it
    let deleted = storage
        .delete_security_group(&sg.id)
        .expect("Delete failed");
    assert!(deleted);

    // Verify it's gone
    let fetched = storage
        .get_security_group_by_id(&sg.id)
        .expect("Get failed");
    assert!(fetched.is_none());
}

#[test]
fn test_security_group_name_unique() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    create_test_security_group(&storage, "unique-name");

    // Try to create another with same name
    let dup = SecurityGroupData {
        id: Uuid::new_v4(),
        name: "unique-name".to_string(),
        description: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let result = storage.create_security_group(&dup);
    assert!(result.is_err());
}

// ============================================================================
// Security Group Rule Tests
// ============================================================================

#[test]
fn test_add_security_group_rule() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "rule-test");

    // Add a rule
    let rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(22),
        port_end: Some(22),
        cidr: Some("0.0.0.0/0".to_string()),
        description: Some("SSH access".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage
        .create_security_group_rule(&rule)
        .expect("Failed to create rule");

    // Verify rule was created
    let rules = storage
        .list_rules_for_security_group(&sg.id)
        .expect("List rules failed");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].direction, RuleDirection::Ingress);
    assert_eq!(rules[0].protocol, RuleProtocol::Tcp);
    assert_eq!(rules[0].port_start, Some(22));
}

#[test]
fn test_add_multiple_rules() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "multi-rule-test");

    // Add SSH rule
    let ssh_rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(22),
        port_end: Some(22),
        cidr: Some("10.0.0.0/8".to_string()),
        description: Some("SSH".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&ssh_rule).unwrap();

    // Add HTTP rule
    let http_rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(80),
        port_end: Some(80),
        cidr: Some("0.0.0.0/0".to_string()),
        description: Some("HTTP".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&http_rule).unwrap();

    // Add HTTPS rule
    let https_rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(443),
        port_end: Some(443),
        cidr: Some("0.0.0.0/0".to_string()),
        description: Some("HTTPS".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&https_rule).unwrap();

    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 3);
}

#[test]
fn test_remove_security_group_rule() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "remove-rule-test");

    let rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(22),
        port_end: Some(22),
        cidr: None,
        description: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&rule).unwrap();

    // Remove rule
    let deleted = storage
        .delete_security_group_rule(&rule.id)
        .expect("Delete rule failed");
    assert!(deleted);

    // Verify it's gone
    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 0);
}

#[test]
fn test_rules_deleted_with_security_group() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "cascade-test");

    // Add a rule
    let rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Egress,
        protocol: RuleProtocol::All,
        port_start: None,
        port_end: None,
        cidr: None,
        description: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&rule).unwrap();

    // Delete security group (should cascade delete rules)
    storage.delete_security_group(&sg.id).unwrap();

    // Rules should be gone (foreign key cascade)
    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 0);
}

// ============================================================================
// NIC-Security Group Association Tests
// ============================================================================

#[test]
fn test_attach_security_group_to_nic() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);
    let sg = create_test_security_group(&storage, "attach-test");

    // Attach
    storage
        .attach_security_group(&nic.id, &sg.id)
        .expect("Attach failed");

    // Verify
    let groups = storage
        .list_security_groups_for_nic(&nic.id)
        .expect("List for NIC failed");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, sg.id);
}

#[test]
fn test_detach_security_group_from_nic() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);
    let sg = create_test_security_group(&storage, "detach-test");

    // Attach then detach
    storage.attach_security_group(&nic.id, &sg.id).unwrap();
    storage
        .detach_security_group(&nic.id, &sg.id)
        .expect("Detach failed");

    // Verify
    let groups = storage.list_security_groups_for_nic(&nic.id).unwrap();
    assert_eq!(groups.len(), 0);
}

#[test]
fn test_multiple_security_groups_on_nic() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);

    let sg1 = create_test_security_group(&storage, "multi-sg-1");
    let sg2 = create_test_security_group(&storage, "multi-sg-2");
    let sg3 = create_test_security_group(&storage, "multi-sg-3");

    // Attach all three
    storage.attach_security_group(&nic.id, &sg1.id).unwrap();
    storage.attach_security_group(&nic.id, &sg2.id).unwrap();
    storage.attach_security_group(&nic.id, &sg3.id).unwrap();

    let groups = storage.list_security_groups_for_nic(&nic.id).unwrap();
    assert_eq!(groups.len(), 3);
}

#[test]
fn test_security_group_on_multiple_nics() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);

    // Create two NICs
    let nic1 = NicData {
        id: Uuid::new_v4(),
        network_id: network.id,
        name: Some("nic-1".to_string()),
        mac_address: [0x52, 0x54, 0x00, 0x00, 0x00, 0x01],
        ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 101)),
        ipv6_address: None,
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        tap_name: "tap_1".to_string(),
        state: NicState::Created,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_nic(&nic1).unwrap();

    let nic2 = NicData {
        id: Uuid::new_v4(),
        network_id: network.id,
        name: Some("nic-2".to_string()),
        mac_address: [0x52, 0x54, 0x00, 0x00, 0x00, 0x02],
        ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 102)),
        ipv6_address: None,
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        tap_name: "tap_2".to_string(),
        state: NicState::Created,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_nic(&nic2).unwrap();

    let sg = create_test_security_group(&storage, "shared-sg");

    // Attach to both NICs
    storage.attach_security_group(&nic1.id, &sg.id).unwrap();
    storage.attach_security_group(&nic2.id, &sg.id).unwrap();

    // Both NICs should have the security group
    let groups1 = storage.list_security_groups_for_nic(&nic1.id).unwrap();
    let groups2 = storage.list_security_groups_for_nic(&nic2.id).unwrap();
    assert_eq!(groups1.len(), 1);
    assert_eq!(groups2.len(), 1);
    assert_eq!(groups1[0].id, sg.id);
    assert_eq!(groups2[0].id, sg.id);
}

#[test]
fn test_nic_deleted_removes_sg_associations() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);
    let sg = create_test_security_group(&storage, "nic-cascade-test");

    storage.attach_security_group(&nic.id, &sg.id).unwrap();

    // Delete NIC (should cascade delete associations)
    storage.delete_nic(&nic.id).unwrap();

    // Security group should still exist
    let fetched_sg = storage.get_security_group_by_id(&sg.id).unwrap();
    assert!(fetched_sg.is_some());

    // But NIC associations should be gone
    let groups = storage.list_security_groups_for_nic(&nic.id).unwrap();
    assert_eq!(groups.len(), 0);
}

#[test]
fn test_sg_deleted_removes_nic_associations() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let network = create_test_network(&storage);
    let nic = create_test_nic(&storage, network.id);
    let sg = create_test_security_group(&storage, "sg-cascade-test");

    storage.attach_security_group(&nic.id, &sg.id).unwrap();

    // Delete security group
    storage.delete_security_group(&sg.id).unwrap();

    // NIC should still exist
    let fetched_nic = storage.get_nic_by_id(&nic.id).unwrap();
    assert!(fetched_nic.is_some());

    // But associations should be gone
    let groups = storage.list_security_groups_for_nic(&nic.id).unwrap();
    assert_eq!(groups.len(), 0);
}

// ============================================================================
// Protocol and Direction Tests
// ============================================================================

#[test]
fn test_rule_protocols() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "protocol-test");

    // Test all protocol types
    let protocols = vec![
        RuleProtocol::All,
        RuleProtocol::Tcp,
        RuleProtocol::Udp,
        RuleProtocol::Icmp,
        RuleProtocol::Icmpv6,
    ];

    for (i, protocol) in protocols.iter().enumerate() {
        let rule = SecurityGroupRuleData {
            id: Uuid::new_v4(),
            security_group_id: sg.id,
            direction: RuleDirection::Ingress,
            protocol: *protocol,
            port_start: if *protocol == RuleProtocol::All
                || *protocol == RuleProtocol::Icmp
                || *protocol == RuleProtocol::Icmpv6
            {
                None
            } else {
                Some(1000 + i as u16)
            },
            port_end: if *protocol == RuleProtocol::All
                || *protocol == RuleProtocol::Icmp
                || *protocol == RuleProtocol::Icmpv6
            {
                None
            } else {
                Some(1000 + i as u16)
            },
            cidr: None,
            description: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage
            .create_security_group_rule(&rule)
            .expect(&format!("Failed to create {:?} rule", protocol));
    }

    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 5);
}

#[test]
fn test_rule_directions() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "direction-test");

    // Ingress rule
    let ingress_rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(22),
        port_end: Some(22),
        cidr: None,
        description: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&ingress_rule).unwrap();

    // Egress rule
    let egress_rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Egress,
        protocol: RuleProtocol::All,
        port_start: None,
        port_end: None,
        cidr: None,
        description: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&egress_rule).unwrap();

    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 2);

    let ingress_count = rules
        .iter()
        .filter(|r| r.direction == RuleDirection::Ingress)
        .count();
    let egress_count = rules
        .iter()
        .filter(|r| r.direction == RuleDirection::Egress)
        .count();
    assert_eq!(ingress_count, 1);
    assert_eq!(egress_count, 1);
}

// ============================================================================
// CIDR Tests
// ============================================================================

#[test]
fn test_ipv4_cidr_rules() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "ipv4-cidr-test");

    let cidrs = vec![
        "0.0.0.0/0",      // All IPv4
        "10.0.0.0/8",     // Private class A
        "192.168.1.0/24", // Specific subnet
        "203.0.113.5/32", // Single host
    ];

    for cidr in cidrs {
        let rule = SecurityGroupRuleData {
            id: Uuid::new_v4(),
            security_group_id: sg.id,
            direction: RuleDirection::Ingress,
            protocol: RuleProtocol::Tcp,
            port_start: Some(443),
            port_end: Some(443),
            cidr: Some(cidr.to_string()),
            description: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage
            .create_security_group_rule(&rule)
            .expect(&format!("Failed to create rule with CIDR {}", cidr));
    }

    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 4);
}

#[test]
fn test_ipv6_cidr_rules() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "ipv6-cidr-test");

    let cidrs = vec![
        "::/0",          // All IPv6
        "fd00::/8",      // ULA
        "2001:db8::/32", // Documentation prefix
        "::1/128",       // Localhost
    ];

    for cidr in cidrs {
        let rule = SecurityGroupRuleData {
            id: Uuid::new_v4(),
            security_group_id: sg.id,
            direction: RuleDirection::Ingress,
            protocol: RuleProtocol::Tcp,
            port_start: Some(443),
            port_end: Some(443),
            cidr: Some(cidr.to_string()),
            description: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage
            .create_security_group_rule(&rule)
            .expect(&format!("Failed to create rule with CIDR {}", cidr));
    }

    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 4);
}

#[test]
fn test_port_range_rule() {
    let storage = Storage::in_memory().expect("Failed to create storage");
    let sg = create_test_security_group(&storage, "port-range-test");

    // Rule for port range 8000-9000
    let rule = SecurityGroupRuleData {
        id: Uuid::new_v4(),
        security_group_id: sg.id,
        direction: RuleDirection::Ingress,
        protocol: RuleProtocol::Tcp,
        port_start: Some(8000),
        port_end: Some(9000),
        cidr: None,
        description: Some("High ports".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    storage.create_security_group_rule(&rule).unwrap();

    let rules = storage.list_rules_for_security_group(&sg.id).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].port_start, Some(8000));
    assert_eq!(rules[0].port_end, Some(9000));
}
