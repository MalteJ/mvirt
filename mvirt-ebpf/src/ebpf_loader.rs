//! eBPF program loading and BPF map management.
//!
//! This module handles loading the TC eBPF programs and managing the BPF maps
//! for routing. The actual eBPF programs are compiled separately in mvirt-ebpf-programs.

use aya::maps::{HashMap, LpmTrie, MapData, lpm_trie::Key};
use aya::programs::{SchedClassifier, TcAttachType, tc::TcOptions};
use aya::{Bpf, BpfLoader};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::info;

/// Route action constants (must match eBPF program)
pub const ACTION_DROP: u8 = 0;
pub const ACTION_REDIRECT: u8 = 1;
pub const ACTION_PASS: u8 = 2;

/// Route entry for LPM lookup result.
/// Must match the eBPF struct exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RouteEntry {
    /// Action: 0=drop, 1=redirect, 2=pass
    pub action: u8,
    _padding: [u8; 3],
    /// Target interface index for redirect
    pub target_ifindex: u32,
    /// Destination MAC address
    pub dst_mac: [u8; 6],
    /// Source MAC address (gateway)
    pub src_mac: [u8; 6],
}

impl RouteEntry {
    pub fn new(action: u8, target_ifindex: u32, dst_mac: [u8; 6], src_mac: [u8; 6]) -> Self {
        Self {
            action,
            _padding: [0; 3],
            target_ifindex,
            dst_mac,
            src_mac,
        }
    }
}

unsafe impl aya::Pod for RouteEntry {}

/// Interface MAC address entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IfMac {
    pub mac: [u8; 6],
}

unsafe impl aya::Pod for IfMac {}

/// Security rule for packet filtering.
/// Must match the eBPF struct exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SecurityRule {
    /// Rule is active (1) or disabled (0)
    pub enabled: u8,
    /// Direction: 0=ingress, 1=egress
    pub direction: u8,
    /// Protocol: 0=all, 1=ICMP, 6=TCP, 17=UDP, 58=ICMPv6
    pub protocol: u8,
    /// IP version: 4=IPv4, 6=IPv6, 0=both
    pub ip_version: u8,
    /// Start of port range
    pub port_start: u16,
    /// End of port range
    pub port_end: u16,
    /// CIDR network address (IPv4 in first 4 bytes, IPv6 uses all 16)
    pub cidr_addr: [u8; 16],
    /// CIDR prefix length
    pub cidr_prefix_len: u8,
    _padding: [u8; 3],
}

impl SecurityRule {
    pub fn new(
        direction: u8,
        protocol: u8,
        ip_version: u8,
        port_start: u16,
        port_end: u16,
        cidr_addr: [u8; 16],
        cidr_prefix_len: u8,
    ) -> Self {
        Self {
            enabled: 1,
            direction,
            protocol,
            ip_version,
            port_start,
            port_end,
            cidr_addr,
            cidr_prefix_len,
            _padding: [0; 3],
        }
    }
}

unsafe impl aya::Pod for SecurityRule {}

/// NIC security configuration.
/// Must match the eBPF struct exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NicSecurityConfig {
    /// Whether security filtering is enabled
    pub enabled: u8,
    _padding: [u8; 3],
    /// Start index into SECURITY_RULES map
    pub rules_start: u32,
    /// Number of rules
    pub rules_count: u32,
}

impl NicSecurityConfig {
    pub fn new(enabled: bool, rules_start: u32, rules_count: u32) -> Self {
        Self {
            enabled: if enabled { 1 } else { 0 },
            _padding: [0; 3],
            rules_start,
            rules_count,
        }
    }
}

unsafe impl aya::Pod for NicSecurityConfig {}

/// Connection tracking key.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ConnTrackKey {
    pub src_addr: [u8; 16],
    pub dst_addr: [u8; 16],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
    pub ip_version: u8,
    _padding: [u8; 2],
}

unsafe impl aya::Pod for ConnTrackKey {}

/// Connection tracking entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ConnTrackEntry {
    pub state: u8,
    pub flags: u8,
    _padding: [u8; 2],
    pub last_seen_ns: u64,
    pub packet_count: u64,
}

unsafe impl aya::Pod for ConnTrackEntry {}

/// Tunnel endpoint for remote hypervisors.
/// Maps inner destination subnet to remote HV prefix.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TunnelEndpoint {
    /// Remote hypervisor /80 prefix (for outer dst address construction)
    pub remote_prefix: [u8; 10],
    _pad: [u8; 6],
}

