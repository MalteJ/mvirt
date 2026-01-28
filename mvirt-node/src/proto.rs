//! Generated protobuf types for mvirt-node.

#![allow(clippy::enum_variant_names)]

/// Node service (mvirt-api)
pub mod node {
    tonic::include_proto!("mvirt.node");
}

/// VM service (mvirt-vmm)
pub mod vmm {
    tonic::include_proto!("mvirt");
}

/// ZFS service (mvirt-zfs)
pub mod zfs {
    tonic::include_proto!("mvirt.zfs");
}

/// Network service (mvirt-net)
pub mod net {
    tonic::include_proto!("mvirt.net");
}

// Re-export node types at the top level for backwards compatibility
pub use node::*;
