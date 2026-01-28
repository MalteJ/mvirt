#![no_std]

/// Route action for LPM lookup result
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RouteEntry {
    /// Action: 0=drop, 1=redirect to ifindex, 2=pass to kernel stack
    pub action: u8,
    _padding: [u8; 3],
    /// Target interface index for redirect action
    pub target_ifindex: u32,
    /// Destination MAC address (for L2 rewrite)
    pub dst_mac: [u8; 6],
    /// Source MAC address (gateway MAC for L2 rewrite)
    pub src_mac: [u8; 6],
}

impl RouteEntry {
    pub const fn new(action: u8, target_ifindex: u32, dst_mac: [u8; 6], src_mac: [u8; 6]) -> Self {
        Self {
            action,
            _padding: [0; 3],
            target_ifindex,
            dst_mac,
            src_mac,
        }
    }
}

/// LPM key for IPv4 routing - must match aya's Key format
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ipv4LpmKey {
    /// Prefix length (0-32)
    pub prefix_len: u32,
    /// IPv4 address in network byte order
    pub addr: [u8; 4],
}

/// LPM key for IPv6 routing - must match aya's Key format
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ipv6LpmKey {
    /// Prefix length (0-128)
    pub prefix_len: u32,
    /// IPv6 address in network byte order
    pub addr: [u8; 16],
}

/// Interface MAC address entry
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IfMac {
    pub mac: [u8; 6],
}

// Route actions
pub const ACTION_DROP: u8 = 0;
pub const ACTION_REDIRECT: u8 = 1;
pub const ACTION_PASS: u8 = 2;

// EtherTypes
pub const ETH_P_IP: u16 = 0x0800;
pub const ETH_P_IPV6: u16 = 0x86DD;
pub const ETH_P_ARP: u16 = 0x0806;

// IP protocols
pub const IPPROTO_UDP: u8 = 17;
pub const IPPROTO_ICMPV6: u8 = 58;

// DHCP ports
pub const DHCP_SERVER_PORT: u16 = 67;
pub const DHCP_CLIENT_PORT: u16 = 68;
pub const DHCPV6_SERVER_PORT: u16 = 547;
pub const DHCPV6_CLIENT_PORT: u16 = 546;

// ICMPv6 types for NDP
pub const ICMPV6_ROUTER_SOLICITATION: u8 = 133;
pub const ICMPV6_ROUTER_ADVERTISEMENT: u8 = 134;
pub const ICMPV6_NEIGHBOR_SOLICITATION: u8 = 135;
pub const ICMPV6_NEIGHBOR_ADVERTISEMENT: u8 = 136;

// IP protocols for security rules
pub const IPPROTO_ICMP: u8 = 1;
pub const IPPROTO_TCP: u8 = 6;

// Security rule directions
pub const DIRECTION_INGRESS: u8 = 0;
pub const DIRECTION_EGRESS: u8 = 1;

// Security rule protocols (0 = all)
pub const PROTO_ALL: u8 = 0;
// IPPROTO_ICMP = 1 (already defined above)
// IPPROTO_TCP = 6 (already defined above)
// IPPROTO_UDP = 17 (already defined above)
// IPPROTO_ICMPV6 = 58 (already defined above)

// Connection tracking states
pub const CT_STATE_NEW: u8 = 0;
pub const CT_STATE_ESTABLISHED: u8 = 1;
pub const CT_STATE_RELATED: u8 = 2;

// Connection tracking flags
pub const CT_FLAG_SEEN_REPLY: u8 = 1;
pub const CT_FLAG_ASSURED: u8 = 2;

/// Security rule for packet filtering
/// Rules are always "allow" - if ANY rule matches, packet is allowed
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SecurityRule {
    /// Rule is active (1) or disabled (0)
    pub enabled: u8,
    /// Direction: 0=ingress (to VM), 1=egress (from VM)
    pub direction: u8,
    /// IP protocol: 0=all, 1=ICMP, 6=TCP, 17=UDP, 58=ICMPv6
    pub protocol: u8,
    /// IP version: 4=IPv4, 6=IPv6, 0=both
    pub ip_version: u8,
    /// Start of port range (inclusive), 0 = any
    pub port_start: u16,
    /// End of port range (inclusive), 0 = any
    pub port_end: u16,
    /// CIDR network address (IPv4 in first 4 bytes, IPv6 uses all 16)
    pub cidr_addr: [u8; 16],
    /// CIDR prefix length (0-32 for IPv4, 0-128 for IPv6)
    pub cidr_prefix_len: u8,
    _padding: [u8; 3],
}

