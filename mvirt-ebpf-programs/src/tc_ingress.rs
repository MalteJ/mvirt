//! TC Ingress program for external TUN device
//!
//! This program is attached to the ingress (RX) path of the TUN device
//! that handles external traffic. It routes incoming packets to the
//! appropriate VM TAP device via LPM lookup.

#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::TC_ACT_OK,
    bindings::TC_ACT_SHOT,
    helpers::bpf_redirect,
    macros::{classifier, map},
    maps::{HashMap, LpmTrie, lpm_trie::Key},
    programs::TcContext,
};

use mvirt_ebpf_programs::{
    ACTION_DROP, ACTION_PASS, ACTION_REDIRECT, ETH_P_IP, ETH_P_IPV6, IfMac, RouteEntry,
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

            // Get destination IP (bytes 16-19)
            let dst_addr: [u8; 4] = unsafe { *((ip_ptr + 16) as *const [u8; 4]) };
            let key = Key::new(32, dst_addr);

            if let Some(route) = TUN_ROUTES_V4.get(&key) {
                return handle_route(ctx, route);
            }

            // No route: drop
            Ok(TC_ACT_SHOT)
        }
        ETH_P_IPV6 => {
            // IPv6: need at least ETH + IPv6 header
            if data + ETH_HLEN + IPV6_HLEN > data_end {
                return Ok(TC_ACT_SHOT);
            }

            let ipv6_ptr = data + ETH_HLEN;

            // Get destination IP (bytes 24-39)
            let dst_addr: [u8; 16] = unsafe { *((ipv6_ptr + 24) as *const [u8; 16]) };
            let key = Key::new(128, dst_addr);

            if let Some(route) = TUN_ROUTES_V6.get(&key) {
                return handle_route(ctx, route);
            }

            // No route: drop
            Ok(TC_ACT_SHOT)
        }
        _ => {
            // Non-IP traffic from external: drop
            Ok(TC_ACT_SHOT)
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
