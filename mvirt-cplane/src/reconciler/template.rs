//! Template reconciler — converges desired TemplateData onto the owning
//! node's mvirt-zfs daemon via the reverse tunnel (template import + state
//! tracking through Pending → Importing → Ready/Failed).
//!
//! Stub: structure in place, daemon RPC + status writeback to come.

use anyhow::Result;
use tracing::debug;

use super::Ctx;
use crate::state::ApiState;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.template_ids()
}

pub async fn reconcile(_ctx: &Ctx, id: &str) -> Result<()> {
    debug!(template = %id, "template reconcile (stub)");
    Ok(())
}
