//! mvirt-ebpf: eBPF-based networking for mvirt VMs.
//!
//! This crate provides network connectivity for VMs using TAP devices
//! and TC eBPF programs for packet routing, instead of vhost-user.
//!
//! # Architecture
//!
//! ```text
//! VM (virtio-net) --> TAP device --> TC eBPF (Kernel)
//!                                        |
//!                                        +--> bpf_redirect (VM-to-VM)
//!                                        +--> Pass to Stack --> extern (NAT via nftables)
//!                                        +--> DHCP/ARP/NDP --> Userspace Raw Socket
//! ```

pub mod audit;
pub mod conntrack;
pub mod ebpf_loader;
pub mod grpc;
pub mod nat;
pub mod proto_handler;
pub mod tap;

#[cfg(any(test, feature = "test-util"))]
pub mod test_util;

// Re-export commonly used types
pub use audit::{EbpfAuditLogger, create_audit_logger};
pub use conntrack::ConnTrackCleaner;
pub use ebpf_loader::EbpfManager;
pub use grpc::{EbpfNetServiceImpl, NetworkData, NicData, NicState, Storage};
pub use proto_handler::{
    GATEWAY_IPV4_LINK_LOCAL, GATEWAY_IPV6_LINK_LOCAL, GATEWAY_MAC, ProtocolHandler,
    process_packet_sync,
};
pub use tap::TapDevice;
