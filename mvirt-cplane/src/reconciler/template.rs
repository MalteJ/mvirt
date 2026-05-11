//! Template reconciler — converges desired TemplateData onto the owning
//! node's mvirt-zfs daemon via the reverse tunnel.
//!
//! Driven by:
//! - `Event::TemplateCreated` → kicks off the first reconcile.
//! - `Event::TemplateUpdated` → reacts to user edits / our own status writes.
//! - 30s resync → re-polls active import jobs so the UI doesn't go stale.
//!
//! State mapping (cplane phase ← zfs ImportJobState):
//!   Pending → ImportTemplate has not been kicked off yet
//!   Importing ← PENDING / DOWNLOADING / CONVERTING / WRITING
//!   Ready ← template exists on the node (ListTemplates match)
//!   Failed ← FAILED / CANCELLED, or non-recoverable error
//!
//! Idempotency: every reconcile lists templates and active jobs first. If
//! the template already exists, we short-circuit to Ready without firing
//! another ImportTemplate.

use anyhow::Result;
use chrono::Utc;
use mvirt_daemon_protos::zfs::{
    ImportJobState, ImportTemplateRequest, ListImportJobsRequest, ListTemplatesRequest,
};
use tracing::{info, warn};

use super::Ctx;
use crate::command::{Command, TemplatePhase};
use crate::state::ApiState;
use crate::tunnel::NodeHandle;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.template_ids()
}

pub async fn reconcile(ctx: &Ctx, id: &str) -> Result<()> {
    let state = ctx.store.snapshot().await;
    let Some(tmpl) = state.get_template(id) else {
        return Ok(());
    };

    if tmpl.status.phase == TemplatePhase::Ready {
        return Ok(());
    }

    let spec = &tmpl.spec;
    let Some(node) = ctx.registry.get(&spec.node_id).await else {
        warn!(template = %id, node = %spec.node_id, "owning node not connected; will retry on resync");
        return Ok(());
    };

    info!(template = %id, node = %spec.node_id, name = %spec.name, phase = ?tmpl.status.phase, "reconciling template");

    let outcome = drive(&node, &tmpl).await;

    let cmd = match outcome {
        Ok(progress) => Command::UpdateTemplateStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: progress.phase,
            bytes_written: progress.bytes_written,
            size_bytes: progress.size_bytes,
            error: None,
        },
        Err(e) => Command::UpdateTemplateStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: TemplatePhase::Failed,
            bytes_written: tmpl.status.bytes_written,
            size_bytes: tmpl.status.size_bytes,
            error: Some(e),
        },
    };

    ctx.store
        .submit(cmd)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("write template status: {e}"))
}

struct Progress {
    phase: TemplatePhase,
    bytes_written: u64,
    size_bytes: u64,
}

async fn drive(
    node: &NodeHandle,
    tmpl: &crate::command::TemplateData,
) -> std::result::Result<Progress, String> {
    // (1) Template already exists on the node → Ready.
    if let Some(t) = find_template(node, &tmpl.spec.name).await? {
        return Ok(Progress {
            phase: TemplatePhase::Ready,
            bytes_written: t.size_bytes,
            size_bytes: t.size_bytes,
        });
    }

    // (2) An import job for this template is in flight → mirror its state.
    if let Some(job) = find_active_job(node, &tmpl.spec.name).await? {
        return Ok(job_to_progress(&job));
    }

    // (3) Nothing yet — kick off the import. We don't need the response
    // here because the next reconcile (event-driven or via resync) will
    // pick up the job from ListImportJobs.
    let Some(source) = tmpl.spec.source_url.as_deref().filter(|s| !s.is_empty()) else {
        return Err("template has no source_url; cannot import".into());
    };
    let mut zfs = node.zfs.clone();
    zfs.import_template(ImportTemplateRequest {
        name: tmpl.spec.name.clone(),
        source: source.to_string(),
        size_bytes: None,
    })
    .await
    .map_err(|s| format!("import_template: {}", s.message()))?;

    Ok(Progress {
        phase: TemplatePhase::Importing,
        bytes_written: 0,
        size_bytes: 0,
    })
}

async fn find_template(
    node: &NodeHandle,
    name: &str,
) -> std::result::Result<Option<mvirt_daemon_protos::zfs::Template>, String> {
    let mut zfs = node.zfs.clone();
    let resp = zfs
        .list_templates(ListTemplatesRequest {})
        .await
        .map_err(|s| format!("list_templates: {}", s.message()))?;
    Ok(resp.into_inner().templates.into_iter().find(|t| t.name == name))
}

async fn find_active_job(
    node: &NodeHandle,
    template_name: &str,
) -> std::result::Result<Option<mvirt_daemon_protos::zfs::ImportJob>, String> {
    let mut zfs = node.zfs.clone();
    // include_completed=true so we also see the failed/cancelled tail and
    // surface those errors back to the UI instead of silently retrying.
    let resp = zfs
        .list_import_jobs(ListImportJobsRequest {
            include_completed: true,
        })
        .await
        .map_err(|s| format!("list_import_jobs: {}", s.message()))?;
    Ok(resp
        .into_inner()
        .jobs
        .into_iter()
        .filter(|j| j.template_name == template_name)
        .max_by(|a, b| a.created_at.cmp(&b.created_at)))
}

fn job_to_progress(job: &mvirt_daemon_protos::zfs::ImportJob) -> Progress {
    let phase = match ImportJobState::try_from(job.state).unwrap_or(ImportJobState::Unspecified) {
        ImportJobState::Pending
        | ImportJobState::Downloading
        | ImportJobState::Converting
        | ImportJobState::Writing
        | ImportJobState::Unspecified => TemplatePhase::Importing,
        ImportJobState::Completed => TemplatePhase::Ready,
        ImportJobState::Failed | ImportJobState::Cancelled => TemplatePhase::Failed,
    };
    Progress {
        phase,
        bytes_written: job.bytes_written,
        size_bytes: job
            .template
            .as_ref()
            .map(|t| t.size_bytes)
            .unwrap_or(job.total_bytes),
    }
}
