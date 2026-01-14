use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tonic::{Request, Response, Status};
use tracing::info;

use crate::import::{ImportManager, ImportSource};
use crate::proto::zfs_service_server::ZfsService;
use crate::proto::*;
use crate::store::{Store, TemplateEntry, VolumeEntry};
use crate::zfs::ZfsManager;

pub struct ZfsServiceImpl {
    pool_name: String,
    store: Arc<Store>,
    zfs: Arc<ZfsManager>,
    import: Arc<ImportManager>,
}

impl ZfsServiceImpl {
    pub fn new(
        pool_name: String,
        store: Arc<Store>,
        zfs: Arc<ZfsManager>,
        import: Arc<ImportManager>,
    ) -> Self {
        Self {
            pool_name,
            store,
            zfs,
            import,
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

        // Create the ZFS volume
        let vol = self
            .zfs
            .create_volume(&req.name, req.size_bytes, req.volblocksize)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store in database
        let entry = VolumeEntry::new(
            req.name.clone(),
            self.zfs.volume_zfs_path(&req.name),
            vol.device_path.clone(),
            req.size_bytes,
            Some("create".to_string()),
            None,
        );

        self.store
            .create_volume(&entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, id = %entry.id, "Volume created and stored in database");

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
            match self.zfs.get_volume(&entry.name).await {
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

        // Get current ZFS state
        let vol = self
            .zfs
            .get_volume(&req.name)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;

        Ok(Response::new(volume_to_proto(&entry, &vol)))
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
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete from ZFS
        self.zfs
            .delete_volume(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete from database
        if let Some(entry) = entry {
            self.store
                .delete_volume(&entry.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            info!(name = %req.name, id = %entry.id, "Volume deleted from database");
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

        // Resize in ZFS
        let vol = self
            .zfs
            .resize_volume(&req.name, req.new_size_bytes)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Update database
        self.store
            .update_volume_size(&entry.id, req.new_size_bytes)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, new_size = %req.new_size_bytes, "Volume resized");

        Ok(Response::new(volume_to_proto(&entry, &vol)))
    }

    // === Import operations ===

    async fn import_volume(
        &self,
        request: Request<ImportVolumeRequest>,
    ) -> Result<Response<ImportJob>, Status> {
        let req = request.into_inner();

        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        if req.source.is_empty() {
            return Err(Status::invalid_argument("source is required"));
        }

        // Check if volume already exists
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

        // If completed, get the volume
        let volume = if job.state == "completed" {
            match self.store.get_volume_by_name(&job.volume_name).await {
                Ok(Some(entry)) => match self.zfs.get_volume(&entry.name).await {
                    Ok(vol) => Some(volume_to_proto(&entry, &vol)),
                    Err(_) => None,
                },
                _ => None,
            }
        } else {
            None
        };

        Ok(Response::new(import_job_to_proto(&job, volume)))
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
        let entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        let snap = self
            .zfs
            .create_snapshot(&req.volume_name, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store snapshot in database
        let snap_entry = crate::store::SnapshotEntry::new(
            entry.id.clone(),
            req.snapshot_name.clone(),
            snap.full_name.clone(),
        );

        self.store
            .create_snapshot(&snap_entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

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

        // Get snapshots from ZFS
        let zfs_snapshots = self
            .zfs
            .list_snapshots(&req.volume_name)
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
        let entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Delete from ZFS
        self.zfs
            .delete_snapshot(&req.volume_name, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete from database
        self.store
            .delete_snapshot(&entry.id, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(DeleteSnapshotResponse { deleted: true }))
    }

    async fn rollback_snapshot(
        &self,
        request: Request<RollbackSnapshotRequest>,
    ) -> Result<Response<Volume>, Status> {
        let req = request.into_inner();

        // Get volume from database
        let entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        let vol = self
            .zfs
            .rollback_snapshot(&req.volume_name, &req.snapshot_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(volume_to_proto(&entry, &vol)))
    }

    // === Template operations ===

    async fn create_template(
        &self,
        request: Request<CreateTemplateRequest>,
    ) -> Result<Response<Template>, Status> {
        let req = request.into_inner();

        // Get volume from database
        let vol_entry = self
            .store
            .get_volume_by_name(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("Volume '{}' not found", req.volume_name)))?;

        // Create snapshot in ZFS
        let snap = self
            .zfs
            .create_snapshot(&req.volume_name, &req.template_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Get volume info for size
        let vol = self
            .zfs
            .get_volume(&req.volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store template in database
        let template_entry = TemplateEntry::new(
            req.template_name.clone(),
            snap.full_name.clone(),
            Some(vol_entry.id.clone()),
            vol.volsize_bytes,
        );

        self.store
            .create_template(&template_entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(
            name = %req.template_name,
            source_volume = %req.volume_name,
            "Template created"
        );

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

        // Delete the snapshot from ZFS
        // snapshot_path format: pool/volume@snapshot
        let parts: Vec<&str> = template.snapshot_path.split('@').collect();
        if parts.len() == 2 {
            let volume_path = parts[0];
            let snap_name = parts[1];
            // Extract volume name from path (remove pool prefix)
            let volume_name = volume_path
                .strip_prefix(&format!("{}/", self.pool_name))
                .unwrap_or(volume_path);

            self.zfs
                .delete_snapshot(volume_name, snap_name)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }

        // Delete from database
        self.store
            .delete_template(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(name = %req.name, "Template deleted");

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

        // Clone the snapshot
        let vol = self
            .zfs
            .clone_snapshot(&template.snapshot_path, &req.new_volume_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Store new volume in database
        let entry = VolumeEntry::new(
            req.new_volume_name.clone(),
            self.zfs.volume_zfs_path(&req.new_volume_name),
            vol.device_path.clone(),
            template.size_bytes,
            Some("clone".to_string()),
            Some(template.snapshot_path.clone()),
        );

        self.store
            .create_volume(&entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(
            name = %req.new_volume_name,
            template = %req.template_name,
            "Volume cloned from template"
        );

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
        full_name: entry.snapshot_path.clone(),
        used_bytes: snap.used_bytes,
        created_at: entry.created_at.clone(),
    }
}

fn template_to_proto(entry: &TemplateEntry, clone_count: u32) -> Template {
    Template {
        id: entry.id.clone(),
        name: entry.name.clone(),
        snapshot_path: entry.snapshot_path.clone(),
        source_volume: entry.source_volume_id.clone().unwrap_or_default(),
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

fn import_job_to_proto(entry: &crate::store::ImportJobEntry, volume: Option<Volume>) -> ImportJob {
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
        volume_name: entry.volume_name.clone(),
        source: entry.source.clone(),
        state: state.into(),
        bytes_written: entry.bytes_written,
        total_bytes: entry.total_bytes.unwrap_or(0),
        error: entry.error.clone(),
        volume,
        created_at: entry.created_at.clone(),
        completed_at: entry.completed_at.clone(),
    }
}
