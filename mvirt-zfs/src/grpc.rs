use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::info;

use crate::audit::ZfsAuditLogger;
use crate::import::{ImportManager, ImportSource};
use crate::proto::zfs_service_server::ZfsService;
use crate::proto::*;
use crate::store::{SnapshotEntry, Store, TemplateEntry, VolumeEntry};
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

    /// Sync DB snapshot entries with actual ZFS snapshots after rollback.
    /// Removes any DB entries for snapshots that no longer exist in ZFS.
    async fn sync_snapshots_after_rollback(&self, volume_id: &str) {
        // Get remaining ZFS snapshots
        let zfs_snapshots = match self.zfs.list_snapshots(volume_id).await {
            Ok(snaps) => snaps,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list ZFS snapshots for sync");
                return;
            }
        };
        let zfs_snap_names: std::collections::HashSet<_> =
            zfs_snapshots.iter().map(|s| s.name.as_str()).collect();

        // Get DB entries
        let db_snaps = self
            .store
            .list_snapshots(volume_id)
            .await
            .unwrap_or_default();

        // Find and delete orphaned entries
        for db_snap in db_snaps {
            if !zfs_snap_names.contains(db_snap.zfs_name.as_str()) {
                // ZFS snapshot no longer exists - delete DB entry
                if let Err(e) = self.store.delete_snapshot_by_id(&db_snap.id).await {
                    tracing::warn!(error = %e, snapshot_id = %db_snap.id, "Failed to delete orphaned snapshot");
                } else {
                    tracing::info!(snapshot_name = %db_snap.name, "Deleted snapshot destroyed by rollback");
                }
            }
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
                    let mut volume = volume_to_proto(&entry, &vol);

                    // Load snapshots for this volume
                    let zfs_snapshots =
                        self.zfs.list_snapshots(&entry.id).await.unwrap_or_default();
                    let db_snapshots = self
                        .store
                        .list_snapshots(&entry.id)
                        .await
                        .unwrap_or_default();

                    volume.snapshots = zfs_snapshots
                        .iter()
                        .filter_map(|zfs_snap| {
                            // Find the DB snapshot entry by zfs_name
                            let db_snap =
                                db_snapshots.iter().find(|s| s.zfs_name == zfs_snap.name)?;
                            Some(snapshot_to_proto(db_snap, zfs_snap))
                        })
                        .collect();

                    volumes.push(volume);
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

        // Get snapshots
        let zfs_snapshots = self.zfs.list_snapshots(&entry.id).await.unwrap_or_default();
        let db_snapshots = self
            .store
            .list_snapshots(&entry.id)
            .await
            .unwrap_or_default();

        let snapshots: Vec<Snapshot> = zfs_snapshots
            .iter()
            .filter_map(|zfs_snap| {
                // Find the DB snapshot entry by zfs_name
                let db_snap = db_snapshots.iter().find(|s| s.zfs_name == zfs_snap.name)?;
                Some(snapshot_to_proto(db_snap, zfs_snap))
            })
            .collect();

        let mut volume = volume_to_proto(&entry, &vol);
        volume.snapshots = snapshots;

        Ok(Response::new(volume))
    }

    async fn delete_volume(
        &self,
        request: Request<DeleteVolumeRequest>,
    ) -> Result<Response<DeleteVolumeResponse>, Status> {
        let req = request.into_inner();

        // Get from database to get ID
        let entry = self
            .store
            .get_volume_by_name(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.name)))?;

        let zfs_path = self.zfs.volume_zfs_path(&entry.id);

        // Delete volume and all its snapshots (recursive)
        // Note: Templates are now independent copies, so we don't need clone checks for volumes
        self.zfs
            .destroy_recursive(&zfs_path)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete from database
        self.store
            .delete_volume_with_snapshots(&entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, id = %entry.id, "Volume deleted");

        // Audit log
        self.audit.volume_deleted(&entry.id, &entry.name).await;

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

        // Generate UUIDs
        let zfs_name = uuid::Uuid::new_v4().to_string();
        let snapshot_id = uuid::Uuid::new_v4().to_string();

        // Create ZFS snapshot using volume's UUID and ZFS snapshot UUID
        let snap = self
            .zfs
            .create_snapshot(&vol_entry.id, &zfs_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store snapshot in database
        let snap_entry = SnapshotEntry::new(
            snapshot_id,
            vol_entry.id.clone(),
            req.snapshot_name.clone(),
            zfs_name,
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

        // Get snapshot entries from database
        let db_snapshots = self
            .store
            .list_snapshots(&entry.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Combine: use DB for IDs, ZFS for current stats
        let snapshots: Vec<Snapshot> = zfs_snapshots
            .iter()
            .filter_map(|zfs_snap| {
                // Find the DB snapshot entry by zfs_name
                let db_snap = db_snapshots.iter().find(|s| s.zfs_name == zfs_snap.name)?;
                Some(snapshot_to_proto(db_snap, zfs_snap))
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

        // Get snapshot from database
        let snap_entry = self
            .store
            .get_snapshot(&vol_entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| {
                Status::not_found(format!("Snapshot '{}' not found", req.snapshot_name))
            })?;

        // Delete from ZFS
        self.zfs
            .delete_snapshot(&vol_entry.id, &snap_entry.zfs_name)
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

        // Get snapshot from database
        let snap_entry = self
            .store
            .get_snapshot(&vol_entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| {
                Status::not_found(format!("Snapshot '{}' not found", req.snapshot_name))
            })?;

        // Perform ZFS rollback (uses -r to destroy newer snapshots)
        let vol = self
            .zfs
            .rollback_snapshot(&vol_entry.id, &snap_entry.zfs_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Clean up DB entries for snapshots destroyed by rollback
        self.sync_snapshots_after_rollback(&vol_entry.id).await;

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

        // Check if template name already exists
        if self
            .store
            .get_template(&req.template_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .is_some()
        {
            return Err(Status::already_exists(format!(
                "Template '{}' already exists",
                req.template_name
            )));
        }

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

        // Build ZFS paths
        let snapshot_path = format!(
            "{}@{}",
            self.zfs.volume_zfs_path(&vol_entry.id),
            snap_entry.zfs_name
        );
        let template_zfs_path = self.zfs.template_zfs_path(&template_id);

        // Copy the snapshot to create an independent template ZVOL
        info!(
            template_name = %req.template_name,
            template_id = %template_id,
            source_snapshot = %snapshot_path,
            "Creating template by copying snapshot (independent copy)"
        );

        self.zfs
            .copy_snapshot_to_dataset(&snapshot_path, &template_zfs_path)
            .await
            .map_err(|e| Status::internal(format!("Failed to copy snapshot: {}", e)))?;

        // Create @img snapshot on the new template for future cloning
        let template_snapshot_path = self
            .zfs
            .create_template_snapshot(&template_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to create template snapshot: {}", e)))?;

        // Create template entry in database
        let template_entry = TemplateEntry::new(
            template_id.clone(),
            req.template_name.clone(),
            template_zfs_path,
            template_snapshot_path,
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

        // Get ZFS path for template
        let zfs_path = if let Some(ref base_zvol_path) = template.base_zvol_path {
            base_zvol_path.clone()
        } else {
            self.zfs.template_zfs_path(&template_id)
        };

        // Check if template's @img snapshot has clones (volumes created from it)
        let clones = self
            .zfs
            .get_snapshot_clones(&zfs_path)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // If clones exist, promote one to transfer ownership of shared data
        if !clones.is_empty() {
            let clone_to_promote = &clones[0];
            info!(
                template = %req.name,
                clone = %clone_to_promote,
                "Template has clones, promoting first clone to preserve data"
            );

            self.zfs
                .promote(clone_to_promote)
                .await
                .map_err(|e| Status::internal(format!("Failed to promote clone: {}", e)))?;
        }

        // Now safe to delete template ZVOL
        self.zfs
            .destroy(&zfs_path)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete template from database
        self.store
            .delete_template(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, template_id = %template_id, "Template deleted");

        // Audit log
        self.audit.template_deleted(&template_id, &req.name).await;

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

        // Clone from template's @img snapshot
        let mut vol = self
            .zfs
            .clone_template_to_volume(&template.id, &volume_id)
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
        // For promoted templates, snapshot_path is empty (uses zfs_snapshot_id internally)
        snapshot_path: entry.snapshot_path.clone().unwrap_or_default(),
        source_volume: String::new(), // No longer tracked
        size_bytes: entry.size_bytes,
        created_at: entry.created_at.clone(),
        clone_count,
    }
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
