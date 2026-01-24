//! Integration tests for mvirt-zfs storage lifecycle
//!
//! These tests require:
//! - A ZFS pool named "mvirt" (or set MVIRT_ZFS_POOL env var)
//! - Root/sudo permissions for ZFS operations
//! - Network access to download test image
//!
//! Run with: sudo -E cargo test --test lifecycle -- --test-threads=1

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

// Re-export the crate modules for testing
// We need to make these public in lib.rs or use a different approach

/// Test configuration
struct TestConfig {
    pool_name: String,
    state_dir: String,
    test_image_url: String,
    db_path: String,
}

impl TestConfig {
    fn new() -> Self {
        let pool_name = std::env::var("MVIRT_ZFS_POOL").unwrap_or_else(|_| "testpool".to_string());
        // Use target directory for state and database (pool is not mounted)
        let target_dir = env!("CARGO_MANIFEST_DIR").to_string() + "/../target";
        let state_dir = format!("{}/mvirt-zfs-test", target_dir);
        let db_dir = format!("{}/db", state_dir);
        Self {
            pool_name,
            state_dir,
            test_image_url: "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2".to_string(),
            db_path: db_dir,
        }
    }
}

impl TestConfig {
    /// Clean up state directory before/after tests
    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.state_dir);
    }
}

impl Drop for TestConfig {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Helper to run ZFS commands and check output
fn zfs_cmd(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("zfs").args(args).output()
}

/// Check if a ZFS dataset exists
fn zfs_exists(dataset: &str) -> bool {
    let output = zfs_cmd(&["list", "-H", dataset]);
    output.map(|o| o.status.success()).unwrap_or(false)
}

/// List ZFS datasets matching a pattern
fn zfs_list(pattern: &str) -> Vec<String> {
    let output = zfs_cmd(&["list", "-H", "-o", "name", "-r", pattern]);
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
        _ => vec![],
    }
}

/// Check if a ZFS snapshot exists
fn zfs_snapshot_exists(snapshot: &str) -> bool {
    let output = zfs_cmd(&["list", "-H", "-t", "snapshot", snapshot]);
    output.map(|o| o.status.success()).unwrap_or(false)
}

