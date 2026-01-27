//! mvirt-vmm - Virtual Machine Manager library.
//!
//! This module exposes the VMM components for integration testing.

pub mod grpc;
pub mod hypervisor;
pub mod pod_service;
pub mod ready_listener;
pub mod store;
pub mod system_info;
pub mod vsock_client;

pub mod proto {
    tonic::include_proto!("mvirt");
}
