//! Reconcilers for different resource types.
//!
//! Each reconciler compares desired state (spec from API) with actual state
//! (from local daemon) and takes actions to converge.

pub mod network;
pub mod nic;
pub mod vm;

use anyhow::Result;
use async_trait::async_trait;

/// Trait for resource reconcilers.
#[async_trait]
pub trait Reconciler: Send + Sync {
    /// The spec type from the API.
    type Spec;
    /// The status type to report back.
    type Status;

    /// Reconcile the resource - compare desired vs actual state and take action.
    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status>;

    /// Handle resource deletion (finalization).
    async fn finalize(&self, id: &str) -> Result<()>;
}
