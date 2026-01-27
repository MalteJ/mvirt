//! gRPC server module for mvirt-ebpf.

pub mod service;
pub mod storage;
pub mod validation;

/// Generated proto types.
pub mod proto {
    tonic::include_proto!("mvirt.net");
}

pub use service::EbpfNetServiceImpl;
pub use storage::{NetworkData, NicData, NicState, Storage};
