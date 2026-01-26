//! gRPC service for mvirt-node agents.

pub mod server;

// Include the generated protobuf code
pub mod proto {
    tonic::include_proto!("mvirt.node");
}

pub use server::NodeServiceImpl;
