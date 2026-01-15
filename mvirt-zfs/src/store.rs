use anyhow::Result;
use chrono::Utc;
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use uuid::Uuid;

/// SQLite-backed metadata store for ZFS volumes
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn new(metadata_dir: &str) -> Result<Self> {
        let db_path = format!("{}/metadata.db", metadata_dir);
        let db_url = format!("sqlite:{}?mode=rwc", db_path);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await?;

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    // === Volume operations ===

    pub async fn create_volume(&self, entry: &VolumeEntry) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO volumes (id, name, zfs_path, device_path, size_bytes, origin_template_id, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.name)
        .bind(&entry.zfs_path)
        .bind(&entry.device_path)
        .bind(entry.size_bytes as i64)
        .bind(&entry.origin_template_id)
        .bind(&entry.created_at)
        .bind(&entry.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_volume(&self, id: &str) -> Result<Option<VolumeEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, zfs_path, device_path, size_bytes, origin_template_id, created_at, updated_at
            FROM volumes WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| VolumeEntry {
            id: r.get("id"),
            name: r.get("name"),
            zfs_path: r.get("zfs_path"),
            device_path: r.get("device_path"),
            size_bytes: r.get::<i64, _>("size_bytes") as u64,
            origin_template_id: r.get("origin_template_id"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }

    pub async fn get_volume_by_name(&self, name: &str) -> Result<Option<VolumeEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, zfs_path, device_path, size_bytes, origin_template_id, created_at, updated_at
            FROM volumes WHERE name = ?
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| VolumeEntry {
            id: r.get("id"),
            name: r.get("name"),
            zfs_path: r.get("zfs_path"),
            device_path: r.get("device_path"),
            size_bytes: r.get::<i64, _>("size_bytes") as u64,
            origin_template_id: r.get("origin_template_id"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }

    pub async fn list_volumes(&self) -> Result<Vec<VolumeEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, zfs_path, device_path, size_bytes, origin_template_id, created_at, updated_at
            FROM volumes ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| VolumeEntry {
                id: r.get("id"),
                name: r.get("name"),
                zfs_path: r.get("zfs_path"),
                device_path: r.get("device_path"),
                size_bytes: r.get::<i64, _>("size_bytes") as u64,
                origin_template_id: r.get("origin_template_id"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    pub async fn delete_volume(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM volumes WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_volume_size(&self, id: &str, size_bytes: u64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE volumes SET size_bytes = ?, updated_at = ? WHERE id = ?")
            .bind(size_bytes as i64)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // === Template operations ===

    pub async fn create_template(&self, entry: &TemplateEntry) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO templates (id, name, base_zvol_path, snapshot_path, size_bytes, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.name)
        .bind(&entry.base_zvol_path)
        .bind(&entry.snapshot_path)
        .bind(entry.size_bytes as i64)
        .bind(&entry.created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_template(&self, name: &str) -> Result<Option<TemplateEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, base_zvol_path, snapshot_path, size_bytes, created_at
            FROM templates WHERE name = ?
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| TemplateEntry {
            id: r.get("id"),
            name: r.get("name"),
            base_zvol_path: r.get("base_zvol_path"),
            snapshot_path: r.get("snapshot_path"),
            size_bytes: r.get::<i64, _>("size_bytes") as u64,
            created_at: r.get("created_at"),
        }))
    }

    pub async fn get_template_by_id(&self, id: &str) -> Result<Option<TemplateEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, base_zvol_path, snapshot_path, size_bytes, created_at
            FROM templates WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| TemplateEntry {
            id: r.get("id"),
            name: r.get("name"),
            base_zvol_path: r.get("base_zvol_path"),
            snapshot_path: r.get("snapshot_path"),
            size_bytes: r.get::<i64, _>("size_bytes") as u64,
            created_at: r.get("created_at"),
        }))
    }

    pub async fn list_templates(&self) -> Result<Vec<TemplateEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, base_zvol_path, snapshot_path, size_bytes, created_at
            FROM templates ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| TemplateEntry {
                id: r.get("id"),
                name: r.get("name"),
                base_zvol_path: r.get("base_zvol_path"),
                snapshot_path: r.get("snapshot_path"),
                size_bytes: r.get::<i64, _>("size_bytes") as u64,
                created_at: r.get("created_at"),
            })
            .collect())
    }

    pub async fn delete_template(&self, name: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM templates WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    // === Import job operations ===

    #[allow(dead_code)]
    pub async fn create_import_job(&self, entry: &ImportJobEntry) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO import_jobs (id, template_name, source, format, state, bytes_written, total_bytes, error, created_at, completed_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.template_name)
        .bind(&entry.source)
        .bind(&entry.format)
        .bind(&entry.state)
        .bind(entry.bytes_written as i64)
        .bind(entry.total_bytes.map(|v| v as i64))
        .bind(&entry.error)
        .bind(&entry.created_at)
        .bind(&entry.completed_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn get_import_job(&self, id: &str) -> Result<Option<ImportJobEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, template_name, source, format, state, bytes_written, total_bytes, error, created_at, completed_at
            FROM import_jobs WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| ImportJobEntry {
            id: r.get("id"),
            template_name: r.get("template_name"),
            source: r.get("source"),
            format: r.get("format"),
            state: r.get("state"),
            bytes_written: r.get::<i64, _>("bytes_written") as u64,
            total_bytes: r.get::<Option<i64>, _>("total_bytes").map(|v| v as u64),
            error: r.get("error"),
            created_at: r.get("created_at"),
            completed_at: r.get("completed_at"),
        }))
    }

    #[allow(dead_code)]
    pub async fn list_import_jobs(&self, include_completed: bool) -> Result<Vec<ImportJobEntry>> {
        let query = if include_completed {
            "SELECT id, template_name, source, format, state, bytes_written, total_bytes, error, created_at, completed_at FROM import_jobs ORDER BY created_at DESC"
        } else {
            "SELECT id, template_name, source, format, state, bytes_written, total_bytes, error, created_at, completed_at FROM import_jobs WHERE state NOT IN ('completed', 'failed', 'cancelled') ORDER BY created_at DESC"
        };

        let rows = sqlx::query(query).fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|r| ImportJobEntry {
                id: r.get("id"),
                template_name: r.get("template_name"),
                source: r.get("source"),
                format: r.get("format"),
                state: r.get("state"),
                bytes_written: r.get::<i64, _>("bytes_written") as u64,
                total_bytes: r.get::<Option<i64>, _>("total_bytes").map(|v| v as u64),
                error: r.get("error"),
                created_at: r.get("created_at"),
                completed_at: r.get("completed_at"),
            })
            .collect())
    }

    pub async fn update_import_job(
        &self,
        id: &str,
        state: &str,
        bytes_written: u64,
        total_bytes: Option<u64>,
        error: Option<&str>,
    ) -> Result<()> {
        let completed_at = if state == "completed" || state == "failed" || state == "cancelled" {
            Some(Utc::now().to_rfc3339())
        } else {
            None
        };

        sqlx::query(
            "UPDATE import_jobs SET state = ?, bytes_written = ?, total_bytes = COALESCE(?, total_bytes), error = ?, completed_at = COALESCE(?, completed_at) WHERE id = ?",
        )
        .bind(state)
        .bind(bytes_written as i64)
        .bind(total_bytes.map(|b| b as i64))
        .bind(error)
        .bind(&completed_at)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // === Snapshot operations ===

    pub async fn create_snapshot(&self, entry: &SnapshotEntry) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO snapshots (id, volume_id, name, zfs_name, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.volume_id)
        .bind(&entry.name)
        .bind(&entry.zfs_name)
        .bind(&entry.created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_snapshot(&self, volume_id: &str, name: &str) -> Result<Option<SnapshotEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, volume_id, name, zfs_name, created_at
            FROM snapshots WHERE volume_id = ? AND name = ?
            "#,
        )
        .bind(volume_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| SnapshotEntry {
            id: r.get("id"),
            volume_id: r.get("volume_id"),
            name: r.get("name"),
            zfs_name: r.get("zfs_name"),
            created_at: r.get("created_at"),
        }))
    }

    pub async fn list_snapshots(&self, volume_id: &str) -> Result<Vec<SnapshotEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT id, volume_id, name, zfs_name, created_at
            FROM snapshots WHERE volume_id = ? ORDER BY created_at DESC
            "#,
        )
        .bind(volume_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| SnapshotEntry {
                id: r.get("id"),
                volume_id: r.get("volume_id"),
                name: r.get("name"),
                zfs_name: r.get("zfs_name"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    pub async fn delete_snapshot(&self, volume_id: &str, name: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM snapshots WHERE volume_id = ? AND name = ?")
            .bind(volume_id)
            .bind(name)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_snapshot_by_id(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM snapshots WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Delete a volume and all its snapshots in a single transaction.
    pub async fn delete_volume_with_snapshots(&self, volume_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // 1. Delete snapshots
        sqlx::query("DELETE FROM snapshots WHERE volume_id = ?")
            .bind(volume_id)
            .execute(&mut *tx)
            .await?;

        // 2. Delete volume
        sqlx::query("DELETE FROM volumes WHERE id = ?")
            .bind(volume_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    // === Garbage Collection helpers ===

    /// Count volumes that originated from a given template
    pub async fn count_volumes_by_origin(&self, template_id: &str) -> Result<u64> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM volumes WHERE origin_template_id = ?")
            .bind(template_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(row.get::<i64, _>("count") as u64)
    }

    /// Check if a template exists by ID
    pub async fn template_exists(&self, template_id: &str) -> Result<bool> {
        let row = sqlx::query("SELECT 1 FROM templates WHERE id = ?")
            .bind(template_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.is_some())
    }
}

// === Entry types ===

#[derive(Debug, Clone)]
pub struct VolumeEntry {
    pub id: String,
    pub name: String,
    pub zfs_path: String,
    pub device_path: String,
    pub size_bytes: u64,
    pub origin_template_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl VolumeEntry {
    pub fn new(
        id: String,
        name: String,
        zfs_path: String,
        device_path: String,
        size_bytes: u64,
        origin_template_id: Option<String>,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id,
            name,
            zfs_path,
            device_path,
            size_bytes,
            origin_template_id,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TemplateEntry {
    pub id: String,
    pub name: String,
    /// ZFS path to the template ZVOL (e.g., mvirt/templates/<uuid>)
    pub base_zvol_path: Option<String>,
    /// ZFS snapshot path for cloning (e.g., mvirt/templates/<uuid>@img)
    pub snapshot_path: Option<String>,
    pub size_bytes: u64,
    pub created_at: String,
}

impl TemplateEntry {
    /// Create a template entry
    pub fn new(
        id: String,
        name: String,
        base_zvol_path: String,
        snapshot_path: String,
        size_bytes: u64,
    ) -> Self {
        Self {
            id,
            name,
            base_zvol_path: Some(base_zvol_path),
            snapshot_path: Some(snapshot_path),
            size_bytes,
            created_at: Utc::now().to_rfc3339(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ImportJobEntry {
    pub id: String,
    pub template_name: String,
    pub source: String,
    pub format: String,
    pub state: String,
    pub bytes_written: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

impl ImportJobEntry {
    #[allow(dead_code)]
    pub fn new(
        template_name: String,
        source: String,
        format: String,
        total_bytes: Option<u64>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            template_name,
            source,
            format,
            state: "pending".to_string(),
            bytes_written: 0,
            total_bytes,
            error: None,
            created_at: Utc::now().to_rfc3339(),
            completed_at: None,
        }
    }
}

/// Snapshot entry (directly contains ZFS snapshot name)
#[derive(Debug, Clone)]
pub struct SnapshotEntry {
    pub id: String,
    pub volume_id: String,
    pub name: String,
    /// The UUID used in ZFS path: mvirt/volumes/<vol-uuid>@<zfs_name>
    pub zfs_name: String,
    pub created_at: String,
}

impl SnapshotEntry {
    pub fn new(id: String, volume_id: String, name: String, zfs_name: String) -> Self {
        Self {
            id,
            volume_id,
            name,
            zfs_name,
            created_at: Utc::now().to_rfc3339(),
        }
    }
}
