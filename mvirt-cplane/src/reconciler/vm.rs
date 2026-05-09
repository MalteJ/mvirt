//! VM reconciler — converges desired VmData onto the owning node's
//! mvirt-vmm daemon via the reverse tunnel.
//!
//! Stub: structure in place, daemon RPC + status writeback to come.

use anyhow::Result;
use tracing::debug;

use super::Ctx;
use crate::state::ApiState;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.vm_ids()
}

pub async fn reconcile(_ctx: &Ctx, id: &str) -> Result<()> {
    debug!(vm = %id, "vm reconcile (stub)");
    Ok(())
}
