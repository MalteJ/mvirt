//! mvirt-log client library
//!
//! Re-exports the generated gRPC client and shared AuditLogger for use by other mvirt components.
//!
//! # Example (AuditLogger - recommended)
//! ```ignore
//! use mvirt_log::{AuditLogger, LogLevel, create_audit_logger};
//!
//! let audit = create_audit_logger("http://[::1]:50052", "vmm");
//! audit.log(LogLevel::Audit, "VM created", vec![vm_id]).await;
//! ```
//!
//! # Example (Direct Client)
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

mod audit;
pub mod command;
pub mod distributed;
pub mod storage;

// Re-export commonly used types at crate root
pub use proto::log_service_client::LogServiceClient;
pub use proto::log_service_server::{LogService, LogServiceServer};
pub use proto::{LogEntry, LogLevel, LogRequest, LogResponse, QueryRequest};

// Re-export AuditLogger
pub use audit::{create_audit_logger, AuditLogger};
