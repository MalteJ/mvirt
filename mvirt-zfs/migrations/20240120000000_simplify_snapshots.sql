-- Simplify snapshot schema: merge zfs_snapshots into snapshots
-- Templates are now independent copies, not clones, so no reference counting needed

-- SQLite doesn't support DROP COLUMN, so we recreate the tables

-- Step 1: Create new snapshots table with zfs_name column
CREATE TABLE IF NOT EXISTS snapshots_new (
    id TEXT PRIMARY KEY,
    volume_id TEXT NOT NULL,
    name TEXT NOT NULL,
    zfs_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (volume_id) REFERENCES volumes(id) ON DELETE CASCADE,
    UNIQUE(volume_id, name)
);

-- Step 2: Migrate data from old snapshots table (if exists and has data)
INSERT OR IGNORE INTO snapshots_new (id, volume_id, name, zfs_name, created_at)
SELECT s.id, s.volume_id, s.name, z.zfs_name, s.created_at
FROM snapshots s
JOIN zfs_snapshots z ON s.zfs_snapshot_id = z.id
WHERE EXISTS (SELECT 1 FROM snapshots LIMIT 1);

-- Step 3: Drop old tables
DROP TABLE IF EXISTS snapshots;
DROP TABLE IF EXISTS zfs_snapshots;

-- Step 4: Rename new table
ALTER TABLE snapshots_new RENAME TO snapshots;

-- Step 5: Remove zfs_snapshot_id from templates (recreate without it)
CREATE TABLE IF NOT EXISTS templates_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    base_zvol_path TEXT,
    snapshot_path TEXT,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

-- Step 6: Migrate template data
INSERT OR IGNORE INTO templates_new (id, name, base_zvol_path, snapshot_path, size_bytes, created_at)
SELECT id, name, base_zvol_path, snapshot_path, size_bytes, created_at
FROM templates
WHERE EXISTS (SELECT 1 FROM templates LIMIT 1);

-- Step 7: Drop old templates table and rename
DROP TABLE IF EXISTS templates;
ALTER TABLE templates_new RENAME TO templates;
