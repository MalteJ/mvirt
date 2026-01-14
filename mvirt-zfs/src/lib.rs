//! mvirt-zfs: ZFS volume manager for mvirt
//!
//! This library provides storage management for VMs using ZFS.

pub mod audit;
pub mod grpc;
pub mod import;
pub mod store;
pub mod zfs;

pub mod proto {
    tonic::include_proto!("mvirt.zfs");
}
