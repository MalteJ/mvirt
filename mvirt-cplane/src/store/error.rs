//! Store error types.

use thiserror::Error;

/// Errors that can occur during store operations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Resource not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Conflict with existing resource.
    #[error("conflict: {0}")]
    Conflict(String),

    /// Version mismatch (optimistic concurrency control).
    #[error("version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u64, actual: u64 },

    /// Not the leader, request should be forwarded.
    #[error("not leader, try node {leader_id:?}")]
    NotLeader { leader_id: Option<u64> },

    /// Scheduling failed - no suitable node found.
    #[error("scheduling failed: {0}")]
    ScheduleFailed(String),

    /// Internal error.
    #[error("internal: {0}")]
    Internal(String),
}

/// Result type for store operations.
pub type Result<T> = std::result::Result<T, StoreError>;
