//! Shared context passed to every per-resource reconciler. Holds the cluster
//! state store, the per-node tunnel registry, and the audit logger.

use std::sync::Arc;

use crate::audit::ApiAuditLogger;
use crate::store::RaftStore;
use crate::tunnel::NodeRegistry;

#[derive(Clone)]
pub struct Ctx {
    pub store: Arc<RaftStore>,
    pub registry: Arc<NodeRegistry>,
    pub audit: Arc<ApiAuditLogger>,
}
