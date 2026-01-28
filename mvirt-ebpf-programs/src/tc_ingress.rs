//! TC Ingress program for external TUN device
//!
//! This program is attached to the ingress (RX) path of the TUN device
//! that handles external traffic. It routes incoming packets to the
//! appropriate VM TAP device via LPM lookup.
//!
//! Security filtering for ingress:
//! - Default DENY (except for established connections)
//! - Check connection tracking for return traffic
//! - Check ingress rules for new connections

#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::TC_ACT_OK,
    bindings::TC_ACT_SHOT,
    helpers::{bpf_ktime_get_ns, bpf_redirect},
    macros::{classifier, map},
    maps::{HashMap, LpmTrie, lpm_trie::Key},
    programs::TcContext,
};

use mvirt_ebpf_programs::{
    ACTION_DROP, ACTION_PASS, ACTION_REDIRECT, CT_FLAG_SEEN_REPLY, CT_STATE_ESTABLISHED,
    CT_STATE_NEW, ConnTrackEntry, ConnTrackKey, DIRECTION_INGRESS, ETH_P_IP, ETH_P_IPV6,
    IPPROTO_TCP, IPPROTO_UDP, IfMac, NicSecurityConfig, PROTO_ALL, RouteEntry, SecurityRule,
};

// Header sizes
const ETH_HLEN: usize = 14;
const IPV6_HLEN: usize = 40;

/// IPv4 LPM routing table for TUN ingress
#[map]
static TUN_ROUTES_V4: LpmTrie<[u8; 4], RouteEntry> = LpmTrie::with_max_entries(4096, 0);

/// IPv6 LPM routing table for TUN ingress
#[map]
static TUN_ROUTES_V6: LpmTrie<[u8; 16], RouteEntry> = LpmTrie::with_max_entries(4096, 0);

/// Interface index to MAC address mapping
#[map]
static TUN_IF_MACS: HashMap<u32, IfMac> = HashMap::with_max_entries(256, 0);

/// Security rules map (index -> rule) - shared with egress
#[map]
static SECURITY_RULES: HashMap<u32, SecurityRule> = HashMap::with_max_entries(4096, 0);

/// NIC security configuration (ifindex -> config) - uses target VM's ifindex
#[map]
static NIC_SECURITY: HashMap<u32, NicSecurityConfig> = HashMap::with_max_entries(256, 0);

/// Connection tracking table (5-tuple -> entry) - shared with egress
#[map]
static CONN_TRACK: HashMap<ConnTrackKey, ConnTrackEntry> = HashMap::with_max_entries(65536, 0);

#[classifier]
pub fn tc_ingress(ctx: TcContext) -> i32 {
    match try_tc_ingress(&ctx) {
        Ok(action) => action,
        Err(_) => TC_ACT_OK, // On error, pass to kernel stack
    }
}

