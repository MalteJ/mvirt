-- Initial schema for mvirt-zfs
-- This migration is safe to run on existing databases (uses IF NOT EXISTS)

-- Volumes table: tracks all managed volumes
CREATE TABLE IF NOT EXISTS volumes (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    zfs_path TEXT NOT NULL,
    device_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    origin_template_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- ZFS snapshots table: tracks actual ZFS snapshots with reference counting
CREATE TABLE IF NOT EXISTS zfs_snapshots (
    id TEXT PRIMARY KEY,
    volume_id TEXT NOT NULL,
    zfs_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (volume_id) REFERENCES volumes(id)
);

-- Templates table: base images for cloning volumes
CREATE TABLE IF NOT EXISTS templates (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    zfs_snapshot_id TEXT,
    base_zvol_path TEXT,
    snapshot_path TEXT,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (zfs_snapshot_id) REFERENCES zfs_snapshots(id)
);

-- Import jobs table: tracks async import operations
CREATE TABLE IF NOT EXISTS import_jobs (
    id TEXT PRIMARY KEY,
    template_name TEXT NOT NULL,
    source TEXT NOT NULL,
    format TEXT NOT NULL,
    state TEXT NOT NULL,
    bytes_written INTEGER DEFAULT 0,
    total_bytes INTEGER,
    error TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT
);

-- Snapshots table: point-in-time captures of volumes
CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    volume_id TEXT NOT NULL,
    name TEXT NOT NULL,
    zfs_snapshot_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (volume_id) REFERENCES volumes(id) ON DELETE CASCADE,
    FOREIGN KEY (zfs_snapshot_id) REFERENCES zfs_snapshots(id),
    UNIQUE(volume_id, name)
);
