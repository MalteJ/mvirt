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

    /// Caller is not allowed to perform this operation (401).
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Request validation failed (400).
    #[error("validation: {0}")]
    Validation(String),

    /// Resource existed at one point but has since been removed (410).
    /// Distinguished from `NotFound` so the caller can react differently
    /// — e.g. an onboarding token whose Cluster got deleted post-issue.
    #[error("gone: {0}")]
    Gone(String),

    /// Internal error.
    #[error("internal: {0}")]
    Internal(String),
}

/// Result type for store operations.
pub type Result<T> = std::result::Result<T, StoreError>;
