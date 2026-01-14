use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tonic::{Request, Response, Status};
use tracing::info;

use crate::audit::ZfsAuditLogger;
use crate::import::{ImportManager, ImportSource};
use crate::proto::zfs_service_server::ZfsService;
use crate::proto::*;
use crate::store::{Store, TemplateEntry, VolumeEntry};
use crate::zfs::ZfsManager;

pub struct ZfsServiceImpl {
    store: Arc<Store>,
    zfs: Arc<ZfsManager>,
    import: Arc<ImportManager>,
    audit: Arc<ZfsAuditLogger>,
}

impl ZfsServiceImpl {
    pub fn new(
        store: Arc<Store>,
        zfs: Arc<ZfsManager>,
        import: Arc<ImportManager>,
        audit: Arc<ZfsAuditLogger>,
    ) -> Self {
        Self {
            store,
            zfs,
            import,
            audit,
        }
    }

    /// Garbage collect base ZVOL if it's orphaned (no template entry, no other volumes)
    async fn maybe_gc_base_zvol(&self, template_id: &str) {
        use tracing::warn;

        // Check if template still exists in DB
        let template_exists = self
            .store
            .template_exists(template_id)
            .await
            .unwrap_or(true); // Assume exists on error, don't GC

        if template_exists {
            return; // Template still exists, don't GC
        }

        // Check if any other volumes depend on this template
        let dependent_count = self
            .store
            .count_volumes_by_origin(template_id)
            .await
            .unwrap_or(1); // Assume exists on error, don't GC

        if dependent_count > 0 {
            return; // Other volumes still depend on this base
        }

        // Safe to garbage collect
        info!(template_id = %template_id, "Garbage collecting orphaned base ZVOL");

        if let Err(e) = self.zfs.delete_base_zvol(template_id).await {
            warn!(
                template_id = %template_id,
                error = %e,
                "Failed to garbage collect base ZVOL"
            );
        }
    }
}

#[tonic::async_trait]
impl ZfsService for ZfsServiceImpl {
    // === Pool operations ===

