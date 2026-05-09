//! Shared gRPC bindings for the per-host daemons that mvirt-cplane and mvirt-node
//! both need to talk to: mvirt-vmm, mvirt-zfs, and mvirt-ebpf (which uses
//! mvirt-net's proto).
//!
//! Compiling these in one crate avoids duplicate codegen and keeps both ends
//! of the reverse tunnel using identical message types.

/// Bindings for `mvirt-vmm` (VmService + PodService).
///
/// Proto package: `mvirt`.
pub mod vmm {
    tonic::include_proto!("mvirt");
}

/// Bindings for `mvirt-zfs` (ZfsService).
///
/// Proto package: `mvirt.zfs`.
pub mod zfs {
    tonic::include_proto!("mvirt.zfs");
}

/// Bindings for `mvirt-ebpf` / `mvirt-net` (NetService).
///
/// Proto package: `mvirt.net`.
pub mod net {
    tonic::include_proto!("mvirt.net");
}
