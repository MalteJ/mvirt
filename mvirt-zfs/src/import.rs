//! Image import module
//!
//! Handles importing raw and qcow2 images from local files and HTTP(S) URLs.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, oneshot};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::store::{ImportJobEntry, Store, VolumeEntry};
use crate::zfs::ZfsManager;

/// Image format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Raw,
    Qcow2,
}

/// Import source
#[derive(Debug, Clone)]
pub enum ImportSource {
    LocalFile(String),
    HttpUrl(String),
}

impl ImportSource {
    pub fn parse(source: &str) -> Self {
        if source.starts_with("http://") || source.starts_with("https://") {
            ImportSource::HttpUrl(source.to_string())
        } else {
            ImportSource::LocalFile(source.to_string())
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ImportSource::LocalFile(p) => p,
            ImportSource::HttpUrl(u) => u,
        }
    }
}

/// State of an in-memory import job
struct RunningJob {
    cancel_tx: Option<oneshot::Sender<()>>,
}

/// Import manager handles async import operations
pub struct ImportManager {
    #[allow(dead_code)]
    pool_name: String,
    pool_mountpoint: String,
    store: Arc<Store>,
    zfs: Arc<ZfsManager>,
    running_jobs: Arc<RwLock<HashMap<String, RunningJob>>>,
}

