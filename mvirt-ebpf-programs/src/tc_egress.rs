//! TC Egress program for VM TAP devices
//!
//! This program is attached to the egress (TX) path of each VM's TAP device.
//! It performs:
//! - DHCP/ARP/NDP detection -> pass to userspace handler
//! - Security group rule checking (egress rules)
//! - Connection tracking for stateful filtering
//! - LPM routing lookup for IPv4/IPv6
//! - bpf_redirect() for VM-to-VM traffic
//! - Pass to kernel stack for external traffic

#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::TC_ACT_OK,
    bindings::TC_ACT_SHOT,
    helpers::{bpf_ktime_get_ns, bpf_redirect},
    macros::{classifier, map},
    maps::{HashMap, LpmTrie},
    programs::TcContext,
};

use aya_ebpf::maps::lpm_trie::Key;

use mvirt_ebpf_programs::{
    ACTION_DROP, ACTION_PASS, ACTION_REDIRECT, CT_STATE_NEW, ConnTrackEntry, ConnTrackKey,
    DHCP_CLIENT_PORT, DHCP_SERVER_PORT, DHCPV6_CLIENT_PORT, DHCPV6_SERVER_PORT, DIRECTION_EGRESS,
    ETH_P_ARP, ETH_P_IP, ETH_P_IPV6, ICMPV6_NEIGHBOR_ADVERTISEMENT, ICMPV6_NEIGHBOR_SOLICITATION,
    ICMPV6_ROUTER_ADVERTISEMENT, ICMPV6_ROUTER_SOLICITATION, IPPROTO_TCP, IPPROTO_UDP, IfMac,
    NicSecurityConfig, PROTO_ALL, RouteEntry, SecurityRule,
};

// Local protocol constants
const PROTO_ICMPV6_LOCAL: u8 = 58;

// Header sizes
const ETH_HLEN: usize = 14;
const IPV6_HLEN: usize = 40;

/// IPv4 LPM routing table
/// Key type is [u8; 4] for the IPv4 address
#[map]
static ROUTES_V4: LpmTrie<[u8; 4], RouteEntry> = LpmTrie::with_max_entries(4096, 0);

/// IPv6 LPM routing table
/// Key type is [u8; 16] for the IPv6 address
#[map]
static ROUTES_V6: LpmTrie<[u8; 16], RouteEntry> = LpmTrie::with_max_entries(4096, 0);

/// Interface index to MAC address mapping
#[map]
static IF_MACS: HashMap<u32, IfMac> = HashMap::with_max_entries(256, 0);

/// Security rules map (index -> rule)
#[map]
static SECURITY_RULES: HashMap<u32, SecurityRule> = HashMap::with_max_entries(4096, 0);

/// NIC security configuration (ifindex -> config)
#[map]
static NIC_SECURITY: HashMap<u32, NicSecurityConfig> = HashMap::with_max_entries(256, 0);

/// Connection tracking table (5-tuple -> entry)
#[map]
static CONN_TRACK: HashMap<ConnTrackKey, ConnTrackEntry> = HashMap::with_max_entries(65536, 0);

#[classifier]
pub fn tc_egress(ctx: TcContext) -> i32 {
    match try_tc_egress(&ctx) {
        Ok(action) => action,
        Err(_) => TC_ACT_OK, // On error, pass to kernel stack
    }
}