impl TunnelEndpoint {
    pub fn new(remote_prefix: [u8; 10]) -> Self {
        Self {
            remote_prefix,
            _pad: [0; 6],
        }
    }
}

unsafe impl aya::Pod for TunnelEndpoint {}

/// Local NIC metadata for tunnel source address construction.
/// Embedded in outer IPv6 source address for security policy enforcement.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LocalNicInfo {
    /// Network Function ID (identifies VM/NIC)
    pub nf_id: u16,
    /// Security Group ID (24 bits used, for ingress filtering on destination)
    pub sg_id: u32,
    /// Local hypervisor /80 prefix
    pub local_prefix: [u8; 10],
    _pad: [u8; 2],
}

impl LocalNicInfo {
    pub fn new(nf_id: u16, sg_id: u32, local_prefix: [u8; 10]) -> Self {
        Self {
            nf_id,
            sg_id,
            local_prefix,
            _pad: [0; 2],
        }
    }
}

unsafe impl aya::Pod for LocalNicInfo {}

/// eBPF loader errors.
#[derive(Debug, Error)]
pub enum EbpfError {
    #[error("Failed to load eBPF program: {0}")]
    Load(#[from] aya::BpfError),

    #[error("Failed to attach TC program: {0}")]
    Attach(#[from] aya::programs::ProgramError),

    #[error("Failed to access map: {0}")]
    Map(#[from] aya::maps::MapError),

    #[error("Program not found: {0}")]
    ProgramNotFound(String),

    #[error("Map not found: {0}")]
    MapNotFound(String),

    #[error("TC error: {0}")]
    Tc(String),
}

pub type Result<T> = std::result::Result<T, EbpfError>;

/// Manages loaded eBPF programs and their maps.
///
/// Note: This is a stub implementation. The actual eBPF loading requires
/// the compiled eBPF programs from mvirt-ebpf-programs crate.
pub struct EbpfManager {
    /// TC egress program for VM TAPs
    egress_bpf: Arc<RwLock<Option<Bpf>>>,
    /// TC ingress program for TUN device
    ingress_bpf: Arc<RwLock<Option<Bpf>>>,
}

impl EbpfManager {
    /// Create a new EbpfManager without loading programs.
    /// Programs must be loaded separately using load_programs().
    pub fn new() -> Self {
        Self {
            egress_bpf: Arc::new(RwLock::new(None)),
            ingress_bpf: Arc::new(RwLock::new(None)),
        }
    }

    /// Load eBPF programs from the specified paths.
    pub fn load_from_paths(egress_path: &str, ingress_path: &str) -> Result<Self> {
        let egress_bpf = BpfLoader::new().load_file(egress_path)?;
        let ingress_bpf = BpfLoader::new().load_file(ingress_path)?;

        info!("eBPF programs loaded from files");

        Ok(Self {
            egress_bpf: Arc::new(RwLock::new(Some(egress_bpf))),
            ingress_bpf: Arc::new(RwLock::new(Some(ingress_bpf))),
        })
    }

    /// Load eBPF programs from default paths.
    pub fn load() -> Result<Self> {
        // Default paths for installed eBPF programs
        let egress_path = "/usr/lib/mvirt/ebpf/tc-egress";
        let ingress_path = "/usr/lib/mvirt/ebpf/tc-ingress";

        // Check if files exist, otherwise return empty manager
        if !std::path::Path::new(egress_path).exists() {
            info!("eBPF programs not found, using stub implementation");
            return Ok(Self::new());
        }

        Self::load_from_paths(egress_path, ingress_path)
    }

    /// Attach TC egress program to a TAP interface.
    pub async fn attach_egress(&self, if_index: u32, if_name: &str) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => {
                info!(if_name, if_index, "Skipping eBPF attach (stub mode)");
                return Ok(());
            }
        };

        // Add clsact qdisc if not present
        if let Err(e) = aya::programs::tc::qdisc_add_clsact(if_name)
            && !e.to_string().contains("exists")
        {
            return Err(EbpfError::Tc(format!(
                "Failed to add clsact qdisc to {}: {}",
                if_name, e
            )));
        }

        // Get and attach the program
        let prog: &mut SchedClassifier = bpf
            .program_mut("tc_egress")
            .ok_or_else(|| EbpfError::ProgramNotFound("tc_egress".to_string()))?
            .try_into()?;

        prog.load()?;
        prog.attach_with_options(
            if_name,
            TcAttachType::Egress,
            TcOptions {
                priority: 1,
                handle: 1,
            },
        )?;

        info!(if_name, if_index, "TC egress program attached");
        Ok(())
    }

    /// Attach TC ingress program to TUN interface.
    pub async fn attach_ingress(&self, if_index: u32, if_name: &str) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => {
                info!(if_name, if_index, "Skipping eBPF attach (stub mode)");
                return Ok(());
            }
        };

