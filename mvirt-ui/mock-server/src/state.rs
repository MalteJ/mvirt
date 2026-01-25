use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskConfig {
    pub path: String,
    pub readonly: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NicConfig {
    pub mac_address: String,
    pub network_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmConfig {
    pub vcpus: u32,
    pub memory_mb: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_disk: Option<String>,
    pub disks: Vec<DiskConfig>,
    pub nics: Vec<NicConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vm {
    pub id: String,
    pub name: String,
    pub state: VmState,
    pub config: VmConfig,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub used_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub id: String,
    pub name: String,
    pub path: String,
    pub volsize_bytes: u64,
    pub used_bytes: u64,
    pub compression_ratio: f64,
    pub snapshots: Vec<Snapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub clone_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImportJobState {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportJob {
    pub id: String,
    pub template_name: String,
    pub state: ImportJobState,
    pub bytes_written: u64,
    pub total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_subnet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6_prefix: Option<String>,
    pub nic_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NicState {
    Detached,
    Attached,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nic {
    pub id: String,
    pub name: String,
    pub mac_address: String,
    pub network_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    pub state: NicState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6_address: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Audit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: String,
    pub timestamp_ns: i64,
    pub message: String,
    pub level: LogLevel,
    pub component: String,
    pub related_object_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolStats {
    pub name: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub version: String,
    pub hostname: String,
    pub cpu_count: u32,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub uptime: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeState {
    Online,
    Offline,
    Maintenance,
    Joining,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeRole {
    Leader,
    Follower,
    Candidate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub name: String,
    pub address: String,
    pub state: NodeState,
    pub role: NodeRole,
    pub version: String,
    pub cpu_count: u32,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub vm_count: u32,
    pub uptime: u64,
    pub last_seen: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterInfo {
    pub id: String,
    pub name: String,
    pub node_count: u32,
    pub leader_node_id: String,
    pub term: u64,
    pub created_at: String,
}

pub struct AppStateInner {
    pub vms: HashMap<String, Vm>,
    pub volumes: HashMap<String, Volume>,
    pub templates: HashMap<String, Template>,
    pub import_jobs: HashMap<String, ImportJob>,
    pub networks: HashMap<String, Network>,
    pub nics: HashMap<String, Nic>,
    pub logs: Vec<LogEntry>,
    pub vm_events_tx: broadcast::Sender<Vm>,
    pub log_events_tx: broadcast::Sender<LogEntry>,
}

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<RwLock<AppStateInner>>,
    pub vm_events_tx: broadcast::Sender<Vm>,
    pub log_events_tx: broadcast::Sender<LogEntry>,
}

impl AppState {
    pub fn new() -> Self {
        let (vm_events_tx, _) = broadcast::channel(100);
        let (log_events_tx, _) = broadcast::channel(100);

        let mut state = AppStateInner {
            vms: HashMap::new(),
            volumes: HashMap::new(),
            templates: HashMap::new(),
            import_jobs: HashMap::new(),
            networks: HashMap::new(),
            nics: HashMap::new(),
            logs: Vec::new(),
            vm_events_tx: vm_events_tx.clone(),
            log_events_tx: log_events_tx.clone(),
        };

        // Add some mock data
        Self::init_mock_data(&mut state);

        AppState {
            inner: Arc::new(RwLock::new(state)),
            vm_events_tx,
            log_events_tx,
        }
    }

    fn init_mock_data(state: &mut AppStateInner) {
        // Create mock networks
        let net1_id = Uuid::new_v4().to_string();
        state.networks.insert(
            net1_id.clone(),
            Network {
                id: net1_id.clone(),
                name: "default".to_string(),
                ipv4_subnet: Some("10.0.0.0/24".to_string()),
                ipv6_prefix: Some("fd00::/64".to_string()),
                nic_count: 2,
            },
        );

        let net2_id = Uuid::new_v4().to_string();
        state.networks.insert(
            net2_id.clone(),
            Network {
                id: net2_id.clone(),
                name: "management".to_string(),
                ipv4_subnet: Some("192.168.1.0/24".to_string()),
                ipv6_prefix: None,
                nic_count: 1,
            },
        );

        // Create mock VMs
        let vm1_id = Uuid::new_v4().to_string();
        state.vms.insert(
            vm1_id.clone(),
            Vm {
                id: vm1_id.clone(),
                name: "web-server-01".to_string(),
                state: VmState::Running,
                config: VmConfig {
                    vcpus: 2,
                    memory_mb: 2048,
                    kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                    boot_disk: None,
                    disks: vec![DiskConfig {
                        path: "/dev/zvol/tank/vm/web-server-01".to_string(),
                        readonly: false,
                    }],
                    nics: vec![NicConfig {
                        mac_address: "52:54:00:12:34:56".to_string(),
                        network_id: net1_id.clone(),
                    }],
                    user_data: None,
                },
                created_at: "2024-01-15T10:30:00Z".to_string(),
                started_at: Some("2024-01-15T10:30:05Z".to_string()),
            },
        );

        let vm2_id = Uuid::new_v4().to_string();
        state.vms.insert(
            vm2_id.clone(),
            Vm {
                id: vm2_id.clone(),
                name: "database".to_string(),
                state: VmState::Running,
                config: VmConfig {
                    vcpus: 4,
                    memory_mb: 8192,
                    kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                    boot_disk: None,
                    disks: vec![
                        DiskConfig {
                            path: "/dev/zvol/tank/vm/database-root".to_string(),
                            readonly: false,
                        },
                        DiskConfig {
                            path: "/dev/zvol/tank/vm/database-data".to_string(),
                            readonly: false,
                        },
                    ],
                    nics: vec![NicConfig {
                        mac_address: "52:54:00:ab:cd:ef".to_string(),
                        network_id: net1_id.clone(),
                    }],
                    user_data: None,
                },
                created_at: "2024-01-10T08:00:00Z".to_string(),
                started_at: Some("2024-01-10T08:00:10Z".to_string()),
            },
        );

        let vm3_id = Uuid::new_v4().to_string();
        state.vms.insert(
            vm3_id.clone(),
            Vm {
                id: vm3_id.clone(),
                name: "dev-env".to_string(),
                state: VmState::Stopped,
                config: VmConfig {
                    vcpus: 2,
                    memory_mb: 4096,
                    kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                    boot_disk: None,
                    disks: vec![DiskConfig {
                        path: "/dev/zvol/tank/vm/dev-env".to_string(),
                        readonly: false,
                    }],
                    nics: vec![NicConfig {
                        mac_address: "52:54:00:11:22:33".to_string(),
                        network_id: net2_id.clone(),
                    }],
                    user_data: None,
                },
                created_at: "2024-01-20T14:00:00Z".to_string(),
                started_at: None,
            },
        );

        // Create mock volumes
        let vol1_id = Uuid::new_v4().to_string();
        state.volumes.insert(
            vol1_id.clone(),
            Volume {
                id: vol1_id.clone(),
                name: "web-server-01".to_string(),
                path: "tank/vm/web-server-01".to_string(),
                volsize_bytes: 20 * 1024 * 1024 * 1024,
                used_bytes: 8 * 1024 * 1024 * 1024,
                compression_ratio: 1.45,
                snapshots: vec![Snapshot {
                    id: Uuid::new_v4().to_string(),
                    name: "before-upgrade".to_string(),
                    created_at: "2024-01-14T09:00:00Z".to_string(),
                    used_bytes: 500 * 1024 * 1024,
                }],
            },
        );

        let vol2_id = Uuid::new_v4().to_string();
        state.volumes.insert(
            vol2_id.clone(),
            Volume {
                id: vol2_id.clone(),
                name: "database-root".to_string(),
                path: "tank/vm/database-root".to_string(),
                volsize_bytes: 50 * 1024 * 1024 * 1024,
                used_bytes: 15 * 1024 * 1024 * 1024,
                compression_ratio: 2.1,
                snapshots: vec![],
            },
        );

        let vol3_id = Uuid::new_v4().to_string();
        state.volumes.insert(
            vol3_id.clone(),
            Volume {
                id: vol3_id.clone(),
                name: "database-data".to_string(),
                path: "tank/vm/database-data".to_string(),
                volsize_bytes: 200 * 1024 * 1024 * 1024,
                used_bytes: 80 * 1024 * 1024 * 1024,
                compression_ratio: 1.8,
                snapshots: vec![
                    Snapshot {
                        id: Uuid::new_v4().to_string(),
                        name: "daily-2024-01-14".to_string(),
                        created_at: "2024-01-14T00:00:00Z".to_string(),
                        used_bytes: 2 * 1024 * 1024 * 1024,
                    },
                    Snapshot {
                        id: Uuid::new_v4().to_string(),
                        name: "daily-2024-01-15".to_string(),
                        created_at: "2024-01-15T00:00:00Z".to_string(),
                        used_bytes: 1024 * 1024 * 1024,
                    },
                ],
            },
        );

        // Create mock templates
        let tpl1_id = Uuid::new_v4().to_string();
        state.templates.insert(
            tpl1_id.clone(),
            Template {
                id: tpl1_id,
                name: "ubuntu-22.04".to_string(),
                size_bytes: 3 * 1024 * 1024 * 1024,
                clone_count: 5,
            },
        );

        let tpl2_id = Uuid::new_v4().to_string();
        state.templates.insert(
            tpl2_id.clone(),
            Template {
                id: tpl2_id,
                name: "debian-12".to_string(),
                size_bytes: 2 * 1024 * 1024 * 1024,
                clone_count: 2,
            },
        );

        // Create mock NICs
        let nic1_id = Uuid::new_v4().to_string();
        state.nics.insert(
            nic1_id.clone(),
            Nic {
                id: nic1_id,
                name: "web-server-01-eth0".to_string(),
                mac_address: "52:54:00:12:34:56".to_string(),
                network_id: net1_id.clone(),
                vm_id: Some(vm1_id.clone()),
                state: NicState::Attached,
                ipv4_address: Some("10.0.0.10".to_string()),
                ipv6_address: Some("fd00::10".to_string()),
            },
        );

        let nic2_id = Uuid::new_v4().to_string();
        state.nics.insert(
            nic2_id.clone(),
            Nic {
                id: nic2_id,
                name: "database-eth0".to_string(),
                mac_address: "52:54:00:ab:cd:ef".to_string(),
                network_id: net1_id.clone(),
                vm_id: Some(vm2_id.clone()),
                state: NicState::Attached,
                ipv4_address: Some("10.0.0.20".to_string()),
                ipv6_address: Some("fd00::20".to_string()),
            },
        );

        let nic3_id = Uuid::new_v4().to_string();
        state.nics.insert(
            nic3_id.clone(),
            Nic {
                id: nic3_id,
                name: "spare-nic".to_string(),
                mac_address: "52:54:00:99:88:77".to_string(),
                network_id: net1_id,
                vm_id: None,
                state: NicState::Detached,
                ipv4_address: None,
                ipv6_address: None,
            },
        );

        // Create mock logs
        let now = chrono::Utc::now();
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: (now - chrono::Duration::minutes(30))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "VM web-server-01 started".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm1_id.clone()],
        });

        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: (now - chrono::Duration::minutes(25))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "VM database started".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm2_id.clone()],
        });

        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: (now - chrono::Duration::minutes(10))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Snapshot 'daily-2024-01-15' created for volume database-data".to_string(),
            level: LogLevel::Audit,
            component: "zfs".to_string(),
            related_object_ids: vec![vol3_id],
        });

        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: (now - chrono::Duration::minutes(5))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Service started on [::]:50051".to_string(),
            level: LogLevel::Info,
            component: "vmm".to_string(),
            related_object_ids: vec![],
        });

        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: now.timestamp_nanos_opt().unwrap_or(0),
            message: "Health check passed".to_string(),
            level: LogLevel::Debug,
            component: "vmm".to_string(),
            related_object_ids: vec![],
        });
    }
}