#[inline(always)]
fn try_tc_egress(ctx: &TcContext) -> Result<i32, ()> {
    let data = ctx.data();
    let data_end = ctx.data_end();

    // Need at least Ethernet header
    if data + ETH_HLEN > data_end {
        return Ok(TC_ACT_OK);
    }

    // Read EtherType (bytes 12-13)
    let eth_type = u16::from_be(unsafe { *((data + 12) as *const u16) });

    match eth_type {
        ETH_P_ARP => {
            // ARP: pass to userspace handler via kernel stack
            Ok(TC_ACT_OK)
        }
        ETH_P_IP => {
            // IPv4: need at least ETH + minimal IP header (20 bytes)
            if data + ETH_HLEN + 20 > data_end {
                return Ok(TC_ACT_OK);
            }

            let ip_ptr = data + ETH_HLEN;

            // Get IHL (IP Header Length) from first byte
            let version_ihl = unsafe { *(ip_ptr as *const u8) };
            let ihl = ((version_ihl & 0x0f) * 4) as usize;

            // Get protocol (byte 9)
            let proto = unsafe { *((ip_ptr + 9) as *const u8) };

            // Get source and destination IP
            let src_ip4: [u8; 4] = unsafe { *((ip_ptr + 12) as *const [u8; 4]) };
            let dst_ip4: [u8; 4] = unsafe { *((ip_ptr + 16) as *const [u8; 4]) };

            // Extract ports for TCP/UDP
            let (src_port, dst_port) = if proto == IPPROTO_TCP || proto == IPPROTO_UDP {
                let transport_offset = ETH_HLEN + ihl;
                if data + transport_offset + 4 > data_end {
                    (0u16, 0u16)
                } else {
                    let sp = u16::from_be(unsafe { *((data + transport_offset) as *const u16) });
                    let dp =
                        u16::from_be(unsafe { *((data + transport_offset + 2) as *const u16) });
                    (sp, dp)
                }
            } else {
                (0u16, 0u16)
            };

            // Check for DHCP (UDP port 67/68)
            if proto == IPPROTO_UDP {
                if dst_port == DHCP_SERVER_PORT || src_port == DHCP_CLIENT_PORT {
                    // DHCP: pass to userspace handler
                    return Ok(TC_ACT_OK);
                }
            }

            // Convert IPv4 addresses to 16-byte format for unified handling
            let mut src_addr = [0u8; 16];
            let mut dst_addr = [0u8; 16];
            src_addr[..4].copy_from_slice(&src_ip4);
            dst_addr[..4].copy_from_slice(&dst_ip4);

            // Check security rules and create CT entry
            // Access ifindex from __sk_buff structure
            let ifindex = unsafe { (*ctx.skb.skb).ifindex };
            if !check_security_egress(
                ctx, ifindex, &src_addr, &dst_addr, src_port, dst_port, proto, 4,
            ) {
                return Ok(TC_ACT_SHOT);
            }

            // LPM route lookup
            let key = Key::new(32, dst_ip4);

            if let Some(route) = ROUTES_V4.get(&key) {
                return handle_route(ctx, route);
            }

            // No route found: pass to kernel stack
            Ok(TC_ACT_OK)
        }
        ETH_P_IPV6 => {
            // IPv6: need at least ETH + IPv6 header
            if data + ETH_HLEN + IPV6_HLEN > data_end {
                return Ok(TC_ACT_OK);
            }

            let ipv6_ptr = data + ETH_HLEN;

            // Get next header (byte 6)
            let next_hdr = unsafe { *((ipv6_ptr + 6) as *const u8) };

            // Get source and destination IP (bytes 8-23 and 24-39)
            let src_addr: [u8; 16] = unsafe { *((ipv6_ptr + 8) as *const [u8; 16]) };
            let dst_addr: [u8; 16] = unsafe { *((ipv6_ptr + 24) as *const [u8; 16]) };

            // Check for NDP (ICMPv6 types 133-136)
            if next_hdr == PROTO_ICMPV6_LOCAL {
                let icmp_offset = ETH_HLEN + IPV6_HLEN;
                if data + icmp_offset + 1 > data_end {
                    return Ok(TC_ACT_OK);
                }

                let icmp_type = unsafe { *((data + icmp_offset) as *const u8) };

                if matches!(
                    icmp_type,
                    ICMPV6_ROUTER_SOLICITATION
                        | ICMPV6_ROUTER_ADVERTISEMENT
                        | ICMPV6_NEIGHBOR_SOLICITATION
                        | ICMPV6_NEIGHBOR_ADVERTISEMENT
                ) {
                    // NDP: pass to userspace handler
                    return Ok(TC_ACT_OK);
                }
            }

            // Extract ports for TCP/UDP
            let (src_port, dst_port) = if next_hdr == IPPROTO_TCP || next_hdr == IPPROTO_UDP {
                let transport_offset = ETH_HLEN + IPV6_HLEN;
                if data + transport_offset + 4 > data_end {
                    (0u16, 0u16)
                } else {
                    let sp = u16::from_be(unsafe { *((data + transport_offset) as *const u16) });
                    let dp =
                        u16::from_be(unsafe { *((data + transport_offset + 2) as *const u16) });
                    (sp, dp)
                }
            } else {
                (0u16, 0u16)
            };

            // Check for DHCPv6 (UDP port 546/547)
            if next_hdr == IPPROTO_UDP {
                if dst_port == DHCPV6_SERVER_PORT
                    || src_port == DHCPV6_CLIENT_PORT
                    || dst_port == DHCPV6_CLIENT_PORT
                    || src_port == DHCPV6_SERVER_PORT
                {
                    // DHCPv6: pass to userspace handler
                    return Ok(TC_ACT_OK);
                }
            }

            // Check security rules and create CT entry
            let ifindex = unsafe { (*ctx.skb.skb).ifindex };
            if !check_security_egress(
                ctx, ifindex, &src_addr, &dst_addr, src_port, dst_port, next_hdr, 6,
            ) {
                return Ok(TC_ACT_SHOT);
            }

            // LPM route lookup
            let key = Key::new(128, dst_addr);

            if let Some(route) = ROUTES_V6.get(&key) {
                return handle_route(ctx, route);
            }

            // No route found: pass to kernel stack
            Ok(TC_ACT_OK)
        }
        _ => {
            // Unknown EtherType: pass to kernel stack
            Ok(TC_ACT_OK)
        }
    }
}