        // Add clsact qdisc if not present
        if let Err(e) = aya::programs::tc::qdisc_add_clsact(if_name)
            && !e.to_string().contains("exists")
        {
            return Err(EbpfError::Tc(format!(
                "Failed to add clsact qdisc to {}: {}",
                if_name, e
            )));
        }

        // Get and attach the program
        let prog: &mut SchedClassifier = bpf
            .program_mut("tc_ingress")
            .ok_or_else(|| EbpfError::ProgramNotFound("tc_ingress".to_string()))?
            .try_into()?;

        prog.load()?;
        prog.attach_with_options(
            if_name,
            TcAttachType::Ingress,
            TcOptions {
                priority: 1,
                handle: 1,
            },
        )?;

        info!(if_name, if_index, "TC ingress program attached");
        Ok(())
    }

    /// Add an IPv4 route to the egress routing table.
    pub async fn add_egress_route_v4(
        &self,
        addr: Ipv4Addr,
        _prefix_len: u8,
        entry: RouteEntry,
    ) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        // For now, we use a HashMap with the full address as key
        // A proper LPM trie would be better but requires more setup
        let mut routes: HashMap<&mut MapData, [u8; 4], RouteEntry> = bpf
            .map_mut("ROUTES_V4")
            .ok_or_else(|| EbpfError::MapNotFound("ROUTES_V4".to_string()))?
            .try_into()?;

        routes.insert(addr.octets(), entry, 0)?;
        Ok(())
    }

    /// Remove an IPv4 route from the egress routing table.
    pub async fn remove_egress_route_v4(&self, addr: Ipv4Addr, _prefix_len: u8) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 4], RouteEntry> = bpf
            .map_mut("ROUTES_V4")
            .ok_or_else(|| EbpfError::MapNotFound("ROUTES_V4".to_string()))?
            .try_into()?;

        routes.remove(&addr.octets())?;
        Ok(())
    }

    /// Add an IPv6 route to the egress routing table.
    pub async fn add_egress_route_v6(
        &self,
        addr: Ipv6Addr,
        _prefix_len: u8,
        entry: RouteEntry,
    ) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 16], RouteEntry> = bpf
            .map_mut("ROUTES_V6")
            .ok_or_else(|| EbpfError::MapNotFound("ROUTES_V6".to_string()))?
            .try_into()?;

        routes.insert(addr.octets(), entry, 0)?;
        Ok(())
    }

    /// Remove an IPv6 route from the egress routing table.
    pub async fn remove_egress_route_v6(&self, addr: Ipv6Addr, _prefix_len: u8) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 16], RouteEntry> = bpf
            .map_mut("ROUTES_V6")
            .ok_or_else(|| EbpfError::MapNotFound("ROUTES_V6".to_string()))?
            .try_into()?;

        routes.remove(&addr.octets())?;
        Ok(())
    }

    /// Add an IPv4 route to the TUN ingress routing table.
    pub async fn add_tun_route_v4(
        &self,
        addr: Ipv4Addr,
        _prefix_len: u8,
        entry: RouteEntry,
    ) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 4], RouteEntry> = bpf
            .map_mut("TUN_ROUTES_V4")
            .ok_or_else(|| EbpfError::MapNotFound("TUN_ROUTES_V4".to_string()))?
            .try_into()?;

        routes.insert(addr.octets(), entry, 0)?;
        Ok(())
    }

    /// Remove an IPv4 route from the TUN ingress routing table.
    pub async fn remove_tun_route_v4(&self, addr: Ipv4Addr, _prefix_len: u8) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 4], RouteEntry> = bpf
            .map_mut("TUN_ROUTES_V4")
            .ok_or_else(|| EbpfError::MapNotFound("TUN_ROUTES_V4".to_string()))?
            .try_into()?;

        routes.remove(&addr.octets())?;
        Ok(())
    }

    /// Add an IPv6 route to the TUN ingress routing table.
    pub async fn add_tun_route_v6(
        &self,
        addr: Ipv6Addr,
        _prefix_len: u8,
        entry: RouteEntry,
    ) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 16], RouteEntry> = bpf
            .map_mut("TUN_ROUTES_V6")
            .ok_or_else(|| EbpfError::MapNotFound("TUN_ROUTES_V6".to_string()))?
            .try_into()?;

        routes.insert(addr.octets(), entry, 0)?;
        Ok(())
    }

    /// Remove an IPv6 route from the TUN ingress routing table.
    pub async fn remove_tun_route_v6(&self, addr: Ipv6Addr, _prefix_len: u8) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut routes: HashMap<&mut MapData, [u8; 16], RouteEntry> = bpf
            .map_mut("TUN_ROUTES_V6")
            .ok_or_else(|| EbpfError::MapNotFound("TUN_ROUTES_V6".to_string()))?
            .try_into()?;

        routes.remove(&addr.octets())?;
        Ok(())
    }

    /// Set interface MAC address in the egress IF_MACS map.
    pub async fn set_egress_if_mac(&self, if_index: u32, mac: [u8; 6]) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut macs: HashMap<&mut MapData, u32, IfMac> = bpf
            .map_mut("IF_MACS")
            .ok_or_else(|| EbpfError::MapNotFound("IF_MACS".to_string()))?
            .try_into()?;

        macs.insert(if_index, IfMac { mac }, 0)?;
        Ok(())
    }

    /// Set interface MAC address in the TUN IF_MACS map.
    pub async fn set_tun_if_mac(&self, if_index: u32, mac: [u8; 6]) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut macs: HashMap<&mut MapData, u32, IfMac> = bpf
            .map_mut("TUN_IF_MACS")
            .ok_or_else(|| EbpfError::MapNotFound("TUN_IF_MACS".to_string()))?
            .try_into()?;

        macs.insert(if_index, IfMac { mac }, 0)?;
        Ok(())
    }

    // ========== Security Rule Management ==========

    /// Set a security rule in the egress SECURITY_RULES map.
    pub async fn set_security_rule(&self, rule_index: u32, rule: SecurityRule) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut rules: HashMap<&mut MapData, u32, SecurityRule> = bpf
            .map_mut("SECURITY_RULES")
            .ok_or_else(|| EbpfError::MapNotFound("SECURITY_RULES".to_string()))?
            .try_into()?;

        rules.insert(rule_index, rule, 0)?;
        Ok(())
    }

    /// Remove a security rule from the egress SECURITY_RULES map.
    pub async fn remove_security_rule(&self, rule_index: u32) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut rules: HashMap<&mut MapData, u32, SecurityRule> = bpf
            .map_mut("SECURITY_RULES")
            .ok_or_else(|| EbpfError::MapNotFound("SECURITY_RULES".to_string()))?
            .try_into()?;

        let _ = rules.remove(&rule_index);
        Ok(())
    }

    /// Set NIC security configuration in the egress NIC_SECURITY map.
    pub async fn set_nic_security_config(
        &self,
        if_index: u32,
        config: NicSecurityConfig,
    ) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut configs: HashMap<&mut MapData, u32, NicSecurityConfig> = bpf
            .map_mut("NIC_SECURITY")
            .ok_or_else(|| EbpfError::MapNotFound("NIC_SECURITY".to_string()))?
            .try_into()?;

        configs.insert(if_index, config, 0)?;
        Ok(())
    }

    /// Remove NIC security configuration from the egress NIC_SECURITY map.
    pub async fn remove_nic_security_config(&self, if_index: u32) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut configs: HashMap<&mut MapData, u32, NicSecurityConfig> = bpf
            .map_mut("NIC_SECURITY")
            .ok_or_else(|| EbpfError::MapNotFound("NIC_SECURITY".to_string()))?
            .try_into()?;

        let _ = configs.remove(&if_index);
        Ok(())
    }

    /// Set a security rule in the ingress SECURITY_RULES map.
    pub async fn set_ingress_security_rule(
        &self,
        rule_index: u32,
        rule: SecurityRule,
    ) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut rules: HashMap<&mut MapData, u32, SecurityRule> = bpf
            .map_mut("SECURITY_RULES")
            .ok_or_else(|| EbpfError::MapNotFound("SECURITY_RULES".to_string()))?
            .try_into()?;

        rules.insert(rule_index, rule, 0)?;
        Ok(())
    }

    /// Set NIC security configuration in the ingress NIC_SECURITY map.
    pub async fn set_ingress_nic_security_config(
        &self,
        if_index: u32,
        config: NicSecurityConfig,
    ) -> Result<()> {
        let mut guard = self.ingress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut configs: HashMap<&mut MapData, u32, NicSecurityConfig> = bpf
            .map_mut("NIC_SECURITY")
            .ok_or_else(|| EbpfError::MapNotFound("NIC_SECURITY".to_string()))?
            .try_into()?;

        configs.insert(if_index, config, 0)?;
        Ok(())
    }

    // ========== Tunnel Endpoint Management ==========

    /// Add IPv4 tunnel endpoint for remote subnet.
    /// Maps inner destination subnet to remote hypervisor prefix.
    pub async fn add_tunnel_endpoint_v4(
        &self,
        subnet: Ipv4Addr,
        prefix_len: u8,
        remote_prefix: [u8; 10],
    ) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut endpoints: LpmTrie<&mut MapData, [u8; 4], TunnelEndpoint> = bpf
            .map_mut("TUNNEL_ENDPOINTS_V4")
            .ok_or_else(|| EbpfError::MapNotFound("TUNNEL_ENDPOINTS_V4".to_string()))?
            .try_into()?;

        let key = Key::new(prefix_len as u32, subnet.octets());
        endpoints.insert(&key, TunnelEndpoint::new(remote_prefix), 0)?;
        Ok(())
    }

    /// Remove IPv4 tunnel endpoint.
    pub async fn remove_tunnel_endpoint_v4(&self, subnet: Ipv4Addr, prefix_len: u8) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut endpoints: LpmTrie<&mut MapData, [u8; 4], TunnelEndpoint> = bpf
            .map_mut("TUNNEL_ENDPOINTS_V4")
            .ok_or_else(|| EbpfError::MapNotFound("TUNNEL_ENDPOINTS_V4".to_string()))?
            .try_into()?;

        let key = Key::new(prefix_len as u32, subnet.octets());
        endpoints.remove(&key)?;
        Ok(())
    }

    /// Add IPv6 tunnel endpoint for remote subnet.
    /// Maps inner destination subnet to remote hypervisor prefix.
    pub async fn add_tunnel_endpoint_v6(
        &self,
        subnet: Ipv6Addr,
        prefix_len: u8,
        remote_prefix: [u8; 10],
    ) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut endpoints: LpmTrie<&mut MapData, [u8; 16], TunnelEndpoint> = bpf
            .map_mut("TUNNEL_ENDPOINTS_V6")
            .ok_or_else(|| EbpfError::MapNotFound("TUNNEL_ENDPOINTS_V6".to_string()))?
            .try_into()?;

        let key = Key::new(prefix_len as u32, subnet.octets());
        endpoints.insert(&key, TunnelEndpoint::new(remote_prefix), 0)?;
        Ok(())
    }

    /// Remove IPv6 tunnel endpoint.
    pub async fn remove_tunnel_endpoint_v6(&self, subnet: Ipv6Addr, prefix_len: u8) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut endpoints: LpmTrie<&mut MapData, [u8; 16], TunnelEndpoint> = bpf
            .map_mut("TUNNEL_ENDPOINTS_V6")
            .ok_or_else(|| EbpfError::MapNotFound("TUNNEL_ENDPOINTS_V6".to_string()))?
            .try_into()?;

        let key = Key::new(prefix_len as u32, subnet.octets());
        endpoints.remove(&key)?;
        Ok(())
    }

    /// Set local NIC metadata for tunnel source address construction.
    /// The NF_ID and SG_ID are embedded in the outer IPv6 source address.
    pub async fn set_local_nic_info(&self, ifindex: u32, info: LocalNicInfo) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut nic_info: HashMap<&mut MapData, u32, LocalNicInfo> = bpf
            .map_mut("LOCAL_NIC_INFO")
            .ok_or_else(|| EbpfError::MapNotFound("LOCAL_NIC_INFO".to_string()))?
            .try_into()?;

        nic_info.insert(ifindex, info, 0)?;
        Ok(())
    }

    /// Remove local NIC metadata.
    pub async fn remove_local_nic_info(&self, ifindex: u32) -> Result<()> {
        let mut guard = self.egress_bpf.write().await;
        let bpf = match guard.as_mut() {
            Some(b) => b,
            None => return Ok(()),
        };

        let mut nic_info: HashMap<&mut MapData, u32, LocalNicInfo> = bpf
            .map_mut("LOCAL_NIC_INFO")
            .ok_or_else(|| EbpfError::MapNotFound("LOCAL_NIC_INFO".to_string()))?
            .try_into()?;

        let _ = nic_info.remove(&ifindex);
        Ok(())
    }
}

impl Default for EbpfManager {
    fn default() -> Self {
        Self::new()
    }
}
