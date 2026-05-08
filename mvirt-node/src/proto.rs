//! Protobuf types for mvirt-node.
//!
//! `node.proto` is owned by mvirt-api and compiled here directly because it's
//! the agent↔api wire format. The daemon-side protos (vmm/zfs/net) live in
//! the shared `mvirt-daemon-protos` crate so api and node use identical types.

#![allow(clippy::enum_variant_names)]

/// Node service (mvirt-api).
pub mod node {
    tonic::include_proto!("mvirt.node");
}

/// VM service (mvirt-vmm).
pub use mvirt_daemon_protos::vmm;

/// ZFS service (mvirt-zfs).
pub use mvirt_daemon_protos::zfs;

/// Network service (mvirt-ebpf / mvirt-net).
pub use mvirt_daemon_protos::net;

// Re-export node types at the top level for backwards compatibility
pub use node::*;