impl SecurityRule {
    pub const fn new() -> Self {
        Self {
            enabled: 0,
            direction: 0,
            protocol: 0,
            ip_version: 0,
            port_start: 0,
            port_end: 0,
            cidr_addr: [0; 16],
            cidr_prefix_len: 0,
            _padding: [0; 3],
        }
    }
}

/// Connection tracking key (5-tuple + ip version)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConnTrackKey {
    /// Source IP address (IPv4: first 4 bytes, IPv6: all 16)
    pub src_addr: [u8; 16],
    /// Destination IP address (IPv4: first 4 bytes, IPv6: all 16)
    pub dst_addr: [u8; 16],
    /// Source port (0 for ICMP)
    pub src_port: u16,
    /// Destination port (0 for ICMP, but stores ICMP id)
    pub dst_port: u16,
    /// IP protocol: 1=ICMP, 6=TCP, 17=UDP, 58=ICMPv6
    pub protocol: u8,
    /// IP version: 4 or 6
    pub ip_version: u8,
    /// Padding for alignment
    pub _pad: [u8; 2],
}

impl ConnTrackKey {
    pub const fn new() -> Self {
        Self {
            src_addr: [0; 16],
            dst_addr: [0; 16],
            src_port: 0,
            dst_port: 0,
            protocol: 0,
            ip_version: 0,
            _pad: [0; 2],
        }
    }

    /// Create reverse key for reply traffic lookup
    pub const fn reverse(&self) -> Self {
        Self {
            src_addr: self.dst_addr,
            dst_addr: self.src_addr,
            src_port: self.dst_port,
            dst_port: self.src_port,
            protocol: self.protocol,
            ip_version: self.ip_version,
            _pad: [0; 2],
        }
    }

    /// Create a new key with all fields specified
    pub const fn from_tuple(
        src_addr: [u8; 16],
        dst_addr: [u8; 16],
        src_port: u16,
        dst_port: u16,
        protocol: u8,
        ip_version: u8,
    ) -> Self {
        Self {
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            protocol,
            ip_version,
            _pad: [0; 2],
        }
    }
}

/// Connection tracking entry
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConnTrackEntry {
    /// Connection state (CT_STATE_*)
    pub state: u8,
    /// Flags (CT_FLAG_*)
    pub flags: u8,
    /// Padding for alignment
    pub _pad: [u8; 2],
    /// Timestamp of last seen packet (nanoseconds since boot)
    pub last_seen_ns: u64,
    /// Total packet count for this connection
    pub packet_count: u64,
}

impl ConnTrackEntry {
    pub const fn new() -> Self {
        Self {
            state: CT_STATE_NEW,
            flags: 0,
            _pad: [0; 2],
            last_seen_ns: 0,
            packet_count: 0,
        }
    }

    /// Create a new entry with specified state and timestamp
    pub const fn with_state(state: u8, last_seen_ns: u64) -> Self {
        Self {
            state,
            flags: 0,
            _pad: [0; 2],
            last_seen_ns,
            packet_count: 1,
        }
    }
}

/// NIC security configuration
/// Maps NIC ifindex to its security settings
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NicSecurityConfig {
    /// Whether security filtering is enabled for this NIC
    pub enabled: u8,
    _padding: [u8; 3],
    /// Start index into SECURITY_RULES map for this NIC's rules
    pub rules_start: u32,
    /// Number of rules for this NIC
    pub rules_count: u32,
}

impl NicSecurityConfig {
    pub const fn new() -> Self {
        Self {
            enabled: 0,
            _padding: [0; 3],
            rules_start: 0,
            rules_count: 0,
        }
    }
}
