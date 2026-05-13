//! Volume reconciler — converges desired VolumeData onto the owning node's
//! mvirt-zfs daemon via the reverse tunnel.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use mvirt_daemon_protos::zfs::{
    CloneFromTemplateRequest, CreateVolumeRequest, GetVolumeRequest, Volume,
};
use tonic::Code;
use tracing::{info, warn};

use super::Ctx;
use crate::command::{Command, VolumePhase};
use crate::state::ApiState;
use crate::tunnel::NodeHandle;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.volume_ids()
}

pub async fn reconcile(ctx: &Ctx, id: &str) -> Result<()> {
    let state = ctx.store.snapshot().await;
    let Some(vol) = state.get_volume(id) else {
        // Resource gone from state — finalizer pattern is not yet uniform
        // across reconcilers (see ADR-0002, Subsystem 4); cleanup-on-delete
        // will be wired in once Event::*Deleted variants carry the deleted
        // data uniformly.
        return Ok(());
    };

    if vol.status.phase == VolumePhase::Ready {
        return Ok(());
    }

    let spec = &vol.spec;
    let Some(node) = ctx.registry.get(&spec.node_id).await else {
        warn!(volume = %id, node = %spec.node_id, "owning node not connected; will retry on resync");
        return Ok(());
    };

    info!(volume = %id, node = %spec.node_id, name = %spec.name, "reconciling volume");
    let result = if let Some(template_id) = &spec.template_id {
        let template_name = state
            .get_template(template_id)
            .map(|t| t.spec.name)
            .unwrap_or_else(|| template_id.clone());
        clone_from_template(&node, id, &template_name, &spec.name, spec.size_bytes).await
    } else {
        create_volume(&node, id, &spec.name, spec.size_bytes).await
    };

    let cmd = match result {
        Ok(v) => Command::UpdateVolumeStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: VolumePhase::Ready,
            path: Some(v.path),
            used_bytes: v.used_bytes,
            error: None,
        },
        Err(e) => Command::UpdateVolumeStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: VolumePhase::Failed,
            path: None,
            used_bytes: 0,
            error: Some(e),
        },
    };

    ctx.store
        .submit(cmd)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("write volume status: {e}"))
}

async fn create_volume(
    node: &Arc<NodeHandle>,
    id: &str,
    name: &str,
    size_bytes: u64,
) -> std::result::Result<Volume, String> {
    let mut zfs = node.zfs.clone();
    match zfs
        .get_volume(GetVolumeRequest {
            name: name.to_string(),
        })
        .await
    {
        Ok(resp) => return Ok(resp.into_inner()),
        Err(s) if s.code() == Code::NotFound => {}
        Err(s) => return Err(format!("get_volume: {}", s.message())),
    }
    zfs.create_volume(CreateVolumeRequest {
        id: id.to_string(),
        name: name.to_string(),
        size_bytes,
        volblocksize: None,
    })
    .await
    .map(|r| r.into_inner())
    .map_err(|s| format!("create_volume: {}", s.message()))
}

async fn clone_from_template(
    node: &Arc<NodeHandle>,
    id: &str,
    template_name: &str,
    new_volume_name: &str,
    size_bytes: u64,
) -> std::result::Result<Volume, String> {
    let mut zfs = node.zfs.clone();
    match zfs
        .get_volume(GetVolumeRequest {
            name: new_volume_name.to_string(),
        })
        .await
    {
        Ok(resp) => return Ok(resp.into_inner()),
        Err(s) if s.code() == Code::NotFound => {}
        Err(s) => return Err(format!("get_volume: {}", s.message())),
    }
    zfs.clone_from_template(CloneFromTemplateRequest {
        id: id.to_string(),
        template_name: template_name.to_string(),
        new_volume_name: new_volume_name.to_string(),
        size_bytes: Some(size_bytes),
    })
    .await
    .map(|r| r.into_inner())
    .map_err(|s| format!("clone_from_template: {}", s.message()))
}
