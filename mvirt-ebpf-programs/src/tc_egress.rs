//! TC Egress program for VM TAP devices
//!
//! This program is attached to the egress (TX) path of each VM's TAP device.
//! It performs:
//! - DHCP/ARP/NDP detection -> pass to userspace handler
//! - LPM routing lookup for IPv4/IPv6
//! - bpf_redirect() for VM-to-VM traffic
//! - Pass to kernel stack for external traffic

#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::TC_ACT_OK,
    bindings::TC_ACT_SHOT,
    helpers::bpf_redirect,
    macros::{classifier, map},
    maps::{HashMap, LpmTrie},
    programs::TcContext,
};

use aya_ebpf::maps::lpm_trie::Key;

use mvirt_ebpf_programs::{
    ACTION_DROP, ACTION_PASS, ACTION_REDIRECT, DHCP_CLIENT_PORT, DHCP_SERVER_PORT,
    DHCPV6_CLIENT_PORT, DHCPV6_SERVER_PORT, ETH_P_ARP, ETH_P_IP, ETH_P_IPV6,
    ICMPV6_NEIGHBOR_ADVERTISEMENT, ICMPV6_NEIGHBOR_SOLICITATION, ICMPV6_ROUTER_ADVERTISEMENT,
    ICMPV6_ROUTER_SOLICITATION, IfMac, RouteEntry,
};

// Protocol numbers
const IPPROTO_UDP: u8 = 17;
const IPPROTO_ICMPV6: u8 = 58;

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

            // Check for DHCP (UDP port 67/68)
            if proto == IPPROTO_UDP {
                let udp_offset = ETH_HLEN + ihl;
                if data + udp_offset + 4 > data_end {
                    return Ok(TC_ACT_OK);
                }

                let src_port = u16::from_be(unsafe { *((data + udp_offset) as *const u16) });
                let dst_port = u16::from_be(unsafe { *((data + udp_offset + 2) as *const u16) });

                if dst_port == DHCP_SERVER_PORT || src_port == DHCP_CLIENT_PORT {
                    // DHCP: pass to userspace handler
                    return Ok(TC_ACT_OK);
                }
            }

            // Get destination IP (bytes 16-19)
            let dst_addr: [u8; 4] = unsafe { *((ip_ptr + 16) as *const [u8; 4]) };
            let key = Key::new(32, dst_addr);

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

            // Check for NDP (ICMPv6 types 133-136)
            if next_hdr == IPPROTO_ICMPV6 {
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

            // Check for DHCPv6 (UDP port 546/547)
            if next_hdr == IPPROTO_UDP {
                let udp_offset = ETH_HLEN + IPV6_HLEN;
                if data + udp_offset + 4 > data_end {
                    return Ok(TC_ACT_OK);
                }

                let src_port = u16::from_be(unsafe { *((data + udp_offset) as *const u16) });
                let dst_port = u16::from_be(unsafe { *((data + udp_offset + 2) as *const u16) });

                if dst_port == DHCPV6_SERVER_PORT
                    || src_port == DHCPV6_CLIENT_PORT
                    || dst_port == DHCPV6_CLIENT_PORT
                    || src_port == DHCPV6_SERVER_PORT
                {
                    // DHCPv6: pass to userspace handler
                    return Ok(TC_ACT_OK);
                }
            }

            // Get destination IP (bytes 24-39)
            let dst_addr: [u8; 16] = unsafe { *((ipv6_ptr + 24) as *const [u8; 16]) };
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
