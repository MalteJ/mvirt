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