#[inline(always)]
fn try_tc_ingress(ctx: &TcContext) -> Result<i32, ()> {
    let data = ctx.data();
    let data_end = ctx.data_end();

    // Need at least Ethernet header
    if data + ETH_HLEN > data_end {
        return Ok(TC_ACT_OK);
    }

    // Read EtherType (bytes 12-13)
    let eth_type = u16::from_be(unsafe { *((data + 12) as *const u16) });

    match eth_type {
        ETH_P_IP => {
            // IPv4: need at least ETH + minimal IP header
            if data + ETH_HLEN + 20 > data_end {
                return Ok(TC_ACT_SHOT);
            }

            let ip_ptr = data + ETH_HLEN;

            // Get IHL (IP Header Length)
            let version_ihl = unsafe { *(ip_ptr as *const u8) };
            let ihl = ((version_ihl & 0x0f) * 4) as usize;

            // Get protocol
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

            // LPM route lookup first to get target VM
            let key = Key::new(32, dst_ip4);
            let route = match TUN_ROUTES_V4.get(&key) {
                Some(r) => r,
                None => return Ok(TC_ACT_SHOT), // No route: drop
            };

            // Convert addresses to 16-byte format
            let mut src_addr = [0u8; 16];
            let mut dst_addr = [0u8; 16];
            src_addr[..4].copy_from_slice(&src_ip4);
            dst_addr[..4].copy_from_slice(&dst_ip4);

            // Check security for ingress traffic
            if !check_security_ingress(
                route.target_ifindex,
                &src_addr,
                &dst_addr,
                src_port,
                dst_port,
                proto,
                4,
            ) {
                return Ok(TC_ACT_SHOT);
            }

            handle_route(ctx, route)
        }
        ETH_P_IPV6 => {
            // IPv6: need at least ETH + IPv6 header
            if data + ETH_HLEN + IPV6_HLEN > data_end {
                return Ok(TC_ACT_SHOT);
            }

            let ipv6_ptr = data + ETH_HLEN;

            // Get next header (protocol)
            let proto = unsafe { *((ipv6_ptr + 6) as *const u8) };

            // Get source and destination IP
            let src_addr: [u8; 16] = unsafe { *((ipv6_ptr + 8) as *const [u8; 16]) };
            let dst_addr: [u8; 16] = unsafe { *((ipv6_ptr + 24) as *const [u8; 16]) };

            // Extract ports for TCP/UDP
            let (src_port, dst_port) = if proto == IPPROTO_TCP || proto == IPPROTO_UDP {
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

            // LPM route lookup first to get target VM
            let key = Key::new(128, dst_addr);
            let route = match TUN_ROUTES_V6.get(&key) {
                Some(r) => r,
                None => return Ok(TC_ACT_SHOT), // No route: drop
            };

            // Check security for ingress traffic
            if !check_security_ingress(
                route.target_ifindex,
                &src_addr,
                &dst_addr,
                src_port,
                dst_port,
                proto,
                6,
            ) {
                return Ok(TC_ACT_SHOT);
            }

            handle_route(ctx, route)
        }
        _ => {
            // Non-IP traffic from external: drop
            Ok(TC_ACT_SHOT)
        }
    }
}

/// Check security rules for ingress traffic
/// For ingress: default DENY (check CT first, then rules)
#[inline(always)]
fn check_security_ingress(
    target_ifindex: u32,
    src_addr: &[u8; 16],
    dst_addr: &[u8; 16],
    src_port: u16,
    dst_port: u16,
    protocol: u8,
    ip_version: u8,
) -> bool {
    // Check if security is enabled for target NIC
    let config = match unsafe { NIC_SECURITY.get(&target_ifindex) } {
        Some(c) if c.enabled != 0 => c,
        _ => return true, // No security config = allow all
    };

    // Check connection tracking first (for return traffic from outbound connections)
    // Create reverse key to look up the original outbound connection
    let reverse_key = ConnTrackKey::from_tuple(
        *dst_addr, // Our VM's address (was src in outbound)
        *src_addr, // Remote address (was dst in outbound)
        dst_port,  // Our port (was src in outbound)
        src_port,  // Remote port (was dst in outbound)
        protocol, ip_version,
    );

    if let Some(ct_entry) = unsafe { CONN_TRACK.get(&reverse_key) } {
        // Found matching outbound connection - this is return traffic
        // Update the entry to mark it as established with reply seen
        let now_ns = unsafe { bpf_ktime_get_ns() };
        let updated_entry = ConnTrackEntry {
            state: CT_STATE_ESTABLISHED,
            flags: ct_entry.flags | CT_FLAG_SEEN_REPLY,
            _pad: [0; 2],
            last_seen_ns: now_ns,
            packet_count: ct_entry.packet_count + 1,
        };
        let _ = CONN_TRACK.insert(&reverse_key, &updated_entry, 0);
        return true;
    }

    // No CT match - check ingress rules
    let mut i = config.rules_start;
    let end = config.rules_start + config.rules_count;

    // Limit iterations to prevent infinite loops (BPF verifier requirement)
    let max_iter = 256u32;
    let mut iter = 0u32;

    while i < end && iter < max_iter {
        if let Some(rule) = unsafe { SECURITY_RULES.get(&i) } {
            if rule.enabled != 0 && rule.direction == DIRECTION_INGRESS {
                if rule_matches(rule, src_addr, dst_port, protocol, ip_version) {
                    // Rule matched - allow and create CT entry
                    let ct_key = ConnTrackKey::from_tuple(
                        *src_addr, *dst_addr, src_port, dst_port, protocol, ip_version,
                    );
                    let now_ns = unsafe { bpf_ktime_get_ns() };
                    let ct_entry = ConnTrackEntry::with_state(CT_STATE_NEW, now_ns);
                    let _ = CONN_TRACK.insert(&ct_key, &ct_entry, 0);
                    return true;
                }
            }
        }
        i += 1;
        iter += 1;
    }

    // No rule matched - deny (default for ingress)
    false
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

            // Rewrite L2 header with VM's MAC
            unsafe {
                // Destination MAC (bytes 0-5)
                let dst_mac_ptr = data as *mut [u8; 6];
                *dst_mac_ptr = route.dst_mac;

                // Source MAC (bytes 6-11)
                let src_mac_ptr = (data + 6) as *mut [u8; 6];
                *src_mac_ptr = route.src_mac;
            }

            // Redirect to VM's TAP interface
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