/// Check security rules for egress traffic
/// For egress: default ALLOW, but create CT entry for return traffic
#[inline(always)]
fn check_security_egress(
    _ctx: &TcContext,
    ifindex: u32,
    src_addr: &[u8; 16],
    dst_addr: &[u8; 16],
    src_port: u16,
    dst_port: u16,
    protocol: u8,
    ip_version: u8,
) -> bool {
    // Check if security is enabled for this NIC
    let config = match unsafe { NIC_SECURITY.get(&ifindex) } {
        Some(c) if c.enabled != 0 => c,
        _ => return true, // No security config = allow all
    };

    // Check egress rules - if ANY rule matches, allow
    // Default for egress is ALLOW, so we check rules to potentially track connections
    let mut rule_matched = false;
    let mut i = config.rules_start;
    let end = config.rules_start + config.rules_count;

    // Limit iterations to prevent infinite loops (BPF verifier requirement)
    let max_iter = 256u32;
    let mut iter = 0u32;

    while i < end && iter < max_iter {
        if let Some(rule) = unsafe { SECURITY_RULES.get(&i) } {
            if rule.enabled != 0 && rule.direction == DIRECTION_EGRESS {
                if rule_matches(rule, dst_addr, dst_port, protocol, ip_version) {
                    rule_matched = true;
                    break;
                }
            }
        }
        i += 1;
        iter += 1;
    }

    // For egress, default is ALLOW
    // Create connection tracking entry for return traffic
    let ct_key = ConnTrackKey::from_tuple(
        *src_addr, *dst_addr, src_port, dst_port, protocol, ip_version,
    );

    let now_ns = unsafe { bpf_ktime_get_ns() };
    let ct_entry = ConnTrackEntry::with_state(CT_STATE_NEW, now_ns);

    // Insert or update CT entry (ignore errors)
    let _ = CONN_TRACK.insert(&ct_key, &ct_entry, 0);

    // Egress default: allow (rule_matched just means we found a matching allow rule)
    let _ = rule_matched; // Suppress unused warning
    true
}

/// Check if a packet matches a security rule
#[inline(always)]
fn rule_matches(
    rule: &SecurityRule,
    addr: &[u8; 16],
    port: u16,
    protocol: u8,
    ip_version: u8,
) -> bool {
    // Check IP version
    if rule.ip_version != 0 && rule.ip_version != ip_version {
        return false;
    }

    // Check protocol
    if rule.protocol != PROTO_ALL && rule.protocol != protocol {
        return false;
    }

    // Check port range (only for TCP/UDP)
    if (protocol == IPPROTO_TCP || protocol == IPPROTO_UDP)
        && rule.port_start != 0
        && (port < rule.port_start || port > rule.port_end)
    {
        return false;
    }

    // Check CIDR
    if rule.cidr_prefix_len > 0 {
        if !cidr_matches(&rule.cidr_addr, rule.cidr_prefix_len, addr, ip_version) {
            return false;
        }
    }

    true
}

/// Check if an address matches a CIDR
#[inline(always)]
fn cidr_matches(cidr_addr: &[u8; 16], prefix_len: u8, addr: &[u8; 16], ip_version: u8) -> bool {
    let bytes = if ip_version == 4 { 4usize } else { 16usize };
    let full_bytes = (prefix_len / 8) as usize;
    let remaining_bits = prefix_len % 8;

    // Check full bytes
    let mut i = 0usize;
    while i < full_bytes && i < bytes {
        if cidr_addr[i] != addr[i] {
            return false;
        }
        i += 1;
    }

    // Check remaining bits
    if remaining_bits > 0 && full_bytes < bytes {
        let mask = 0xffu8 << (8 - remaining_bits);
        if (cidr_addr[full_bytes] & mask) != (addr[full_bytes] & mask) {
            return false;
        }
    }

    true
}

#[inline(always)]
fn handle_route(ctx: &TcContext, route: &RouteEntry) -> Result<i32, ()> {
    match route.action {
        ACTION_DROP => Ok(TC_ACT_SHOT),
        ACTION_PASS => Ok(TC_ACT_OK),
        ACTION_REDIRECT => {
            let data = ctx.data();
            let data_end = ctx.data_end();

            // Verify we have enough space for Ethernet header
            if data + 12 > data_end {
                return Ok(TC_ACT_OK);
            }

            // Rewrite L2 header with new MACs
            unsafe {
                // Destination MAC (bytes 0-5)
                let dst_mac_ptr = data as *mut [u8; 6];
                *dst_mac_ptr = route.dst_mac;

                // Source MAC (bytes 6-11)
                let src_mac_ptr = (data + 6) as *mut [u8; 6];
                *src_mac_ptr = route.src_mac;
            }

            // Redirect to target interface
            let ret = unsafe { bpf_redirect(route.target_ifindex, 0) };
            Ok(ret as i32)
        }
        _ => Ok(TC_ACT_OK),
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
