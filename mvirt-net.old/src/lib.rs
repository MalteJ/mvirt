//! mvirt-net: Virtual network daemon for mvirt VMs
//!
//! Provides L3 networking for VMs using vhost-user virtio-net backends.

pub mod audit;
pub mod config;
pub mod dataplane;
pub mod grpc;
pub mod store;

pub mod proto {
    tonic::include_proto!("mvirt.net");
}
