//! gRPC API for Network/NIC Management.
//!
//! This module provides a gRPC service for managing virtual networks and NICs.
//! Networks can be public (with internet access via TUN) or private (VM-to-VM only).

pub mod manager;
pub mod service;
pub mod storage;
pub mod validation;

// Re-export generated protobuf types
pub mod proto {
    tonic::include_proto!("mvirt.net");
}

pub use manager::NetworkManager;
pub use service::NetServiceImpl;
pub use storage::Storage;