impl ImportManager {
    pub fn new(
        pool_name: String,
        pool_mountpoint: String,
        store: Arc<Store>,
        zfs: Arc<ZfsManager>,
    ) -> Self {
        Self {
            pool_name,
            pool_mountpoint,
            store,
            zfs,
            running_jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Detect image format from file header
    pub async fn detect_format_from_file(path: &str) -> Result<ImageFormat> {
        let mut file = File::open(path).await.context("Failed to open file")?;

        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)
            .await
            .context("Failed to read file header")?;

        // qcow2 magic: "QFI\xfb" (0x514649fb big-endian)
        if magic == [0x51, 0x46, 0x49, 0xfb] {
            Ok(ImageFormat::Qcow2)
        } else {
            Ok(ImageFormat::Raw)
        }
    }

    /// Detect image format from HTTP Content-Type or URL extension
    pub fn detect_format_from_url(url: &str, content_type: Option<&str>) -> ImageFormat {
        // Check content type first
        if let Some(ct) = content_type
            && (ct.contains("qcow2") || ct.contains("x-qemu-disk"))
        {
            return ImageFormat::Qcow2;
        }

        // Check URL extension
        let lower_url = url.to_lowercase();
        if lower_url.ends_with(".qcow2") || lower_url.contains(".qcow2?") {
            ImageFormat::Qcow2
        } else {
            ImageFormat::Raw
        }
    }

    /// Start an import job
    pub async fn start_import(
        &self,
        volume_name: String,
        source: ImportSource,
        size_bytes: Option<u64>,
    ) -> Result<ImportJobEntry> {
        // Determine format
        let format = match &source {
            ImportSource::LocalFile(path) => Self::detect_format_from_file(path).await?,
            ImportSource::HttpUrl(url) => {
                // We'll refine this when we start downloading
                Self::detect_format_from_url(url, None)
            }
        };

        // For qcow2, we need size_bytes or we'll determine it during conversion
        // For raw from URL, we need size_bytes to create the volume
        let format_str = match format {
            ImageFormat::Raw => "raw",
            ImageFormat::Qcow2 => "qcow2",
        };

        // Create job entry
        let job_entry = ImportJobEntry::new(
            volume_name.clone(),
            source.as_str().to_string(),
            format_str.to_string(),
            size_bytes,
        );

        // Store in database
        self.store.create_import_job(&job_entry).await?;

        info!(
            job_id = %job_entry.id,
            volume = %volume_name,
            source = %source.as_str(),
            format = %format_str,
            "Starting import job"
        );

        // Create cancel channel
        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Register running job
        {
            let mut jobs = self.running_jobs.write().await;
            jobs.insert(
                job_entry.id.clone(),
                RunningJob {
                    cancel_tx: Some(cancel_tx),
                },
            );
        }

        // Spawn background task
        let job_id = job_entry.id.clone();
        let store = Arc::clone(&self.store);
        let zfs = Arc::clone(&self.zfs);
        let pool_mountpoint = self.pool_mountpoint.clone();
        let running_jobs = Arc::clone(&self.running_jobs);

        tokio::spawn(async move {
            let result = Self::run_import(
                &job_id,
                &volume_name,
                source,
                format,
                size_bytes,
                &store,
                &zfs,
                &pool_mountpoint,
                cancel_rx,
            )
            .await;

            // Clean up running job entry
            {
                let mut jobs = running_jobs.write().await;
                jobs.remove(&job_id);
            }

            if let Err(e) = result {
                error!(job_id = %job_id, error = %e, "Import job failed");
            }
        });

        Ok(job_entry)
    }

    /// Get current job state from database
    pub async fn get_job(&self, job_id: &str) -> Result<Option<ImportJobEntry>> {
        self.store.get_import_job(job_id).await
    }

    /// List jobs
    pub async fn list_jobs(&self, include_completed: bool) -> Result<Vec<ImportJobEntry>> {
        self.store.list_import_jobs(include_completed).await
    }

    /// Cancel a running job
    pub async fn cancel_job(&self, job_id: &str) -> Result<bool> {
        let mut jobs = self.running_jobs.write().await;
        if let Some(mut job) = jobs.remove(job_id)
            && let Some(tx) = job.cancel_tx.take()
        {
            let _ = tx.send(());
            // Update job state in database
            self.store
                .update_import_job(job_id, "cancelled", 0, None)
                .await?;
            info!(job_id = %job_id, "Import job cancelled");
            return Ok(true);
        }
        Ok(false)
    }

    /// Run the actual import with centralized error handling
    #[allow(clippy::too_many_arguments)]
    async fn run_import(
        job_id: &str,
        volume_name: &str,
        source: ImportSource,
        format: ImageFormat,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        pool_mountpoint: &str,
        mut cancel_rx: oneshot::Receiver<()>,
    ) -> Result<()> {
        let result = Self::run_import_inner(
            job_id,
            volume_name,
            source,
            format,
            size_bytes,
            store,
            zfs,
            pool_mountpoint,
            &mut cancel_rx,
        )
        .await;

        // Handle errors centrally: update job state and cleanup
        if let Err(ref e) = result {
            let error_msg = e.to_string();
            error!(job_id = %job_id, error = %error_msg, "Import failed");

            // Try to clean up the volume if it was created
            if let Err(cleanup_err) = zfs.delete_volume(volume_name).await {
                // Volume might not exist yet, that's fine
                warn!(
                    job_id = %job_id,
                    volume = %volume_name,
                    error = %cleanup_err,
                    "Failed to cleanup volume after import error (may not exist)"
                );
            }

            // Update job state to failed
            if let Err(db_err) = store
                .update_import_job(job_id, "failed", 0, Some(&error_msg))
                .await
            {
                error!(
                    job_id = %job_id,
                    error = %db_err,
                    "Failed to update job state to failed"
                );
            }
        }

        result
    }

    /// Inner import logic - errors are handled by run_import wrapper
    #[allow(clippy::too_many_arguments)]
    async fn run_import_inner(
        job_id: &str,
        volume_name: &str,
        source: ImportSource,
        format: ImageFormat,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        pool_mountpoint: &str,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        match format {
            ImageFormat::Raw => match source {
                ImportSource::LocalFile(path) => {
                    Self::import_raw_file(
                        job_id,
                        volume_name,
                        &path,
                        size_bytes,
                        store,
                        zfs,
                        cancel_rx,
                    )
                    .await
                }
                ImportSource::HttpUrl(url) => {
                    Self::import_raw_url(
                        job_id,
                        volume_name,
                        &url,
                        size_bytes,
                        store,
                        zfs,
                        cancel_rx,
                    )
                    .await
                }
            },
            ImageFormat::Qcow2 => {
                // qcow2 import requires temp file for random access
                match source {
                    ImportSource::LocalFile(path) => {
                        Self::import_qcow2_file(job_id, volume_name, &path, store, zfs, cancel_rx)
                            .await
                    }
                    ImportSource::HttpUrl(url) => {
                        Self::import_qcow2_url(
                            job_id,
                            volume_name,
                            &url,
                            store,
                            zfs,
                            pool_mountpoint,
                            cancel_rx,
                        )
                        .await
                    }
                }
            }
        }
    }

    /// Import raw file to ZVOL
    async fn import_raw_file(
        job_id: &str,
        volume_name: &str,
        path: &str,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        // Update state to writing
        store.update_import_job(job_id, "writing", 0, None).await?;

        // Get file size
        let metadata = tokio::fs::metadata(path)
            .await
            .context("Failed to get file metadata")?;
        let file_size = size_bytes.unwrap_or(metadata.len());

        // Create ZVOL
        let vol = zfs.create_volume(volume_name, file_size, None).await?;

        // Open source file
        let mut src_file = File::open(path)
            .await
            .context("Failed to open source file")?;

        // Open ZVOL device
        let mut zvol = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&vol.device_path)
            .await
            .context("Failed to open ZVOL device")?;

        // Stream data
        let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer
        let mut bytes_written: u64 = 0;
        let mut last_update = std::time::Instant::now();

        loop {
            // Check for cancellation
            if cancel_rx.try_recv().is_ok() {
                // Clean up: delete the volume
                let _ = zfs.delete_volume(volume_name).await;
                store
                    .update_import_job(job_id, "cancelled", bytes_written, None)
                    .await?;
                return Ok(());
            }

            let n = src_file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            zvol.write_all(&buffer[..n]).await?;
            bytes_written += n as u64;

            // Update progress every second
            if last_update.elapsed().as_secs() >= 1 {
                store
                    .update_import_job(job_id, "writing", bytes_written, None)
                    .await?;
                last_update = std::time::Instant::now();
            }
        }

        zvol.flush().await?;

        // Store volume in database
        let vol_entry = VolumeEntry::new(
            volume_name.to_string(),
            zfs.volume_zfs_path(volume_name),
            vol.device_path.clone(),
            file_size,
            Some("import".to_string()),
            Some(path.to_string()),
        );
        store.create_volume(&vol_entry).await?;

        // Mark completed
        store
            .update_import_job(job_id, "completed", bytes_written, None)
            .await?;

        info!(
            job_id = %job_id,
            volume = %volume_name,
            bytes = %bytes_written,
            "Raw file import completed"
        );

        Ok(())
    }

