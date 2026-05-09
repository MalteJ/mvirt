//! Security group reconciler — converges desired SecurityGroupData onto the
//! mvirt-ebpf daemons of every node that hosts an attached NIC.
//!
//! Stub: structure in place, daemon RPC + status writeback to come.

use anyhow::Result;
use tracing::debug;

use super::Ctx;
use crate::state::ApiState;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.security_groups.keys().cloned().collect()
}

pub async fn reconcile(_ctx: &Ctx, id: &str) -> Result<()> {
    debug!(security_group = %id, "security_group reconcile (stub)");
    Ok(())
}