/// Get the origin of a ZFS clone
fn zfs_get_origin(dataset: &str) -> Option<String> {
    let output = zfs_cmd(&["get", "-H", "-o", "value", "origin", dataset]).ok()?;
    if output.status.success() {
        let origin = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if origin == "-" { None } else { Some(origin) }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full lifecycle test:
    /// 1. Import image → Template
    /// 2. Clone Template → Volume
    /// 3. Create Snapshot of Volume
    /// 4. Promote Snapshot → new Template
    /// 5. Delete Volume (with snapshots)
    /// 6. Delete Templates (with GC)
    #[tokio::test]
    async fn test_full_lifecycle() {
        // Skip if not root
        if !nix::unistd::Uid::effective().is_root() {
            eprintln!("Skipping test: requires root privileges");
            return;
        }

        let config = TestConfig::new();

        // Check pool exists
        if !zfs_exists(&config.pool_name) {
            eprintln!("Skipping test: pool '{}' does not exist", config.pool_name);
            return;
        }

        println!("=== Starting lifecycle test ===");
        println!("Pool: {}", config.pool_name);
        println!("Image: {}", config.test_image_url);

        // Clean up before starting (ensure fresh state)
        config.cleanup();
        std::fs::create_dir_all(&config.db_path).expect("Failed to create test db directory");

        // Initialize components
        let store = mvirt_zfs::store::Store::new(&config.db_path)
            .await
            .expect("Failed to create store");
        let zfs = mvirt_zfs::zfs::ZfsManager::new(config.pool_name.clone());
        let audit = mvirt_zfs::audit::ZfsAuditLogger::new_noop();

        let store = Arc::new(store);
        let zfs = Arc::new(zfs);
        let audit = Arc::new(audit);

        let import_manager = mvirt_zfs::import::ImportManager::new(
            config.pool_name.clone(),
            config.state_dir.clone(),
            Arc::clone(&store),
            Arc::clone(&zfs),
            Arc::clone(&audit),
        );

        // === Step 1: Import Template ===
        println!("\n--- Step 1: Import Template ---");

        let template_name = "test-debian-13";
        let source = mvirt_zfs::import::ImportSource::HttpUrl(config.test_image_url.clone());

        let job = import_manager
            .start_import(template_name.to_string(), source, None)
            .await
            .expect("Failed to start import");

        println!("Import job started: {}", job.id);

        // Wait for import to complete (with timeout)
        let timeout = Duration::from_secs(600); // 10 minutes for download
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                panic!("Import timed out after {:?}", timeout);
            }

            let job_state = import_manager
                .get_job(&job.id)
                .await
                .expect("Failed to get job")
                .expect("Job not found");

            println!(
                "  State: {}, Progress: {}/{}",
                job_state.state,
                job_state.bytes_written,
                job_state.total_bytes.unwrap_or(0)
            );

            match job_state.state.as_str() {
                "completed" => {
                    println!("Import completed!");
                    break;
                }
                "failed" => {
                    panic!("Import failed: {:?}", job_state.error);
                }
                _ => {
                    sleep(Duration::from_secs(2)).await;
                }
            }
        }

        // Verify template in database
        let template = store
            .get_template(template_name)
            .await
            .expect("DB error")
            .expect("Template not found in database");

        println!("Template in DB: id={}, name={}", template.id, template.name);

        // Verify base ZVOL exists
        let base_zvol = zfs.base_zvol_path(&template.id);
        assert!(
            zfs_exists(&base_zvol),
            "Base ZVOL should exist: {}",
            base_zvol
        );
        println!("Base ZVOL exists: {}", base_zvol);

        // Verify template snapshot exists
        let template_snap = format!("{}@img", base_zvol);
        assert!(
            zfs_snapshot_exists(&template_snap),
            "Template snapshot should exist: {}",
            template_snap
        );
        println!("Template snapshot exists: {}", template_snap);

        // === Step 2: Clone Template → Volume ===
        println!("\n--- Step 2: Clone Template → Volume ---");

        let volume_name = "test-vm-01";
        let volume_id = uuid::Uuid::new_v4().to_string();

        let vol_info = zfs
            .clone_to_volume(&template.id, &volume_id)
            .await
            .expect("Failed to clone template");

        // Store volume in database
        let volume_entry = mvirt_zfs::store::VolumeEntry::new(
            volume_id.clone(),
            volume_name.to_string(),
            zfs.volume_zfs_path(&volume_id),
            vol_info.device_path.clone(),
            template.size_bytes,
            Some(template.id.clone()),
        );

        store
            .create_volume(&volume_entry)
            .await
            .expect("Failed to store volume");

        println!(
            "Volume created: id={}, name={}, path={}",
            volume_entry.id, volume_entry.name, volume_entry.device_path
        );

        // Verify volume in database
        let vol_db = store
            .get_volume_by_name(volume_name)
            .await
            .expect("DB error")
            .expect("Volume not found in database");

        assert_eq!(vol_db.origin_template_id, Some(template.id.clone()));
        println!(
            "Volume in DB with origin_template_id: {:?}",
            vol_db.origin_template_id
        );

        // Verify volume ZVOL exists
        let vol_zvol = zfs.volume_zfs_path(&volume_id);
        assert!(
            zfs_exists(&vol_zvol),
            "Volume ZVOL should exist: {}",
            vol_zvol
        );
        println!("Volume ZVOL exists: {}", vol_zvol);

        // Verify origin is the template snapshot
        let origin = zfs_get_origin(&vol_zvol);
        assert_eq!(
            origin,
            Some(template_snap.clone()),
            "Volume origin should be template snapshot"
        );
        println!("Volume origin: {:?}", origin);

        // === Step 3: Create Snapshot of Volume ===
        println!("\n--- Step 3: Create Snapshot of Volume ---");

        let snapshot_name = "before-update";
        let zfs_snap_id = uuid::Uuid::new_v4().to_string();
        let mvirt_snap_id = uuid::Uuid::new_v4().to_string();

        let snap_info = zfs
            .create_snapshot(&volume_id, &zfs_snap_id)
            .await
            .expect("Failed to create snapshot");

        // First create ZFS snapshot entry
        let zfs_snapshot_entry = mvirt_zfs::store::ZfsSnapshotEntry::new(
            zfs_snap_id.clone(),
            volume_id.clone(),
            zfs_snap_id.clone(), // zfs_name is the UUID used in the ZFS path
        );
        store
            .create_zfs_snapshot(&zfs_snapshot_entry)
            .await
            .expect("Failed to store ZFS snapshot");

        // Then create mvirt snapshot entry referencing the ZFS snapshot
        let snapshot_entry = mvirt_zfs::store::SnapshotEntry::new(
            mvirt_snap_id.clone(),
            volume_id.clone(),
            snapshot_name.to_string(),
            zfs_snap_id.clone(), // zfs_snapshot_id references zfs_snapshots table
        );

        store
            .create_snapshot(&snapshot_entry)
            .await
            .expect("Failed to store snapshot");

        println!(
            "Snapshot created: id={}, name={}, full_name={}",
            snapshot_entry.id, snapshot_entry.name, snap_info.full_name
        );

        // Verify snapshot in database
        let snap_db = store
            .get_snapshot(&volume_id, snapshot_name)
            .await
            .expect("DB error")
            .expect("Snapshot not found in database");

        assert_eq!(snap_db.volume_id, volume_id);
        println!("Snapshot in DB for volume: {}", snap_db.volume_id);

        // Verify snapshot exists in ZFS
        let snap_full = format!("{}@{}", vol_zvol, zfs_snap_id);
        assert!(
            zfs_snapshot_exists(&snap_full),
            "Snapshot should exist: {}",
            snap_full
        );
        println!("Snapshot exists in ZFS: {}", snap_full);

        // === Step 4: Promote Snapshot → new Template ===
        println!("\n--- Step 4: Promote Snapshot → new Template ---");

        let new_template_name = "test-debian-13-updated";
        let new_template_id = uuid::Uuid::new_v4().to_string();

        // Clone snapshot to new base ZVOL
        let snap_path = format!("{}@{}", zfs.volume_zfs_path(&volume_id), zfs_snap_id);
        zfs.clone_snapshot(&snap_path, &zfs.base_zvol_path(&new_template_id))
            .await
            .expect("Failed to clone snapshot to base");

        // Create template snapshot
        let new_snap_path = zfs
            .create_template_snapshot(&new_template_id)
            .await
            .expect("Failed to create template snapshot");

        // Store new template
        let new_template_entry = mvirt_zfs::store::TemplateEntry::new_from_import(
            new_template_id.clone(),
            new_template_name.to_string(),
            zfs.base_zvol_path(&new_template_id),
            new_snap_path.clone(),
            template.size_bytes,
        );

        store
            .create_template(&new_template_entry)
            .await
            .expect("Failed to store new template");

        println!(
            "New template created: id={}, name={}",
            new_template_entry.id, new_template_entry.name
        );

        // Verify new template in database
        let new_tpl_db = store
            .get_template(new_template_name)
            .await
            .expect("DB error")
            .expect("New template not found in database");

        println!("New template in DB: {}", new_tpl_db.name);

        // Verify new base ZVOL and snapshot exist
        let new_base = zfs.base_zvol_path(&new_template_id);
        assert!(
            zfs_exists(&new_base),
            "New base ZVOL should exist: {}",
            new_base
        );

        let new_tpl_snap = format!("{}@img", new_base);
        assert!(
            zfs_snapshot_exists(&new_tpl_snap),
            "New template snapshot should exist: {}",
            new_tpl_snap
        );
        println!("New template base and snapshot exist");

        // === Step 5: Delete new Template first ===
        // (Because it depends on the volume's snapshot)
        println!("\n--- Step 5: Delete new Template (depends on volume snapshot) ---");

        store
            .delete_template(new_template_name)
            .await
            .expect("Failed to delete new template from database");

        // GC the new template's base ZVOL immediately since it has no dependent volumes
        let new_tpl_exists = store
            .template_exists(&new_template_id)
            .await
            .expect("DB error");
        let new_dep_count = store
            .count_volumes_by_origin(&new_template_id)
            .await
            .expect("DB error");

        assert!(!new_tpl_exists, "New template should not exist in DB");
        assert_eq!(
            new_dep_count, 0,
            "New template should have no dependent volumes"
        );

        zfs.delete_base_zvol(&new_template_id)
            .await
            .expect("Failed to GC new base ZVOL");

        assert!(
            !zfs_exists(&new_base),
            "New base ZVOL should be gone: {}",
            new_base
        );
        println!("New template base ZVOL garbage collected");

        // === Step 6: Delete Volume (with snapshots) ===
        println!("\n--- Step 6: Delete Volume (with snapshots) ---");

        let _origin_template_id = vol_db.origin_template_id.clone();

        // First delete all mvirt snapshots and their backing zfs_snapshots
        let snapshots = store
            .list_snapshots(&volume_id)
            .await
            .expect("Failed to list snapshots");
        for snap in &snapshots {
            // Delete mvirt snapshot
            store
                .delete_snapshot(&volume_id, &snap.name)
                .await
                .expect("Failed to delete snapshot");

            // Check ref count for the zfs_snapshot
            let ref_count = store
                .count_zfs_snapshot_refs(&snap.zfs_snapshot_id)
                .await
                .expect("Failed to count refs");

            // If no more references, delete the zfs_snapshot entry
            if ref_count == 0 {
                store
                    .delete_zfs_snapshot(&snap.zfs_snapshot_id)
                    .await
                    .expect("Failed to delete zfs_snapshot");
            }
            println!(
                "  Deleted snapshot: {} (refs remaining: {})",
                snap.name, ref_count
            );
        }

        // Delete from ZFS (recursive, includes snapshots)
        zfs.delete_volume_recursive(&volume_id)
            .await
            .expect("Failed to delete volume from ZFS");

        // Delete volume from database
        store
            .delete_volume(&volume_id)
            .await
            .expect("Failed to delete volume from database");

        println!("Volume deleted: {}", volume_name);

        // Verify volume gone from database
        let vol_gone = store
            .get_volume_by_name(volume_name)
            .await
            .expect("DB error");
        assert!(vol_gone.is_none(), "Volume should be gone from database");
        println!("Volume removed from database");

        // Verify snapshots gone from database
        let snaps_gone = store.list_snapshots(&volume_id).await.expect("DB error");
        assert!(
            snaps_gone.is_empty(),
            "Snapshots should be gone from database"
        );
        println!("Snapshots removed from database");

        // Verify volume ZVOL gone
        assert!(
            !zfs_exists(&vol_zvol),
            "Volume ZVOL should be gone: {}",
            vol_zvol
        );
        println!("Volume ZVOL removed from ZFS");

        // === Step 7: Delete original Template ===
        println!("\n--- Step 7: Delete original Template ---");

        // First, check how many volumes depend on the template
        let dependent_count = store
            .count_volumes_by_origin(&template.id)
            .await
            .expect("DB error");
        println!(
            "Volumes depending on original template: {}",
            dependent_count
        );

        // Delete template from database only
        store
            .delete_template(template_name)
            .await
            .expect("Failed to delete template from database");

        println!("Template '{}' deleted from database", template_name);

        // Since no volumes depend on it, the base ZVOL should be eligible for GC
        // But we need to call GC manually (in real code, this is done in grpc.rs)

        // Check if template still exists in DB
        let tpl_exists = store.template_exists(&template.id).await.expect("DB error");
        assert!(!tpl_exists, "Template should not exist in DB");

        // Check if any volumes depend on it
        let dep_count = store
            .count_volumes_by_origin(&template.id)
            .await
            .expect("DB error");

        if !tpl_exists && dep_count == 0 {
            // Safe to GC
            println!("GC: Deleting orphaned base ZVOL...");
            zfs.delete_base_zvol(&template.id)
                .await
                .expect("Failed to delete base ZVOL");
        }

        // Verify base ZVOL is gone
        assert!(
            !zfs_exists(&base_zvol),
            "Base ZVOL should be gone after GC: {}",
            base_zvol
        );
        println!("Original template base ZVOL garbage collected");

        // === Final Verification ===
        println!("\n--- Final Verification ---");

        // List all datasets in pool
        let remaining = zfs_list(&config.pool_name);
        println!("Remaining datasets in pool:");
        for ds in &remaining {
            // Filter out unrelated datasets
            if ds.contains(".templates/") || ds.contains(&volume_id) || ds.contains(&template.id) {
                println!("  UNEXPECTED: {}", ds);
            }
        }

        // Verify database is clean
        let templates = store.list_templates().await.expect("DB error");
        let volumes = store.list_volumes().await.expect("DB error");

        assert!(
            templates.is_empty(),
            "No templates should remain: {:?}",
            templates.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        assert!(
            volumes.is_empty(),
            "No volumes should remain: {:?}",
            volumes.iter().map(|v| &v.name).collect::<Vec<_>>()
        );

        println!("Database is clean");
        println!("\n=== Lifecycle test completed successfully! ===");
    }

    /// Test that deleting a template while volumes exist preserves the base ZVOL
    #[tokio::test]
    async fn test_template_gc_with_dependent_volume() {
        if !nix::unistd::Uid::effective().is_root() {
            eprintln!("Skipping test: requires root privileges");
            return;
        }

        let config = TestConfig::new();

        if !zfs_exists(&config.pool_name) {
            eprintln!("Skipping test: pool '{}' does not exist", config.pool_name);
            return;
        }

        println!("=== Testing GC with dependent volume ===");

        // Use state_dir for this test
        let gc_state_dir = format!("{}-gc", config.state_dir);
        let db_dir = format!("{}/db", gc_state_dir);
        let _ = std::fs::remove_dir_all(&gc_state_dir);
        std::fs::create_dir_all(&db_dir).expect("Failed to create test db directory");

        let store = Arc::new(
            mvirt_zfs::store::Store::new(&db_dir)
                .await
                .expect("Failed to create store"),
        );
        let zfs = Arc::new(mvirt_zfs::zfs::ZfsManager::new(config.pool_name.clone()));

        // Create a small test "template" (empty ZVOL, not from import)
        let template_id = uuid::Uuid::new_v4().to_string();
        let template_name = "test-gc-template";
        let size_bytes = 1024 * 1024 * 100; // 100MB

        // Ensure base dataset exists
        zfs.ensure_base_dataset()
            .await
            .expect("Failed to ensure base dataset");

        // Create template ZVOL
        zfs.create_template_zvol(&template_id, size_bytes)
            .await
            .expect("Failed to create template ZVOL");

        // Create template snapshot
        let snap_path = zfs
            .create_template_snapshot(&template_id)
            .await
            .expect("Failed to create template snapshot");

        // Store template
        let template_entry = mvirt_zfs::store::TemplateEntry::new(
            template_id.clone(),
            template_name.to_string(),
            zfs.template_zfs_path(&template_id),
            snap_path,
            size_bytes,
        );
        store
            .create_template(&template_entry)
            .await
            .expect("Failed to store template");

        // Clone to volume
        let volume_id = uuid::Uuid::new_v4().to_string();
        let volume_name = "test-gc-volume";

        let vol_info = zfs
            .clone_template_to_volume(&template_id, &volume_id)
            .await
            .expect("Failed to clone");

        let volume_entry = mvirt_zfs::store::VolumeEntry::new(
            volume_id.clone(),
            volume_name.to_string(),
            zfs.volume_zfs_path(&volume_id),
            vol_info.device_path,
            size_bytes,
            Some(template_id.clone()),
        );
        store
            .create_volume(&volume_entry)
            .await
            .expect("Failed to store volume");

        println!("Template and volume created");

        // Delete template
        store
            .delete_template(template_name)
            .await
            .expect("Failed to delete template");
        println!("Template deleted from database");

        // Try to GC - should NOT delete base ZVOL because volume depends on it
        let tpl_exists = store.template_exists(&template_id).await.expect("DB error");
        let dep_count = store
            .count_volumes_by_origin(&template_id)
            .await
            .expect("DB error");

        println!("Template exists in DB: {}", tpl_exists);
        println!("Dependent volume count: {}", dep_count);

        assert!(!tpl_exists, "Template should not exist in DB");
        assert_eq!(dep_count, 1, "Should have 1 dependent volume");

        // Base ZVOL should still exist
        let base_zvol = zfs.template_zfs_path(&template_id);
        assert!(
            zfs_exists(&base_zvol),
            "Base ZVOL should still exist (volume depends on it): {}",
            base_zvol
        );
        println!("Base ZVOL preserved (volume depends on it)");

        // Now delete the volume
        zfs.delete_volume_recursive(&volume_id)
            .await
            .expect("Failed to delete volume");
        store
            .delete_volume(&volume_id)
            .await
            .expect("Failed to delete volume from DB");

        println!("Volume deleted");

        // Now GC should work
        let dep_count_after = store
            .count_volumes_by_origin(&template_id)
            .await
            .expect("DB error");
        assert_eq!(dep_count_after, 0, "No dependent volumes");

        if !tpl_exists && dep_count_after == 0 {
            zfs.destroy_recursive(&zfs.template_zfs_path(&template_id))
                .await
                .expect("Failed to GC template ZVOL");
        }

        assert!(
            !zfs_exists(&base_zvol),
            "Base ZVOL should be gone after GC: {}",
            base_zvol
        );
        println!("Base ZVOL garbage collected after volume deletion");

        // Cleanup
        let _ = std::fs::remove_dir_all(&gc_state_dir);

        println!("=== GC test completed successfully! ===");
    }
}