    /// Import raw from HTTP(S) URL
    async fn import_raw_url(
        job_id: &str,
        volume_name: &str,
        url: &str,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        // Update state to downloading
        store
            .update_import_job(job_id, "downloading", 0, None)
            .await?;

        // Start HTTP request
        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .send()
            .await
            .context("Failed to start HTTP request")?;

        if !response.status().is_success() {
            return Err(anyhow!("HTTP request failed: {}", response.status()));
        }

        // Get content length
        let content_length = response.content_length();
        let file_size = size_bytes.or(content_length).ok_or_else(|| {
            anyhow!("size_bytes required for raw URL import without Content-Length")
        })?;

        // Update state to writing
        store.update_import_job(job_id, "writing", 0, None).await?;

        // Create ZVOL
        let vol = zfs.create_volume(volume_name, file_size, None).await?;

        // Open ZVOL device
        let mut zvol = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&vol.device_path)
            .await
            .context("Failed to open ZVOL device")?;

        // Stream response body to ZVOL
        let mut stream = response.bytes_stream();
        let mut bytes_written: u64 = 0;
        let mut last_update = std::time::Instant::now();

        use futures_util::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            // Check for cancellation
            if cancel_rx.try_recv().is_ok() {
                let _ = zfs.delete_volume(volume_name).await;
                store
                    .update_import_job(job_id, "cancelled", bytes_written, None)
                    .await?;
                return Ok(());
            }

            let chunk = chunk_result.context("Failed to read HTTP chunk")?;
            zvol.write_all(&chunk).await?;
            bytes_written += chunk.len() as u64;

