use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use uuid::Uuid;

use crate::proto::{Vm, VmConfig, VmState};

pub struct VmStore {
    pool: SqlitePool,
}

impl VmStore {
    pub async fn new(data_dir: &Path) -> Result<Self> {
        let db_path = data_dir.join("mvirt.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await?;

        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS vms (
                id TEXT PRIMARY KEY,
                name TEXT,
                state TEXT NOT NULL DEFAULT 'stopped',
                config_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                started_at INTEGER
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS vm_runtime (
                vm_id TEXT PRIMARY KEY REFERENCES vms(id) ON DELETE CASCADE,
                pid INTEGER NOT NULL,
                api_socket TEXT NOT NULL,
                serial_socket TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn create(&self, name: Option<String>, config: VmConfig) -> Result<VmEntry> {
        let id = Uuid::new_v4().to_string();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let config_json = serde_json::to_string(&ProtoConfig::from(config.clone()))?;

        sqlx::query(
            r#"
            INSERT INTO vms (id, name, state, config_json, created_at)
            VALUES (?, ?, 'stopped', ?, ?)
            "#,
        )
        .bind(&id)
        .bind(&name)
        .bind(&config_json)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(VmEntry {
            id,
            name,
            state: VmState::Stopped,
            config,
            created_at: now,
            started_at: None,
        })
    }

    pub async fn get(&self, id: &str) -> Result<Option<VmEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, state, config_json, created_at, started_at
            FROM vms WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_entry(row)?)),
            None => Ok(None),
        }
    }

    pub async fn list(&self) -> Result<Vec<VmEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, state, config_json, created_at, started_at
            FROM vms ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_entry).collect()
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM vms WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_state(&self, id: &str, state: VmState) -> Result<Option<VmEntry>> {
        let state_str = state_to_str(state);
        let now = if state == VmState::Running {
            Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            )
        } else {
            None
        };

        let result = sqlx::query(
            r#"
            UPDATE vms SET state = ?, started_at = COALESCE(?, started_at)
            WHERE id = ?
            "#,
        )
        .bind(state_str)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Ok(None);
        }

        self.get(id).await
    }

    // Runtime management

    pub async fn set_runtime(
        &self,
        vm_id: &str,
        pid: u32,
        api_socket: &str,
        serial_socket: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO vm_runtime (vm_id, pid, api_socket, serial_socket)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(vm_id)
        .bind(pid as i64)
        .bind(api_socket)
        .bind(serial_socket)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_runtime(&self, vm_id: &str) -> Result<Option<VmRuntime>> {
        let row = sqlx::query(
            r#"
            SELECT pid, api_socket, serial_socket
            FROM vm_runtime WHERE vm_id = ?
            "#,
        )
        .bind(vm_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| VmRuntime {
            pid: r.get::<i64, _>("pid") as u32,
            api_socket: r.get("api_socket"),
            serial_socket: r.get("serial_socket"),
        }))
    }

    pub async fn clear_runtime(&self, vm_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM vm_runtime WHERE vm_id = ?")
            .bind(vm_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

// Helper types

#[derive(Debug, Clone)]
pub struct VmEntry {
    pub id: String,
    pub name: Option<String>,
    pub state: VmState,
    pub config: VmConfig,
    pub created_at: i64,
    pub started_at: Option<i64>,
}

impl VmEntry {
    pub fn to_proto(&self) -> Vm {
        Vm {
            id: self.id.clone(),
            name: self.name.clone(),
            state: self.state.into(),
            config: Some(self.config.clone()),
            created_at: self.created_at,
            started_at: self.started_at,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields stored for recovery and future use
pub struct VmRuntime {
    pub pid: u32,
    pub api_socket: String,
    pub serial_socket: String,
}

// Serialization helpers for VmConfig

#[derive(serde::Serialize, serde::Deserialize)]
struct ProtoConfig {
    vcpus: u32,
    memory_mb: u64,
    #[serde(default)]
    boot_mode: i32, // 0=unspecified, 1=disk, 2=kernel
    #[serde(default)]
    kernel: Option<String>,
    initramfs: Option<String>,
    cmdline: Option<String>,
    disks: Vec<ProtoDisk>,
    nics: Vec<ProtoNic>,
    #[serde(default)]
    user_data: Option<String>,
    #[serde(default)]
    nested_virt: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProtoDisk {
    path: String,
    readonly: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProtoNic {
    tap: Option<String>,
    mac: Option<String>,
    #[serde(default)]
    vhost_socket: Option<String>,
}

impl From<VmConfig> for ProtoConfig {
    fn from(c: VmConfig) -> Self {
        Self {
            vcpus: c.vcpus,
            memory_mb: c.memory_mb,
            boot_mode: c.boot_mode,
            kernel: c.kernel,
            initramfs: c.initramfs,
            cmdline: c.cmdline,
            disks: c
                .disks
                .into_iter()
                .map(|d| ProtoDisk {
                    path: d.path,
                    readonly: d.readonly,
                })
                .collect(),
            nics: c
                .nics
                .into_iter()
                .map(|n| ProtoNic {
                    tap: n.tap,
                    mac: n.mac,
                    vhost_socket: n.vhost_socket,
                })
                .collect(),
            user_data: c.user_data,
            nested_virt: c.nested_virt,
        }
    }
}

impl From<ProtoConfig> for VmConfig {
    fn from(c: ProtoConfig) -> Self {
        use crate::proto::{DiskConfig, NicConfig};
        Self {
            vcpus: c.vcpus,
            memory_mb: c.memory_mb,
            boot_mode: c.boot_mode,
            kernel: c.kernel,
            initramfs: c.initramfs,
            cmdline: c.cmdline,
            disks: c
                .disks
                .into_iter()
                .map(|d| DiskConfig {
                    path: d.path,
                    readonly: d.readonly,
                })
                .collect(),
            nics: c
                .nics
                .into_iter()
                .map(|n| NicConfig {
                    tap: n.tap,
                    mac: n.mac,
                    vhost_socket: n.vhost_socket,
                })
                .collect(),
            user_data: c.user_data,
            nested_virt: c.nested_virt,
        }
    }
}

fn row_to_entry(row: sqlx::sqlite::SqliteRow) -> Result<VmEntry> {
    let config_json: String = row.get("config_json");
    let proto_config: ProtoConfig = serde_json::from_str(&config_json)?;

    Ok(VmEntry {
        id: row.get("id"),
        name: row.get("name"),
        state: str_to_state(row.get("state")),
        config: proto_config.into(),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
    })
}

fn state_to_str(state: VmState) -> &'static str {
    match state {
        VmState::Unspecified => "unspecified",
        VmState::Stopped => "stopped",
        VmState::Starting => "starting",
        VmState::Running => "running",
        VmState::Stopping => "stopping",
    }
}

fn str_to_state(s: &str) -> VmState {
    match s {
        "stopped" => VmState::Stopped,
        "starting" => VmState::Starting,
        "running" => VmState::Running,
        "stopping" => VmState::Stopping,
        _ => VmState::Unspecified,
    }
}