    async fn get_pool_stats(
        &self,
        _request: Request<GetPoolStatsRequest>,
    ) -> Result<Response<PoolStats>, Status> {
        let stats = self
            .zfs
            .get_pool_stats()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(PoolStats {
            name: stats.name,
            total_bytes: stats.total_bytes,
            available_bytes: stats.available_bytes,
            used_bytes: stats.used_bytes,
            provisioned_bytes: stats.provisioned_bytes,
            compression_ratio: stats.compression_ratio,
        }))
    }

    // === Volume CRUD ===

    async fn create_volume(
        &self,
        request: Request<CreateVolumeRequest>,
    ) -> Result<Response<Volume>, Status> {
        let req = request.into_inner();

        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        if req.size_bytes == 0 {
            return Err(Status::invalid_argument("size_bytes must be > 0"));
        }

        // Check if volume already exists in DB
        if self
            .store
            .get_volume_by_name(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .is_some()
        {
            return Err(Status::already_exists(format!(
                "Volume '{}' already exists",
                req.name
            )));
        }

        // Generate volume UUID
        let volume_id = uuid::Uuid::new_v4().to_string();

        // Create the ZFS volume using UUID
        let vol = self
            .zfs
            .create_volume(&volume_id, req.size_bytes, req.volblocksize)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store in database with name as label
        let entry = VolumeEntry::new(
            volume_id.clone(),
            req.name.clone(),
            self.zfs.volume_zfs_path(&volume_id),
            vol.device_path.clone(),
            req.size_bytes,
            None, // No origin template for empty volumes
        );

        self.store
            .create_volume(&entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, id = %entry.id, "Volume created and stored in database");

        // Audit log
        self.audit
            .volume_created(&entry.id, &entry.name, req.size_bytes)
            .await;

        Ok(Response::new(volume_to_proto(&entry, &vol)))
    }

    async fn list_volumes(
        &self,
        _request: Request<ListVolumesRequest>,
    ) -> Result<Response<ListVolumesResponse>, Status> {
        // Get volumes from database
        let db_volumes = self
            .store
            .list_volumes()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Get current ZFS state for each volume
        let mut volumes = Vec::new();
        for entry in db_volumes {
            match self.zfs.get_volume(&entry.id).await {
                Ok(vol) => {
                    volumes.push(volume_to_proto(&entry, &vol));
                }
                Err(_) => {
                    // Volume exists in DB but not in ZFS - could be orphaned
                    // For now, skip it (could add cleanup logic later)
                }
            }
        }

        Ok(Response::new(ListVolumesResponse { volumes }))
    }

    async fn get_volume(
        &self,
        request: Request<GetVolumeRequest>,
    ) -> Result<Response<Volume>, Status> {
        let req = request.into_inner();

        // Get from database
        let entry = self
            .store
            .get_volume_by_name(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.name)))?;

        // Get current ZFS state using UUID
        let vol = self
            .zfs
            .get_volume(&entry.id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;

        Ok(Response::new(volume_to_proto(&entry, &vol)))
    }

    async fn delete_volume(
        &self,
        request: Request<DeleteVolumeRequest>,
    ) -> Result<Response<DeleteVolumeResponse>, Status> {
        let req = request.into_inner();

        // Get from database to get ID and origin template
        let entry = self
            .store
            .get_volume_by_name(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.name)))?;

        let origin_template_id = entry.origin_template_id.clone();

        // Delete from ZFS (using UUID, includes snapshots)
        self.zfs
            .delete_volume_recursive(&entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete from database (cascades to snapshots)
        self.store
            .delete_volume(&entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, id = %entry.id, "Volume deleted from database");

        // Audit log
        self.audit.volume_deleted(&entry.id, &entry.name).await;

        // Garbage collection: check if origin template's base ZVOL is orphaned
        if let Some(template_id) = origin_template_id {
            self.maybe_gc_base_zvol(&template_id).await;
        }

        Ok(Response::new(DeleteVolumeResponse { deleted: true }))
    }

    async fn resize_volume(
        &self,
        request: Request<ResizeVolumeRequest>,
    ) -> Result<Response<Volume>, Status> {
        let req = request.into_inner();

        // Get from database
        let entry = self
            .store
            .get_volume_by_name(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.name)))?;

        // Resize in ZFS using UUID
        let vol = self
            .zfs
            .resize_volume(&entry.id, req.new_size_bytes)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Update database
        self.store
            .update_volume_size(&entry.id, req.new_size_bytes)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, new_size = %req.new_size_bytes, "Volume resized");

        // Audit log
        self.audit
            .volume_resized(&entry.id, &entry.name, req.new_size_bytes)
            .await;

        Ok(Response::new(volume_to_proto(&entry, &vol)))
    }

    // === Import operations (creates templates) ===

    async fn import_template(
        &self,
        request: Request<ImportTemplateRequest>,
    ) -> Result<Response<ImportJob>, Status> {
        let req = request.into_inner();

        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        if req.source.is_empty() {
            return Err(Status::invalid_argument("source is required"));
        }

        // Check if template already exists
        if self
            .store
            .get_template(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .is_some()
        {
            return Err(Status::already_exists(format!(
                "Template '{}' already exists",
                req.name
            )));
        }

        let source = ImportSource::parse(&req.source);

        let job = self
            .import
            .start_import(req.name, source, req.size_bytes)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(import_job_to_proto(&job, None)))
    }

    async fn get_import_job(
        &self,
        request: Request<GetImportJobRequest>,
    ) -> Result<Response<ImportJob>, Status> {
        let req = request.into_inner();

        let job = self
            .import
            .get_job(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Import job '{}' not found", req.id)))?;

        // If completed, get the template
        let template = if job.state == "completed" {
            match self.store.get_template(&job.template_name).await {
                Ok(Some(entry)) => Some(template_to_proto(&entry, 0)),
                _ => None,
            }
        } else {
            None
        };

        Ok(Response::new(import_job_to_proto(&job, template)))
    }

    async fn list_import_jobs(
        &self,
        request: Request<ListImportJobsRequest>,
    ) -> Result<Response<ListImportJobsResponse>, Status> {
        let req = request.into_inner();

        let jobs = self
            .import
            .list_jobs(req.include_completed)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let jobs: Vec<ImportJob> = jobs.iter().map(|j| import_job_to_proto(j, None)).collect();

        Ok(Response::new(ListImportJobsResponse { jobs }))
    }

    async fn cancel_import_job(
        &self,
        request: Request<CancelImportJobRequest>,
    ) -> Result<Response<CancelImportJobResponse>, Status> {
        let req = request.into_inner();

        let cancelled = self
            .import
            .cancel_job(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(CancelImportJobResponse { cancelled }))
    }

    // === Snapshot operations ===

    async fn create_snapshot(
        &self,
        request: Request<CreateSnapshotRequest>,
    ) -> Result<Response<Snapshot>, Status> {
        let req = request.into_inner();

        // Verify volume exists
        let vol_entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Generate snapshot UUID
        let snapshot_id = uuid::Uuid::new_v4().to_string();

        // Create ZFS snapshot using volume's UUID and snapshot UUID
        let snap = self
            .zfs
            .create_snapshot(&vol_entry.id, &snapshot_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store snapshot in database with user-friendly name
        let snap_entry = crate::store::SnapshotEntry::new(
            snapshot_id,
            vol_entry.id.clone(),
            req.snapshot_name.clone(),
            snap.name.clone(), // ZFS snapshot name (UUID)
        );

        self.store
            .create_snapshot(&snap_entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Audit log
        self.audit
            .snapshot_created(&vol_entry.id, &req.snapshot_name)
            .await;

        Ok(Response::new(snapshot_to_proto(&snap_entry, &snap)))
    }

    async fn list_snapshots(
        &self,
        request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<ListSnapshotsResponse>, Status> {
        let req = request.into_inner();

        // Get volume from database
        let entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Get snapshots from ZFS using volume UUID
        let zfs_snapshots = self
            .zfs
            .list_snapshots(&entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Get snapshots from database
        let db_snapshots = self
            .store
            .list_snapshots(&entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Combine: use DB for IDs, ZFS for current stats
        let snapshots: Vec<Snapshot> = zfs_snapshots
            .iter()
            .map(|zfs_snap| {
                let db_snap = db_snapshots.iter().find(|s| s.name == zfs_snap.name);
                if let Some(db) = db_snap {
                    snapshot_to_proto(db, zfs_snap)
                } else {
                    // Snapshot exists in ZFS but not DB (created outside mvirt-zfs)
                    Snapshot {
                        id: String::new(),
                        name: zfs_snap.name.clone(),
                        full_name: zfs_snap.full_name.clone(),
                        used_bytes: zfs_snap.used_bytes,
                        created_at: timestamp_to_iso(zfs_snap.creation_timestamp),
                    }
                }
            })
            .collect();

        Ok(Response::new(ListSnapshotsResponse { snapshots }))
    }

    async fn delete_snapshot(
        &self,
        request: Request<DeleteSnapshotRequest>,
    ) -> Result<Response<DeleteSnapshotResponse>, Status> {
        let req = request.into_inner();

        // Get volume from database
        let vol_entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Get snapshot from database to get ZFS snapshot name
        let snap_entry = self
            .store
            .get_snapshot(&vol_entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| {
                Status::not_found(format!("Snapshot '{}' not found", req.snapshot_name))
            })?;

        // Delete from ZFS using volume UUID and snapshot UUID
        self.zfs
            .delete_snapshot(&vol_entry.id, &snap_entry.zfs_snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete from database
        self.store
            .delete_snapshot(&vol_entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Audit log
        self.audit
            .snapshot_deleted(&vol_entry.id, &req.snapshot_name)
            .await;

        Ok(Response::new(DeleteSnapshotResponse { deleted: true }))
    }

    async fn rollback_snapshot(
        &self,
        request: Request<RollbackSnapshotRequest>,
    ) -> Result<Response<Volume>, Status> {
        let req = request.into_inner();

        // Get volume from database
        let vol_entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Get snapshot from database to get ZFS snapshot name
        let snap_entry = self
            .store
            .get_snapshot(&vol_entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| {
                Status::not_found(format!("Snapshot '{}' not found", req.snapshot_name))
            })?;

        let vol = self
            .zfs
            .rollback_snapshot(&vol_entry.id, &snap_entry.zfs_snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Audit log
        self.audit
            .snapshot_rollback(&vol_entry.id, &req.snapshot_name)
            .await;

        Ok(Response::new(volume_to_proto(&vol_entry, &vol)))
    }

    // === Template operations ===

    async fn promote_snapshot_to_template(
        &self,
        request: Request<PromoteSnapshotRequest>,
    ) -> Result<Response<Template>, Status> {
        let req = request.into_inner();

        // Get volume from database
        let vol_entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Get snapshot from database
        let snap_entry = self
            .store
            .get_snapshot(&vol_entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| {
                Status::not_found(format!("Snapshot '{}' not found", req.snapshot_name))
            })?;

        // Get volume info for size
        let vol = self
            .zfs
            .get_volume(&vol_entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Generate template UUID
        let template_id = uuid::Uuid::new_v4().to_string();

        // Clone the snapshot to base ZVOL
        let snap_path = format!(
            "{}@{}",
            self.zfs.volume_zfs_path(&vol_entry.id),
            snap_entry.zfs_snapshot_name
        );
        self.zfs
            .clone_snapshot(&snap_path, &self.zfs.base_zvol_path(&template_id))
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Create template snapshot (@img)
        let snapshot_path = self
            .zfs
            .create_template_snapshot(&template_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store template in database
        let template_entry = TemplateEntry::new(
            template_id.clone(),
            req.template_name.clone(),
            self.zfs.base_zvol_path(&template_id),
            snapshot_path,
            vol.volsize_bytes,
        );

        self.store
            .create_template(&template_entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(
            name = %req.template_name,
            template_id = %template_id,
            source_volume = %req.volume_name,
            source_snapshot = %req.snapshot_name,
            "Template created from snapshot"
        );

        // Audit log
        self.audit
            .template_created(&template_entry.id, &req.template_name, &req.volume_name)
            .await;

        Ok(Response::new(template_to_proto(&template_entry, 0)))
    }

    async fn list_templates(
        &self,
        _request: Request<ListTemplatesRequest>,
    ) -> Result<Response<ListTemplatesResponse>, Status> {
        let templates = self
            .store
            .list_templates()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // TODO: Count clones for each template
        let templates: Vec<Template> = templates.iter().map(|t| template_to_proto(t, 0)).collect();

        Ok(Response::new(ListTemplatesResponse { templates }))
    }

    async fn delete_template(
        &self,
        request: Request<DeleteTemplateRequest>,
    ) -> Result<Response<DeleteTemplateResponse>, Status> {
        let req = request.into_inner();

        // Get template from database
        let template = self
            .store
            .get_template(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Template '{}' not found", req.name)))?;

        let template_id = template.id.clone();

        // Delete from database only - ZFS resources stay for dependent volumes
        self.store
            .delete_template(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, template_id = %template_id, "Template deleted from database");

        // Audit log
        self.audit.template_deleted(&template_id, &req.name).await;

        // Try to GC base ZVOL (will succeed only if no volumes depend on it)
        self.maybe_gc_base_zvol(&template_id).await;

        Ok(Response::new(DeleteTemplateResponse { deleted: true }))
    }

    async fn clone_from_template(
        &self,
        request: Request<CloneFromTemplateRequest>,
    ) -> Result<Response<Volume>, Status> {
        let req = request.into_inner();

        // Get template from database
        let template = self
            .store
            .get_template(&req.template_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| {
                Status::not_found(format!("Template '{}' not found", req.template_name))
            })?;

        // Determine final volume size
        let volume_size = req.size_bytes.unwrap_or(template.size_bytes);
        if volume_size < template.size_bytes {
            return Err(Status::invalid_argument(format!(
                "Volume size {} is smaller than template size {}",
                volume_size, template.size_bytes
            )));
        }

        // Generate volume UUID
        let volume_id = uuid::Uuid::new_v4().to_string();

        // Clone the template to new volume
        let mut vol = self
            .zfs
            .clone_to_volume(&template.id, &volume_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Expand volume if requested size is larger than template
        if volume_size > template.size_bytes {
            vol = self
                .zfs
                .resize_volume(&volume_id, volume_size)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }

        // Store new volume in database with origin template
        let entry = VolumeEntry::new(
            volume_id.clone(),
            req.new_volume_name.clone(),
            self.zfs.volume_zfs_path(&volume_id),
            vol.device_path.clone(),
            volume_size,
            Some(template.id.clone()), // origin_template_id
        );

        self.store
            .create_volume(&entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(
            name = %req.new_volume_name,
            volume_id = %volume_id,
            template = %req.template_name,
            size_bytes = %volume_size,
            "Volume cloned from template"
        );

        // Audit log
        self.audit
            .volume_cloned(&entry.id, &req.new_volume_name, &req.template_name)
            .await;

        Ok(Response::new(volume_to_proto(&entry, &vol)))
    }
}

// === Helper functions ===

fn volume_to_proto(entry: &VolumeEntry, vol: &crate::zfs::VolumeInfo) -> Volume {
    Volume {
        id: entry.id.clone(),
        name: entry.name.clone(),
        path: entry.device_path.clone(),
        volsize_bytes: vol.volsize_bytes,
        used_bytes: vol.used_bytes,
        volblocksize: vol.volblocksize,
        compression_ratio: vol.compression_ratio,
        created_at: entry.created_at.clone(),
        snapshots: vec![], // Populated separately if needed
    }
}

fn snapshot_to_proto(
    entry: &crate::store::SnapshotEntry,
    snap: &crate::zfs::SnapshotInfo,
) -> Snapshot {
    Snapshot {
        id: entry.id.clone(),
        name: entry.name.clone(),
        full_name: snap.full_name.clone(),
        used_bytes: snap.used_bytes,
        created_at: entry.created_at.clone(),
    }
}

fn template_to_proto(entry: &TemplateEntry, clone_count: u32) -> Template {
    Template {
        id: entry.id.clone(),
        name: entry.name.clone(),
        snapshot_path: entry.snapshot_path.clone(),
        source_volume: String::new(), // No longer tracked
        size_bytes: entry.size_bytes,
        created_at: entry.created_at.clone(),
        clone_count,
    }
}

fn timestamp_to_iso(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

fn import_job_to_proto(
    entry: &crate::store::ImportJobEntry,
    template: Option<Template>,
) -> ImportJob {
    let state = match entry.state.as_str() {
        "pending" => ImportJobState::Pending,
        "downloading" => ImportJobState::Downloading,
        "converting" => ImportJobState::Converting,
        "writing" => ImportJobState::Writing,
        "completed" => ImportJobState::Completed,
        "failed" => ImportJobState::Failed,
        "cancelled" => ImportJobState::Cancelled,
        _ => ImportJobState::Unspecified,
    };

    ImportJob {
        id: entry.id.clone(),
        template_name: entry.template_name.clone(),
        source: entry.source.clone(),
        state: state.into(),
        bytes_written: entry.bytes_written,
        total_bytes: entry.total_bytes.unwrap_or(0),
        error: entry.error.clone(),
        template,
        created_at: entry.created_at.clone(),
        completed_at: entry.completed_at.clone(),
    }
}
