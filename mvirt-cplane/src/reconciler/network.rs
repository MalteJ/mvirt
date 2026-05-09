//! Network reconciler — converges desired NetworkData onto the relevant
//! mvirt-ebpf daemons (one per node that hosts a NIC on this network).
//!
//! Stub: structure in place, daemon RPC + status writeback to come.

use anyhow::Result;
use tracing::debug;

use super::Ctx;
use crate::state::ApiState;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.networks.keys().cloned().collect()
}

pub async fn reconcile(_ctx: &Ctx, id: &str) -> Result<()> {
    debug!(network = %id, "network reconcile (stub)");
    Ok(())
}
