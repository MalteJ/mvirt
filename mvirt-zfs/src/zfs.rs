//! ZFS operations module
//!
//! Uses libzfs for reading pool/dataset info and shell commands for write operations.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;
use tracing::info;

/// Manager for ZFS pool and volume operations
pub struct ZfsManager {
    pool_name: String,
}

impl ZfsManager {
    pub fn new(pool_name: String) -> Self {
        Self { pool_name }
    }

    #[allow(dead_code)]
    pub fn pool_name(&self) -> &str {
        &self.pool_name
    }

    /// Get the device path for a volume
    #[allow(dead_code)]
    pub fn volume_device_path(&self, name: &str) -> String {
        format!("/dev/zvol/{}/{}", self.pool_name, name)
    }

    /// Get the ZFS path for a volume (by UUID)
    pub fn volume_zfs_path(&self, uuid: &str) -> String {
        format!("{}/{}", self.pool_name, uuid)
    }

    /// Get the device path for a volume (by UUID)
    pub fn volume_device_path_by_uuid(&self, uuid: &str) -> String {
        format!("/dev/zvol/{}/{}", self.pool_name, uuid)
    }

    /// Get the ZFS path for a base ZVOL (template storage)
    pub fn base_zvol_path(&self, uuid: &str) -> String {
        format!("{}/.base/{}", self.pool_name, uuid)
    }

    /// Get the device path for a base ZVOL
    pub fn base_device_path(&self, uuid: &str) -> String {
        format!("/dev/zvol/{}/.base/{}", self.pool_name, uuid)
    }

    /// Get the snapshot path for a template (base@img)
    pub fn template_snapshot_path(&self, uuid: &str) -> String {
        format!("{}/.base/{}@img", self.pool_name, uuid)
    }

    // === Pool Operations ===

    /// Ensure the .tmp dataset exists for temporary files during import
    pub async fn ensure_tmp_dataset(&self, mountpoint: &str) -> Result<()> {
        let dataset = format!("{}/.tmp", self.pool_name);

        // Check if dataset exists
        let output = Command::new("zfs")
            .args(["list", "-H", &dataset])
            .output()
            .await
            .context("Failed to run zfs list")?;

        if output.status.success() {
            info!(dataset = %dataset, "Temp dataset already exists");
            return Ok(());
        }

        // Create dataset with mountpoint
        info!(dataset = %dataset, mountpoint = %mountpoint, "Creating temp dataset");
        let output = Command::new("zfs")
            .args([
                "create",
                "-o",
                &format!("mountpoint={}", mountpoint),
                &dataset,
            ])
            .output()
            .await
            .context("Failed to run zfs create")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs create failed: {}", stderr));
        }

