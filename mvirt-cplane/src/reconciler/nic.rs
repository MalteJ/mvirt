//! NIC reconciler — converges desired NicData onto the owning node's
//! mvirt-ebpf daemon via the reverse tunnel.
//!
//! Stub: structure in place, daemon RPC + status writeback to come.

use anyhow::Result;
use tracing::debug;

use super::Ctx;
use crate::state::ApiState;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.nic_ids()
}

pub async fn reconcile(_ctx: &Ctx, id: &str) -> Result<()> {
    debug!(nic = %id, "nic reconcile (stub)");
    Ok(())
}