            // Update progress every second
            if last_update.elapsed().as_secs() >= 1 {
                store
                    .update_import_job(job_id, "writing", bytes_written, None)
                    .await?;
                last_update = std::time::Instant::now();
            }
        }

        zvol.flush().await?;

        // Store volume in database
        let vol_entry = VolumeEntry::new(
            volume_name.to_string(),
            zfs.volume_zfs_path(volume_name),
            vol.device_path.clone(),
            file_size,
            Some("import".to_string()),
            Some(url.to_string()),
        );
        store.create_volume(&vol_entry).await?;

        // Mark completed
        store
            .update_import_job(job_id, "completed", bytes_written, None)
            .await?;

        info!(
            job_id = %job_id,
            volume = %volume_name,
            bytes = %bytes_written,
            "HTTP raw import completed"
        );

        Ok(())
    }

    /// Import qcow2 file using qemu-img convert
    async fn import_qcow2_file(
        job_id: &str,
        volume_name: &str,
        path: &str,
        store: &Store,
        zfs: &ZfsManager,
        _cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        use tokio::process::Command;

        // Update state to converting
        store
            .update_import_job(job_id, "converting", 0, None)
            .await?;

        // Get virtual size from qcow2 using qemu-img info
        let output = Command::new("qemu-img")
            .args(["info", "--output=json", path])
            .output()
            .await
            .context("Failed to run qemu-img info")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("qemu-img info failed: {}", stderr));
        }

        // Parse JSON output to get virtual-size
        let info: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("Failed to parse qemu-img info output")?;
        let virtual_size = info["virtual-size"]
            .as_u64()
            .ok_or_else(|| anyhow!("Failed to get virtual-size from qemu-img info"))?;

        info!(
            job_id = %job_id,
            virtual_size = %virtual_size,
            "qcow2 virtual size determined, creating ZVOL"
        );

        // Create ZVOL
        let vol = zfs.create_volume(volume_name, virtual_size, None).await?;

        // Update state to writing
        store.update_import_job(job_id, "writing", 0, None).await?;

        // Convert qcow2 to raw directly to ZVOL using qemu-img convert
        info!(
            job_id = %job_id,
            source = %path,
            target = %vol.device_path,
            "Converting qcow2 to ZVOL"
        );

        let output = Command::new("qemu-img")
            .args([
                "convert",
                "-f",
                "qcow2",
                "-O",
                "raw",
                "-p", // Show progress (goes to stderr)
                path,
                &vol.device_path,
            ])
            .output()
            .await
            .context("Failed to run qemu-img convert")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("qemu-img convert failed: {}", stderr));
        }

        // Store volume in database
        let vol_entry = VolumeEntry::new(
            volume_name.to_string(),
            zfs.volume_zfs_path(volume_name),
            vol.device_path.clone(),
            virtual_size,
            Some("import".to_string()),
            Some(path.to_string()),
        );
        store.create_volume(&vol_entry).await?;

        // Mark completed
        store
            .update_import_job(job_id, "completed", virtual_size, None)
            .await?;

        info!(
            job_id = %job_id,
            volume = %volume_name,
            bytes = %virtual_size,
            "qcow2 import completed"
        );

        Ok(())
    }

    /// Import qcow2 from URL (download first, then convert)
    async fn import_qcow2_url(
        job_id: &str,
        volume_name: &str,
        url: &str,
        store: &Store,
        zfs: &ZfsManager,
        pool_mountpoint: &str,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        // Create temp directory on ZFS pool
        let tmp_dir = format!("{}/.tmp", pool_mountpoint);
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .context("Failed to create temp directory")?;

        let tmp_file = format!("{}/import-{}.qcow2", tmp_dir, Uuid::new_v4());

        // Run download and conversion, ensuring temp file cleanup on any error
        let result = Self::download_and_convert_qcow2(
            job_id,
            volume_name,
            url,
            &tmp_file,
            store,
            zfs,
            cancel_rx,
        )
        .await;

        // Always clean up temp file, regardless of success or failure
        if tokio::fs::metadata(&tmp_file).await.is_ok()
            && let Err(e) = tokio::fs::remove_file(&tmp_file).await
        {
            warn!(path = %tmp_file, error = %e, "Failed to remove temp file");
        }

        result
    }

    /// Download qcow2 and convert - helper for import_qcow2_url
    #[allow(clippy::too_many_arguments)]
    async fn download_and_convert_qcow2(
        job_id: &str,
        volume_name: &str,
        url: &str,
        tmp_file: &str,
        store: &Store,
        zfs: &ZfsManager,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        // Update state to downloading
        store
            .update_import_job(job_id, "downloading", 0, None)
            .await?;

        // Download to temp file
        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .send()
            .await
            .context("Failed to start HTTP request")?;

        if !response.status().is_success() {
            return Err(anyhow!("HTTP request failed: {}", response.status()));
        }

        let mut file = tokio::fs::File::create(tmp_file)
            .await
            .context("Failed to create temp file")?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded: u64 = 0;
        let mut last_update = std::time::Instant::now();

        use futures_util::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            if cancel_rx.try_recv().is_ok() {
                store
                    .update_import_job(job_id, "cancelled", bytes_downloaded, None)
                    .await?;
                return Ok(());
            }

            let chunk = chunk_result.context("Failed to read HTTP chunk")?;
            file.write_all(&chunk).await?;
            bytes_downloaded += chunk.len() as u64;

            if last_update.elapsed().as_secs() >= 1 {
                store
                    .update_import_job(job_id, "downloading", bytes_downloaded, None)
                    .await?;
                last_update = std::time::Instant::now();
            }
        }

        file.flush().await?;
        drop(file);

        info!(
            job_id = %job_id,
            bytes = %bytes_downloaded,
            "qcow2 download completed, converting..."
        );

        // Now convert the downloaded qcow2 file
        Self::import_qcow2_file(job_id, volume_name, tmp_file, store, zfs, cancel_rx).await
    }
}
