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

use crate::audit::ZfsAuditLogger;
use crate::store::{ImportJobEntry, Store, TemplateEntry};
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
    state_dir: String,
    store: Arc<Store>,
    zfs: Arc<ZfsManager>,
    audit: Arc<ZfsAuditLogger>,
    running_jobs: Arc<RwLock<HashMap<String, RunningJob>>>,
}

impl ImportManager {
    pub fn new(
        pool_name: String,
        state_dir: String,
        store: Arc<Store>,
        zfs: Arc<ZfsManager>,
        audit: Arc<ZfsAuditLogger>,
    ) -> Self {
        Self {
            pool_name,
            state_dir,
            store,
            zfs,
            audit,
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

    /// Detect image format from magic bytes (first 4 bytes)
    pub fn detect_format_from_magic(bytes: &[u8]) -> ImageFormat {
        // qcow2 magic: "QFI\xfb" (0x514649fb big-endian)
        if bytes.len() >= 4 && bytes[..4] == [0x51, 0x46, 0x49, 0xfb] {
            ImageFormat::Qcow2
        } else {
            ImageFormat::Raw
        }
    }

    /// Start an import job (creates a template)
    pub async fn start_import(
        &self,
        template_name: String,
        source: ImportSource,
        size_bytes: Option<u64>,
    ) -> Result<ImportJobEntry> {
        // For local files, detect format upfront. For URLs, detect during download.
        let format = match &source {
            ImportSource::LocalFile(path) => Some(Self::detect_format_from_file(path).await?),
            ImportSource::HttpUrl(_) => None, // Detect from first bytes during download
        };

        let format_str = match format {
            Some(ImageFormat::Raw) => "raw",
            Some(ImageFormat::Qcow2) => "qcow2",
            None => "auto",
        };

        // Create job entry
        let job_entry = ImportJobEntry::new(
            template_name.clone(),
            source.as_str().to_string(),
            format_str.to_string(),
            size_bytes,
        );

        // Store in database
        self.store.create_import_job(&job_entry).await?;

        info!(
            job_id = %job_entry.id,
            template = %template_name,
            source = %source.as_str(),
            format = %format_str,
            "Starting import job"
        );

        // Audit log: import started
        self.audit
            .import_started(&job_entry.id, &template_name, source.as_str())
            .await;

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
        let audit = Arc::clone(&self.audit);
        let state_dir = self.state_dir.clone();
        let running_jobs = Arc::clone(&self.running_jobs);

        tokio::spawn(async move {
            let result = Self::run_import(
                &job_id,
                &template_name,
                source,
                format,
                size_bytes,
                &store,
                &zfs,
                &audit,
                &state_dir,
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
    /// Returns the template_id on success for cleanup purposes
    #[allow(clippy::too_many_arguments)]
    async fn run_import(
        job_id: &str,
        template_name: &str,
        source: ImportSource,
        format: Option<ImageFormat>,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        audit: &ZfsAuditLogger,
        state_dir: &str,
        mut cancel_rx: oneshot::Receiver<()>,
    ) -> Result<()> {
        // Generate template UUID upfront for cleanup tracking
        let template_id = Uuid::new_v4().to_string();

        let result = Self::run_import_inner(
            job_id,
            &template_id,
            template_name,
            source,
            format,
            size_bytes,
            store,
            zfs,
            state_dir,
            &mut cancel_rx,
        )
        .await;

        // Handle success: log completion
        if result.is_ok() {
            audit
                .import_completed(job_id, &template_id, template_name)
                .await;
        }

        // Handle errors centrally: update job state and cleanup
        if let Err(ref e) = result {
            let error_msg = e.to_string();
            error!(job_id = %job_id, error = %error_msg, "Import failed");

            // Audit log: import failed
            audit.import_failed(job_id, template_name, &error_msg).await;

            // Try to clean up the base ZVOL if it was created
            if let Err(cleanup_err) = zfs.delete_base_zvol(&template_id).await {
                // Base ZVOL might not exist yet, that's fine
                warn!(
                    job_id = %job_id,
                    template_id = %template_id,
                    error = %cleanup_err,
                    "Failed to cleanup base ZVOL after import error (may not exist)"
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
        template_id: &str,
        template_name: &str,
        source: ImportSource,
        format: Option<ImageFormat>,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        state_dir: &str,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        match (format, source) {
            // Local files: format is known
            (Some(ImageFormat::Raw), ImportSource::LocalFile(path)) => {
                Self::import_raw_file(
                    job_id,
                    template_id,
                    template_name,
                    &path,
                    size_bytes,
                    store,
                    zfs,
                    cancel_rx,
                )
                .await
            }
            (Some(ImageFormat::Qcow2), ImportSource::LocalFile(path)) => {
                Self::import_qcow2_file(
                    job_id,
                    template_id,
                    template_name,
                    &path,
                    store,
                    zfs,
                    cancel_rx,
                )
                .await
            }
            // HTTP URLs: detect format from first bytes during download
            (None, ImportSource::HttpUrl(url)) => {
                Self::import_from_url(
                    job_id,
                    template_id,
                    template_name,
                    &url,
                    size_bytes,
                    store,
                    zfs,
                    state_dir,
                    cancel_rx,
                )
                .await
            }
            // Should not happen: local file without format or URL with format
            (None, ImportSource::LocalFile(_)) => {
                Err(anyhow!("Local file format should be detected upfront"))
            }
            (Some(_), ImportSource::HttpUrl(_)) => {
                Err(anyhow!("URL format should be detected during download"))
            }
        }
    }

    /// Import raw file to template
    #[allow(clippy::too_many_arguments)]
    async fn import_raw_file(
        job_id: &str,
        template_id: &str,
        template_name: &str,
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

        // Create base ZVOL for template
        let device_path = zfs.create_base_zvol(template_id, file_size).await?;

        // Open source file
        let mut src_file = File::open(path)
            .await
            .context("Failed to open source file")?;

        // Open base ZVOL device
        let mut zvol = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&device_path)
            .await
            .context("Failed to open ZVOL device")?;

        // Stream data
        let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer
        let mut bytes_written: u64 = 0;
        let mut last_update = std::time::Instant::now();

        loop {
            // Check for cancellation
            if cancel_rx.try_recv().is_ok() {
                // Clean up: delete the base ZVOL
                let _ = zfs.delete_base_zvol(template_id).await;
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
        drop(zvol); // Close before creating snapshot

        // Create template snapshot
        let snapshot_path = zfs.create_template_snapshot(template_id).await?;

        // Store template in database
        let template_entry = TemplateEntry::new(
            template_id.to_string(),
            template_name.to_string(),
            zfs.base_zvol_path(template_id),
            snapshot_path,
            file_size,
        );
        store.create_template(&template_entry).await?;

        // Mark completed
        store
            .update_import_job(job_id, "completed", bytes_written, None)
            .await?;

        info!(
            job_id = %job_id,
            template = %template_name,
            template_id = %template_id,
            bytes = %bytes_written,
            "Raw file import completed"
        );

        Ok(())
    }

    /// Import qcow2 file using qemu-img convert to template
    async fn import_qcow2_file(
        job_id: &str,
        template_id: &str,
        template_name: &str,
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
            "qcow2 virtual size determined, creating base ZVOL"
        );

        // Create base ZVOL for template
        let device_path = zfs.create_base_zvol(template_id, virtual_size).await?;

        // Update state to writing
        store.update_import_job(job_id, "writing", 0, None).await?;

        // Convert qcow2 to raw directly to base ZVOL using qemu-img convert
        info!(
            job_id = %job_id,
            source = %path,
            target = %device_path,
            "Converting qcow2 to base ZVOL"
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
                &device_path,
            ])
            .output()
            .await
            .context("Failed to run qemu-img convert")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("qemu-img convert failed: {}", stderr));
        }

        // Create template snapshot
        let snapshot_path = zfs.create_template_snapshot(template_id).await?;

        // Store template in database
        let template_entry = TemplateEntry::new(
            template_id.to_string(),
            template_name.to_string(),
            zfs.base_zvol_path(template_id),
            snapshot_path,
            virtual_size,
        );
        store.create_template(&template_entry).await?;

        // Mark completed
        store
            .update_import_job(job_id, "completed", virtual_size, None)
            .await?;

        info!(
            job_id = %job_id,
            template = %template_name,
            template_id = %template_id,
            bytes = %virtual_size,
            "qcow2 import completed"
        );

        Ok(())
    }

    /// Import from URL with auto-detection of format from first bytes
    /// Downloads to temp file, detects format, then processes accordingly
    #[allow(clippy::too_many_arguments)]
    async fn import_from_url(
        job_id: &str,
        template_id: &str,
        template_name: &str,
        url: &str,
        size_bytes: Option<u64>,
        store: &Store,
        zfs: &ZfsManager,
        state_dir: &str,
        cancel_rx: &mut oneshot::Receiver<()>,
    ) -> Result<()> {
        use futures_util::StreamExt;

        // Update state to downloading
        store
            .update_import_job(job_id, "downloading", 0, None)
            .await?;

        // Create temp directory and file
        let tmp_dir = format!("{}/tmp", state_dir);
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .context("Failed to create temp directory")?;
        let tmp_file = format!("{}/import-{}.tmp", tmp_dir, Uuid::new_v4());

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

        let content_length = response.content_length();

        // Download to temp file, capturing first 4 bytes for format detection
        let mut file = tokio::fs::File::create(&tmp_file)
            .await
            .context("Failed to create temp file")?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded: u64 = 0;
        let mut last_update = std::time::Instant::now();
        let mut magic_bytes = [0u8; 4];
        let mut magic_captured = false;

        while let Some(chunk_result) = stream.next().await {
            if cancel_rx.try_recv().is_ok() {
                let _ = tokio::fs::remove_file(&tmp_file).await;
                store
                    .update_import_job(job_id, "cancelled", bytes_downloaded, None)
                    .await?;
                return Ok(());
            }

            let chunk = chunk_result.context("Failed to read HTTP chunk")?;

            // Capture first 4 bytes for format detection
            if !magic_captured && bytes_downloaded == 0 && chunk.len() >= 4 {
                magic_bytes.copy_from_slice(&chunk[..4]);
                magic_captured = true;
            }

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

        // Detect format from magic bytes
        let format = Self::detect_format_from_magic(&magic_bytes);

        info!(
            job_id = %job_id,
            bytes = %bytes_downloaded,
            format = ?format,
            "Download completed, detected format"
        );

        // Process based on detected format
        let result = match format {
            ImageFormat::Qcow2 => {
                Self::import_qcow2_file(
                    job_id,
                    template_id,
                    template_name,
                    &tmp_file,
                    store,
                    zfs,
                    cancel_rx,
                )
                .await
            }
            ImageFormat::Raw => {
                // For raw, copy temp file to ZVOL
                let file_size = size_bytes.or(content_length).unwrap_or(bytes_downloaded);

                store.update_import_job(job_id, "writing", 0, None).await?;

                let device_path = zfs.create_base_zvol(template_id, file_size).await?;

                let mut src = File::open(&tmp_file)
                    .await
                    .context("Failed to open temp file")?;
                let mut zvol = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&device_path)
                    .await
                    .context("Failed to open ZVOL device")?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let mut bytes_written: u64 = 0;

                loop {
                    let n = src.read(&mut buffer).await?;
                    if n == 0 {
                        break;
                    }
                    zvol.write_all(&buffer[..n]).await?;
                    bytes_written += n as u64;
                }

                zvol.flush().await?;
                drop(zvol);

                let snapshot_path = zfs.create_template_snapshot(template_id).await?;

                let template_entry = TemplateEntry::new(
                    template_id.to_string(),
                    template_name.to_string(),
                    zfs.base_zvol_path(template_id),
                    snapshot_path,
                    file_size,
                );
                store.create_template(&template_entry).await?;

                store
                    .update_import_job(job_id, "completed", bytes_written, None)
                    .await?;

                info!(
                    job_id = %job_id,
                    template = %template_name,
                    "Raw URL import completed"
                );

                Ok(())
            }
        };

        // Clean up temp file
        let _ = tokio::fs::remove_file(&tmp_file).await;

        result
    }
}
