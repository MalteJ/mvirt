//! ZFS operations module
//!
//! Uses libzfs for reading pool/dataset info and shell commands for write operations.

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

    /// Get the ZFS path for a volume
    pub fn volume_zfs_path(&self, name: &str) -> String {
        format!("{}/{}", self.pool_name, name)
    }

    // === Pool Operations ===

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

        self.get_volume(name).await
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

    /// Clone a snapshot to create a new volume
    pub async fn clone_snapshot(
        &self,
        snapshot_path: &str,
        new_volume_name: &str,
    ) -> Result<VolumeInfo> {
        let new_zfs_path = self.volume_zfs_path(new_volume_name);

        info!(snapshot = %snapshot_path, new_volume = %new_volume_name, "Cloning snapshot");

        let output = Command::new("zfs")
            .args(["clone", snapshot_path, &new_zfs_path])
            .output()
            .await
            .context("Failed to run zfs clone")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("zfs clone failed: {}", stderr));
        }

        self.get_volume(new_volume_name).await
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