        Ok(())
    }

    /// Destroy the .tmp dataset on shutdown
    pub async fn destroy_tmp_dataset(&self) {
        let dataset = format!("{}/.tmp", self.pool_name);

        info!(dataset = %dataset, "Destroying temp dataset");
        let output = Command::new("zfs")
            .args(["destroy", "-r", &dataset])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                info!(dataset = %dataset, "Temp dataset destroyed");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::warn!(dataset = %dataset, error = %stderr, "Failed to destroy temp dataset");
            }
            Err(e) => {
                tracing::warn!(dataset = %dataset, error = %e, "Failed to run zfs destroy");
            }
        }
    }

    /// Get pool statistics
    pub async fn get_pool_stats(&self) -> Result<PoolStats> {
        // Use zpool list for basic stats
        let output = Command::new("zpool")
            .args(["list", "-Hp", "-o", "name,size,alloc,free", &self.pool_name])
            .output()
            .await
            .context("Failed to run zpool list")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zpool list failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split('\t').collect();

        if parts.len() < 4 {
            return Err(anyhow!("Unexpected zpool list output: {}", stdout));
        }

        let total_bytes: u64 = parts[1].parse().unwrap_or(0);
        let used_bytes: u64 = parts[2].parse().unwrap_or(0);
        let available_bytes: u64 = parts[3].parse().unwrap_or(0);

        // Get provisioned bytes (sum of all volsize)
        let provisioned_bytes = self.get_total_provisioned().await.unwrap_or(0);

        // Get compression ratio
        let compression_ratio = self.get_pool_compression_ratio().await.unwrap_or(1.0);

        Ok(PoolStats {
            name: self.pool_name.clone(),
            total_bytes,
            available_bytes,
            used_bytes,
            provisioned_bytes,
            compression_ratio,
        })
    }

    async fn get_total_provisioned(&self) -> Result<u64> {
        let output = Command::new("zfs")
            .args([
                "list",
                "-Hp",
                "-t",
                "volume",
                "-o",
                "volsize",
                "-r",
                &self.pool_name,
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let total: u64 = stdout
            .lines()
            .filter_map(|line| line.trim().parse::<u64>().ok())
            .sum();

        Ok(total)
    }

    async fn get_pool_compression_ratio(&self) -> Result<f64> {
        // Get compressratio from the pool's root dataset
        let output = Command::new("zfs")
            .args([
                "get",
                "-Hp",
                "-o",
                "value",
                "compressratio",
                &self.pool_name,
            ])
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let ratio_str = stdout.trim().trim_end_matches('x');
            if let Ok(ratio) = ratio_str.parse::<f64>() {
                return Ok(ratio);
            }
        }

        Ok(1.0)
    }

    // === Volume Operations ===

    /// Create a new sparse (thin-provisioned) ZVOL
    pub async fn create_volume(
        &self,
        name: &str,
        size_bytes: u64,
        volblocksize: Option<u32>,
    ) -> Result<VolumeInfo> {
        let zfs_path = self.volume_zfs_path(name);
        let size_str = size_bytes.to_string();

        let mut args = vec!["create", "-s", "-V", &size_str];

        let blocksize_str;
        if let Some(bs) = volblocksize {
            blocksize_str = format!("{}", bs);
            args.push("-b");
            args.push(&blocksize_str);
        }

        args.push(&zfs_path);

        info!(name = %name, size_bytes = %size_bytes, "Creating ZVOL");

        let output = Command::new("zfs")
            .args(&args)
            .output()
            .await
            .context("Failed to run zfs create")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs create failed: {}", stderr));
        }

        let vol = self.get_volume(name).await?;

        // Wait for udev to create the device node
        Self::wait_for_device(&vol.device_path).await?;

        Ok(vol)
    }

    /// Wait for a device node to appear (udev creates it asynchronously)
    async fn wait_for_device(device_path: &str) -> Result<()> {
        let timeout = Duration::from_secs(10);
        let poll_interval = Duration::from_millis(50);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if tokio::fs::metadata(device_path).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(poll_interval).await;
        }

        Err(anyhow!(
            "Timeout waiting for device {} to appear",
            device_path
        ))
    }

    /// List all volumes in the pool
    #[allow(dead_code)]
    pub async fn list_volumes(&self) -> Result<Vec<VolumeInfo>> {
        let output = Command::new("zfs")
            .args([
                "list",
                "-Hp",
                "-t",
                "volume",
                "-o",
                "name,volsize,used,volblocksize,compressratio,creation",
                "-r",
                &self.pool_name,
            ])
            .output()
            .await
            .context("Failed to run zfs list")?;

        if !output.status.success() {
            // Empty pool returns error, treat as empty list
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut volumes = Vec::new();

        for line in stdout.lines() {
            if let Some(vol) = self.parse_volume_line(line) {
                volumes.push(vol);
            }
        }

        Ok(volumes)
    }

    /// Get a specific volume by name
    pub async fn get_volume(&self, name: &str) -> Result<VolumeInfo> {
        let zfs_path = self.volume_zfs_path(name);

        let output = Command::new("zfs")
            .args([
                "list",
                "-Hp",
                "-t",
                "volume",
                "-o",
                "name,volsize,used,volblocksize,compressratio,creation",
                &zfs_path,
            ])
            .output()
            .await
            .context("Failed to run zfs list")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Volume not found: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_volume_line(stdout.trim())
            .ok_or_else(|| anyhow!("Failed to parse volume info"))
    }

    /// Delete a volume
    pub async fn delete_volume(&self, name: &str) -> Result<()> {
        let zfs_path = self.volume_zfs_path(name);

        info!(name = %name, "Deleting ZVOL");

        let output = Command::new("zfs")
            .args(["destroy", &zfs_path])
            .output()
            .await
            .context("Failed to run zfs destroy")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs destroy failed: {}", stderr));
        }

        Ok(())
    }

    /// Resize a volume (can only grow, not shrink)
    pub async fn resize_volume(&self, name: &str, new_size_bytes: u64) -> Result<VolumeInfo> {
        let zfs_path = self.volume_zfs_path(name);
        let size_str = new_size_bytes.to_string();

        info!(name = %name, new_size_bytes = %new_size_bytes, "Resizing ZVOL");

        let output = Command::new("zfs")
            .args(["set", &format!("volsize={}", size_str), &zfs_path])
            .output()
            .await
            .context("Failed to run zfs set volsize")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs set volsize failed: {}", stderr));
        }

        self.get_volume(name).await
    }

    /// Delete a volume and all its snapshots
    pub async fn delete_volume_recursive(&self, uuid: &str) -> Result<()> {
        let zfs_path = self.volume_zfs_path(uuid);

        info!(uuid = %uuid, "Deleting ZVOL recursively");

        let output = Command::new("zfs")
            .args(["destroy", "-r", &zfs_path])
            .output()
            .await
            .context("Failed to run zfs destroy -r")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs destroy -r failed: {}", stderr));
        }

        Ok(())
    }

    // === Template/Base ZVOL Operations ===

    /// Ensure the .base dataset exists for storing template base ZVOLs
    pub async fn ensure_base_dataset(&self) -> Result<()> {
        let base_path = format!("{}/.base", self.pool_name);

        // Check if it exists
        let output = Command::new("zfs")
            .args(["list", "-H", &base_path])
            .output()
            .await?;

        if output.status.success() {
            return Ok(()); // Already exists
        }

        // Create it
        info!(path = %base_path, "Creating .base dataset");

        let output = Command::new("zfs")
            .args(["create", &base_path])
            .output()
            .await
            .context("Failed to create .base dataset")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs create .base failed: {}", stderr));
        }

        Ok(())
    }

    /// Create a base ZVOL for a template (at vmpool/.base/<uuid>)
    pub async fn create_base_zvol(&self, uuid: &str, size_bytes: u64) -> Result<String> {
        self.ensure_base_dataset().await?;

        let zfs_path = self.base_zvol_path(uuid);
        let device_path = self.base_device_path(uuid);
        let size_str = size_bytes.to_string();

        info!(uuid = %uuid, size_bytes = %size_bytes, "Creating base ZVOL");

        let output = Command::new("zfs")
            .args(["create", "-s", "-V", &size_str, &zfs_path])
            .output()
            .await
            .context("Failed to run zfs create for base ZVOL")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs create base ZVOL failed: {}", stderr));
        }

        // Wait for device node
        Self::wait_for_device(&device_path).await?;

        Ok(device_path)
    }

    /// Create the @img snapshot for a template
    pub async fn create_template_snapshot(&self, uuid: &str) -> Result<String> {
        let snapshot_path = self.template_snapshot_path(uuid);

        info!(uuid = %uuid, "Creating template snapshot @img");

        let output = Command::new("zfs")
            .args(["snapshot", &snapshot_path])
            .output()
            .await
            .context("Failed to run zfs snapshot")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs snapshot failed: {}", stderr));
        }

        Ok(snapshot_path)
    }

    /// Clone a template snapshot to create a new volume
    pub async fn clone_to_volume(
        &self,
        template_uuid: &str,
        volume_uuid: &str,
    ) -> Result<VolumeInfo> {
        let snapshot_path = self.template_snapshot_path(template_uuid);
        let volume_path = self.volume_zfs_path(volume_uuid);

        info!(
            template_uuid = %template_uuid,
            volume_uuid = %volume_uuid,
            "Cloning template to volume"
        );

        let output = Command::new("zfs")
            .args(["clone", &snapshot_path, &volume_path])
            .output()
            .await
            .context("Failed to run zfs clone")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs clone failed: {}", stderr));
        }

        let vol = self.get_volume(volume_uuid).await?;

        // Wait for device node
        Self::wait_for_device(&vol.device_path).await?;

        Ok(vol)
    }

    /// Delete a base ZVOL (for garbage collection)
    pub async fn delete_base_zvol(&self, uuid: &str) -> Result<()> {
        let zfs_path = self.base_zvol_path(uuid);

        info!(uuid = %uuid, "Deleting base ZVOL (GC)");

        let output = Command::new("zfs")
            .args(["destroy", "-r", &zfs_path])
            .output()
            .await
            .context("Failed to run zfs destroy for base ZVOL")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs destroy base ZVOL failed: {}", stderr));
        }

        Ok(())
    }

    // === Snapshot Operations ===

    /// Create a snapshot
    pub async fn create_snapshot(
        &self,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<SnapshotInfo> {
        let snapshot_path = format!("{}@{}", self.volume_zfs_path(volume_name), snapshot_name);

        info!(volume = %volume_name, snapshot = %snapshot_name, "Creating snapshot");

        let output = Command::new("zfs")
            .args(["snapshot", &snapshot_path])
            .output()
            .await
            .context("Failed to run zfs snapshot")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs snapshot failed: {}", stderr));
        }

        self.get_snapshot(volume_name, snapshot_name).await
    }

    /// List snapshots for a volume
    pub async fn list_snapshots(&self, volume_name: &str) -> Result<Vec<SnapshotInfo>> {
        let zfs_path = self.volume_zfs_path(volume_name);

        let output = Command::new("zfs")
            .args([
                "list",
                "-Hp",
                "-t",
                "snapshot",
                "-o",
                "name,used,creation",
                "-r",
                &zfs_path,
            ])
            .output()
            .await
            .context("Failed to run zfs list snapshots")?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut snapshots = Vec::new();

        for line in stdout.lines() {
            if let Some(snap) = self.parse_snapshot_line(line, volume_name) {
                snapshots.push(snap);
            }
        }

        Ok(snapshots)
    }

    /// Get a specific snapshot
    pub async fn get_snapshot(
        &self,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<SnapshotInfo> {
        let snapshot_path = format!("{}@{}", self.volume_zfs_path(volume_name), snapshot_name);

        let output = Command::new("zfs")
            .args([
                "list",
                "-Hp",
                "-t",
                "snapshot",
                "-o",
                "name,used,creation",
                &snapshot_path,
            ])
            .output()
            .await
            .context("Failed to run zfs list snapshot")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Snapshot not found: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_snapshot_line(stdout.trim(), volume_name)
            .ok_or_else(|| anyhow!("Failed to parse snapshot info"))
    }

    /// Delete a snapshot
    pub async fn delete_snapshot(&self, volume_name: &str, snapshot_name: &str) -> Result<()> {
        let snapshot_path = format!("{}@{}", self.volume_zfs_path(volume_name), snapshot_name);

        info!(volume = %volume_name, snapshot = %snapshot_name, "Deleting snapshot");

        let output = Command::new("zfs")
            .args(["destroy", &snapshot_path])
            .output()
            .await
            .context("Failed to run zfs destroy snapshot")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs destroy snapshot failed: {}", stderr));
        }

        Ok(())
    }

    /// Rollback to a snapshot (volume must not be in use!)
    pub async fn rollback_snapshot(
        &self,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<VolumeInfo> {
        let snapshot_path = format!("{}@{}", self.volume_zfs_path(volume_name), snapshot_name);

        info!(volume = %volume_name, snapshot = %snapshot_name, "Rolling back to snapshot");

        let output = Command::new("zfs")
            .args(["rollback", &snapshot_path])
            .output()
            .await
            .context("Failed to run zfs rollback")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs rollback failed: {}", stderr));
        }

        self.get_volume(volume_name).await
    }

    // === Clone Operations ===

    /// Clone a snapshot to a new ZFS dataset (for base ZVOLs)
    /// Use clone_to_volume for cloning templates to regular volumes.
    pub async fn clone_snapshot(&self, snapshot_path: &str, target_path: &str) -> Result<()> {
        info!(snapshot = %snapshot_path, target = %target_path, "Cloning snapshot");

        let output = Command::new("zfs")
            .args(["clone", snapshot_path, target_path])
            .output()
            .await
            .context("Failed to run zfs clone")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs clone failed: {}", stderr));
        }

        Ok(())
    }

    // === Helper Methods ===

    fn parse_volume_line(&self, line: &str) -> Option<VolumeInfo> {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 6 {
            return None;
        }

        let full_name = parts[0];
        let name = full_name
            .strip_prefix(&format!("{}/", self.pool_name))
            .unwrap_or(full_name)
            .to_string();

        // Skip nested datasets (templates are handled separately)
        if name.contains('/') {
            return None;
        }

        let volsize_bytes: u64 = parts[1].parse().ok()?;
        let used_bytes: u64 = parts[2].parse().ok()?;
        let volblocksize: u64 = parts[3].parse().ok()?;
        let compression_ratio = parts[4].trim_end_matches('x').parse().unwrap_or(1.0);
        let creation_timestamp: i64 = parts[5].parse().ok()?;

        Some(VolumeInfo {
            name,
            zfs_path: full_name.to_string(),
            device_path: format!("/dev/zvol/{}", full_name),
            volsize_bytes,
            used_bytes,
            volblocksize,
            compression_ratio,
            creation_timestamp,
        })
    }

    fn parse_snapshot_line(&self, line: &str, volume_name: &str) -> Option<SnapshotInfo> {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            return None;
        }

        let full_name = parts[0];

        // Extract snapshot name from full path (pool/volume@snapshot)
        let snap_name = full_name.split('@').nth(1)?.to_string();
        let used_bytes: u64 = parts[1].parse().ok()?;
        let creation_timestamp: i64 = parts[2].parse().ok()?;

        Some(SnapshotInfo {
            name: snap_name,
            full_name: full_name.to_string(),
            volume_name: volume_name.to_string(),
            used_bytes,
            creation_timestamp,
        })
    }
}

// === Data Types ===

#[derive(Debug, Clone)]
pub struct PoolStats {
    pub name: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub provisioned_bytes: u64,
    pub compression_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub zfs_path: String,
    pub device_path: String,
    pub volsize_bytes: u64,
    pub used_bytes: u64,
    pub volblocksize: u64,
    pub compression_ratio: f64,
    #[allow(dead_code)]
    pub creation_timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub name: String,
    pub full_name: String,
    #[allow(dead_code)]
    pub volume_name: String,
    pub used_bytes: u64,
    pub creation_timestamp: i64,
}
