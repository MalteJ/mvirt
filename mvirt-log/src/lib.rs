//! mvirt-log client library
//!
//! Re-exports the generated gRPC client for use by other mvirt components.
//!
//! # Example
//! ```ignore
//! use mvirt_log::{LogServiceClient, LogEntry, LogLevel, LogRequest};
//!
//! let mut client = LogServiceClient::connect("http://[::1]:50052").await?;
//! client.log(LogRequest {
//!     entry: Some(LogEntry {
//!         message: "VM started".into(),
//!         level: LogLevel::Info as i32,
//!         component: "vmm".into(),
//!         related_object_ids: vec![vm_id],
//!         ..Default::default()
//!     }),
//! }).await?;
//! ```

pub mod proto {
    tonic::include_proto!("mvirt.log");
}

// Re-export commonly used types at crate root
pub use proto::log_service_client::LogServiceClient;
pub use proto::log_service_server::{LogService, LogServiceServer};
pub use proto::{LogEntry, LogLevel, LogRequest, LogResponse, QueryRequest};
