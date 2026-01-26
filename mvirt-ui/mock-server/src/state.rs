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
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PodState {
    Created,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContainerState {
    Creating,
    Created,
    Running,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub id: String,
    pub name: String,
    pub state: ContainerState,
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pod {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub state: PodState,
    pub network_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    pub containers: Vec<Container>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
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
    pub project_id: String,
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
    pub project_id: String,
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
    pub project_id: String,
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
    pub project_id: String,
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
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
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
    pub project_id: String,
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
    pub projects: HashMap<String, Project>,
    pub vms: HashMap<String, Vm>,
    pub pods: HashMap<String, Pod>,
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
            projects: HashMap::new(),
            vms: HashMap::new(),
            pods: HashMap::new(),
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
        // Create mock projects
        let proj1_id = Uuid::new_v4().to_string();
        state.projects.insert(
            proj1_id.clone(),
            Project {
                id: proj1_id.clone(),
                name: "neonwave".to_string(),
                description: Some("AI-powered music streaming platform".to_string()),
                created_at: "2024-01-01T00:00:00Z".to_string(),
            },
        );

        let proj2_id = Uuid::new_v4().to_string();
        state.projects.insert(
            proj2_id.clone(),
            Project {
                id: proj2_id.clone(),
                name: "pixelforge".to_string(),
                description: Some("Real-time collaborative design tool".to_string()),
                created_at: "2024-01-02T00:00:00Z".to_string(),
            },
        );

        let proj3_id = Uuid::new_v4().to_string();
        state.projects.insert(
            proj3_id.clone(),
            Project {
                id: proj3_id.clone(),
                name: "cloudharbor".to_string(),
                description: Some("Multi-cloud deployment orchestrator".to_string()),
                created_at: "2024-01-03T00:00:00Z".to_string(),
            },
        );

        // ===== NETWORKS =====
        // neonwave networks (3)
        let net_neon1 = Uuid::new_v4().to_string();
        state.networks.insert(net_neon1.clone(), Network {
            id: net_neon1.clone(),
            project_id: proj1_id.clone(),
            name: "neonwave-prod".to_string(),
            ipv4_subnet: Some("10.0.0.0/24".to_string()),
            ipv6_prefix: Some("fd00::/64".to_string()),
            nic_count: 4,
        });
        let net_neon2 = Uuid::new_v4().to_string();
        state.networks.insert(net_neon2.clone(), Network {
            id: net_neon2.clone(),
            project_id: proj1_id.clone(),
            name: "neonwave-internal".to_string(),
            ipv4_subnet: Some("10.0.1.0/24".to_string()),
            ipv6_prefix: None,
            nic_count: 2,
        });
        let net_neon3 = Uuid::new_v4().to_string();
        state.networks.insert(net_neon3.clone(), Network {
            id: net_neon3.clone(),
            project_id: proj1_id.clone(),
            name: "neonwave-streaming".to_string(),
            ipv4_subnet: Some("10.0.2.0/24".to_string()),
            ipv6_prefix: Some("fd00:2::/64".to_string()),
            nic_count: 3,
        });

        // pixelforge networks (2)
        let net_pixel1 = Uuid::new_v4().to_string();
        state.networks.insert(net_pixel1.clone(), Network {
            id: net_pixel1.clone(),
            project_id: proj2_id.clone(),
            name: "pixelforge-main".to_string(),
            ipv4_subnet: Some("10.1.0.0/24".to_string()),
            ipv6_prefix: Some("fd01::/64".to_string()),
            nic_count: 3,
        });
        let net_pixel2 = Uuid::new_v4().to_string();
        state.networks.insert(net_pixel2.clone(), Network {
            id: net_pixel2.clone(),
            project_id: proj2_id.clone(),
            name: "pixelforge-storage".to_string(),
            ipv4_subnet: Some("10.1.1.0/24".to_string()),
            ipv6_prefix: None,
            nic_count: 2,
        });

        // cloudharbor networks (4)
        let net_cloud1 = Uuid::new_v4().to_string();
        state.networks.insert(net_cloud1.clone(), Network {
            id: net_cloud1.clone(),
            project_id: proj3_id.clone(),
            name: "cloudharbor-mgmt".to_string(),
            ipv4_subnet: Some("192.168.0.0/24".to_string()),
            ipv6_prefix: Some("fd02::/64".to_string()),
            nic_count: 3,
        });
        let net_cloud2 = Uuid::new_v4().to_string();
        state.networks.insert(net_cloud2.clone(), Network {
            id: net_cloud2.clone(),
            project_id: proj3_id.clone(),
            name: "cloudharbor-data".to_string(),
            ipv4_subnet: Some("192.168.1.0/24".to_string()),
            ipv6_prefix: None,
            nic_count: 2,
        });
        let net_cloud3 = Uuid::new_v4().to_string();
        state.networks.insert(net_cloud3.clone(), Network {
            id: net_cloud3.clone(),
            project_id: proj3_id.clone(),
            name: "cloudharbor-k8s".to_string(),
            ipv4_subnet: Some("10.244.0.0/16".to_string()),
            ipv6_prefix: None,
            nic_count: 5,
        });
        let net_cloud4 = Uuid::new_v4().to_string();
        state.networks.insert(net_cloud4.clone(), Network {
            id: net_cloud4.clone(),
            project_id: proj3_id.clone(),
            name: "cloudharbor-vpn".to_string(),
            ipv4_subnet: Some("172.16.0.0/16".to_string()),
            ipv6_prefix: Some("fd02:1::/64".to_string()),
            nic_count: 1,
        });

        // ===== VMs =====
        // neonwave VMs (4)
        let vm_neon1 = Uuid::new_v4().to_string();
        state.vms.insert(vm_neon1.clone(), Vm {
            id: vm_neon1.clone(),
            project_id: proj1_id.clone(),
            name: "neon-api-01".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 4,
                memory_mb: 8192,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/neon-api-01".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:12:34:56".to_string(),
                    network_id: net_neon1.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-15T10:30:00Z".to_string(),
            started_at: Some("2024-01-15T10:30:05Z".to_string()),
        });
        let vm_neon2 = Uuid::new_v4().to_string();
        state.vms.insert(vm_neon2.clone(), Vm {
            id: vm_neon2.clone(),
            project_id: proj1_id.clone(),
            name: "neon-db-primary".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 8,
                memory_mb: 16384,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![
                    DiskConfig { path: "/dev/zvol/tank/vm/neon-db-root".to_string(), readonly: false },
                    DiskConfig { path: "/dev/zvol/tank/vm/neon-db-data".to_string(), readonly: false },
                ],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:ab:cd:ef".to_string(),
                    network_id: net_neon2.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-10T08:00:00Z".to_string(),
            started_at: Some("2024-01-10T08:00:10Z".to_string()),
        });
        let vm_neon3 = Uuid::new_v4().to_string();
        state.vms.insert(vm_neon3.clone(), Vm {
            id: vm_neon3.clone(),
            project_id: proj1_id.clone(),
            name: "neon-stream-encoder".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 16,
                memory_mb: 32768,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/neon-encoder".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:11:22:33".to_string(),
                    network_id: net_neon3.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-12T14:00:00Z".to_string(),
            started_at: Some("2024-01-12T14:00:15Z".to_string()),
        });
        let vm_neon4 = Uuid::new_v4().to_string();
        state.vms.insert(vm_neon4.clone(), Vm {
            id: vm_neon4.clone(),
            project_id: proj1_id.clone(),
            name: "neon-cache".to_string(),
            state: VmState::Stopped,
            config: VmConfig {
                vcpus: 2,
                memory_mb: 4096,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/neon-cache".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:44:55:66".to_string(),
                    network_id: net_neon1.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-18T09:00:00Z".to_string(),
            started_at: None,
        });

        // pixelforge VMs (3)
        let vm_pixel1 = Uuid::new_v4().to_string();
        state.vms.insert(vm_pixel1.clone(), Vm {
            id: vm_pixel1.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-web-01".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 4,
                memory_mb: 8192,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/pixel-web".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:77:88:99".to_string(),
                    network_id: net_pixel1.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-08T11:00:00Z".to_string(),
            started_at: Some("2024-01-08T11:00:08Z".to_string()),
        });
        let vm_pixel2 = Uuid::new_v4().to_string();
        state.vms.insert(vm_pixel2.clone(), Vm {
            id: vm_pixel2.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-render-node".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 32,
                memory_mb: 65536,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/pixel-render".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:aa:bb:cc".to_string(),
                    network_id: net_pixel2.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-05T16:30:00Z".to_string(),
            started_at: Some("2024-01-05T16:30:20Z".to_string()),
        });
        let vm_pixel3 = Uuid::new_v4().to_string();
        state.vms.insert(vm_pixel3.clone(), Vm {
            id: vm_pixel3.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-collab-server".to_string(),
            state: VmState::Starting,
            config: VmConfig {
                vcpus: 8,
                memory_mb: 16384,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/pixel-collab".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:dd:ee:ff".to_string(),
                    network_id: net_pixel1.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-20T08:00:00Z".to_string(),
            started_at: None,
        });

        // cloudharbor VMs (5)
        let vm_cloud1 = Uuid::new_v4().to_string();
        state.vms.insert(vm_cloud1.clone(), Vm {
            id: vm_cloud1.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-controller".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 4,
                memory_mb: 8192,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/harbor-ctrl".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:01:02:03".to_string(),
                    network_id: net_cloud1.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-02T10:00:00Z".to_string(),
            started_at: Some("2024-01-02T10:00:05Z".to_string()),
        });
        let vm_cloud2 = Uuid::new_v4().to_string();
        state.vms.insert(vm_cloud2.clone(), Vm {
            id: vm_cloud2.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-worker-01".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 8,
                memory_mb: 16384,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/harbor-worker-01".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:04:05:06".to_string(),
                    network_id: net_cloud3.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-03T12:00:00Z".to_string(),
            started_at: Some("2024-01-03T12:00:10Z".to_string()),
        });
        let vm_cloud3 = Uuid::new_v4().to_string();
        state.vms.insert(vm_cloud3.clone(), Vm {
            id: vm_cloud3.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-worker-02".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 8,
                memory_mb: 16384,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/harbor-worker-02".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:07:08:09".to_string(),
                    network_id: net_cloud3.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-03T12:30:00Z".to_string(),
            started_at: Some("2024-01-03T12:30:10Z".to_string()),
        });
        let vm_cloud4 = Uuid::new_v4().to_string();
        state.vms.insert(vm_cloud4.clone(), Vm {
            id: vm_cloud4.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-etcd".to_string(),
            state: VmState::Running,
            config: VmConfig {
                vcpus: 2,
                memory_mb: 4096,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/harbor-etcd".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:0a:0b:0c".to_string(),
                    network_id: net_cloud1.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-02T09:00:00Z".to_string(),
            started_at: Some("2024-01-02T09:00:03Z".to_string()),
        });
        let vm_cloud5 = Uuid::new_v4().to_string();
        state.vms.insert(vm_cloud5.clone(), Vm {
            id: vm_cloud5.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-vpn-gateway".to_string(),
            state: VmState::Stopped,
            config: VmConfig {
                vcpus: 2,
                memory_mb: 2048,
                kernel_path: Some("/var/lib/mvirt/kernels/vmlinux".to_string()),
                boot_disk: None,
                disks: vec![DiskConfig {
                    path: "/dev/zvol/tank/vm/harbor-vpn".to_string(),
                    readonly: false,
                }],
                nics: vec![NicConfig {
                    mac_address: "52:54:00:0d:0e:0f".to_string(),
                    network_id: net_cloud4.clone(),
                }],
                user_data: None,
            },
            created_at: "2024-01-15T14:00:00Z".to_string(),
            started_at: None,
        });

        // ===== VOLUMES =====
        // neonwave volumes (4)
        let vol_neon1 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_neon1.clone(), Volume {
            id: vol_neon1.clone(),
            project_id: proj1_id.clone(),
            name: "neon-api-root".to_string(),
            path: "tank/vm/neon-api-01".to_string(),
            volsize_bytes: 50 * 1024 * 1024 * 1024,
            used_bytes: 12 * 1024 * 1024 * 1024,
            compression_ratio: 1.8,
            snapshots: vec![Snapshot {
                id: Uuid::new_v4().to_string(),
                name: "pre-deploy-v2".to_string(),
                created_at: "2024-01-14T09:00:00Z".to_string(),
                used_bytes: 500 * 1024 * 1024,
            }],
        });
        let vol_neon2 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_neon2.clone(), Volume {
            id: vol_neon2.clone(),
            project_id: proj1_id.clone(),
            name: "neon-db-data".to_string(),
            path: "tank/vm/neon-db-data".to_string(),
            volsize_bytes: 500 * 1024 * 1024 * 1024,
            used_bytes: 180 * 1024 * 1024 * 1024,
            compression_ratio: 2.3,
            snapshots: vec![
                Snapshot {
                    id: Uuid::new_v4().to_string(),
                    name: "hourly-2024-01-20-12".to_string(),
                    created_at: "2024-01-20T12:00:00Z".to_string(),
                    used_bytes: 2 * 1024 * 1024 * 1024,
                },
                Snapshot {
                    id: Uuid::new_v4().to_string(),
                    name: "hourly-2024-01-20-13".to_string(),
                    created_at: "2024-01-20T13:00:00Z".to_string(),
                    used_bytes: 1 * 1024 * 1024 * 1024,
                },
            ],
        });
        let vol_neon3 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_neon3.clone(), Volume {
            id: vol_neon3.clone(),
            project_id: proj1_id.clone(),
            name: "neon-encoder-scratch".to_string(),
            path: "tank/vm/neon-encoder".to_string(),
            volsize_bytes: 1024 * 1024 * 1024 * 1024, // 1TB
            used_bytes: 650 * 1024 * 1024 * 1024,
            compression_ratio: 1.1,
            snapshots: vec![],
        });
        let vol_neon4 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_neon4.clone(), Volume {
            id: vol_neon4.clone(),
            project_id: proj1_id.clone(),
            name: "neon-media-archive".to_string(),
            path: "tank/media/archive".to_string(),
            volsize_bytes: 2 * 1024 * 1024 * 1024 * 1024, // 2TB
            used_bytes: 1400 * 1024 * 1024 * 1024,
            compression_ratio: 1.05,
            snapshots: vec![],
        });

        // pixelforge volumes (3)
        let vol_pixel1 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_pixel1.clone(), Volume {
            id: vol_pixel1.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-web-root".to_string(),
            path: "tank/vm/pixel-web".to_string(),
            volsize_bytes: 100 * 1024 * 1024 * 1024,
            used_bytes: 35 * 1024 * 1024 * 1024,
            compression_ratio: 1.9,
            snapshots: vec![],
        });
        let vol_pixel2 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_pixel2.clone(), Volume {
            id: vol_pixel2.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-render-cache".to_string(),
            path: "tank/vm/pixel-render".to_string(),
            volsize_bytes: 500 * 1024 * 1024 * 1024,
            used_bytes: 420 * 1024 * 1024 * 1024,
            compression_ratio: 1.2,
            snapshots: vec![],
        });
        let vol_pixel3 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_pixel3.clone(), Volume {
            id: vol_pixel3.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-assets".to_string(),
            path: "tank/data/pixel-assets".to_string(),
            volsize_bytes: 200 * 1024 * 1024 * 1024,
            used_bytes: 145 * 1024 * 1024 * 1024,
            compression_ratio: 1.4,
            snapshots: vec![
                Snapshot {
                    id: Uuid::new_v4().to_string(),
                    name: "release-v3.0".to_string(),
                    created_at: "2024-01-18T00:00:00Z".to_string(),
                    used_bytes: 5 * 1024 * 1024 * 1024,
                },
            ],
        });

        // cloudharbor volumes (5)
        let vol_cloud1 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_cloud1.clone(), Volume {
            id: vol_cloud1.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-ctrl-root".to_string(),
            path: "tank/vm/harbor-ctrl".to_string(),
            volsize_bytes: 50 * 1024 * 1024 * 1024,
            used_bytes: 18 * 1024 * 1024 * 1024,
            compression_ratio: 2.0,
            snapshots: vec![],
        });
        let vol_cloud2 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_cloud2.clone(), Volume {
            id: vol_cloud2.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-etcd-data".to_string(),
            path: "tank/vm/harbor-etcd".to_string(),
            volsize_bytes: 20 * 1024 * 1024 * 1024,
            used_bytes: 8 * 1024 * 1024 * 1024,
            compression_ratio: 3.2,
            snapshots: vec![
                Snapshot {
                    id: Uuid::new_v4().to_string(),
                    name: "daily-2024-01-19".to_string(),
                    created_at: "2024-01-19T00:00:00Z".to_string(),
                    used_bytes: 500 * 1024 * 1024,
                },
                Snapshot {
                    id: Uuid::new_v4().to_string(),
                    name: "daily-2024-01-20".to_string(),
                    created_at: "2024-01-20T00:00:00Z".to_string(),
                    used_bytes: 400 * 1024 * 1024,
                },
            ],
        });
        let vol_cloud3 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_cloud3.clone(), Volume {
            id: vol_cloud3.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-registry".to_string(),
            path: "tank/data/harbor-registry".to_string(),
            volsize_bytes: 500 * 1024 * 1024 * 1024,
            used_bytes: 320 * 1024 * 1024 * 1024,
            compression_ratio: 1.6,
            snapshots: vec![],
        });
        let vol_cloud4 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_cloud4.clone(), Volume {
            id: vol_cloud4.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-logs".to_string(),
            path: "tank/data/harbor-logs".to_string(),
            volsize_bytes: 100 * 1024 * 1024 * 1024,
            used_bytes: 75 * 1024 * 1024 * 1024,
            compression_ratio: 4.5,
            snapshots: vec![],
        });
        let vol_cloud5 = Uuid::new_v4().to_string();
        state.volumes.insert(vol_cloud5.clone(), Volume {
            id: vol_cloud5.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-backups".to_string(),
            path: "tank/backup/harbor".to_string(),
            volsize_bytes: 1024 * 1024 * 1024 * 1024, // 1TB
            used_bytes: 450 * 1024 * 1024 * 1024,
            compression_ratio: 2.8,
            snapshots: vec![],
        });

        // Create mock templates
        let tpl1_id = Uuid::new_v4().to_string();
        state.templates.insert(tpl1_id.clone(), Template {
            id: tpl1_id,
            name: "ubuntu-22.04".to_string(),
            size_bytes: 3 * 1024 * 1024 * 1024,
            clone_count: 5,
        });
        let tpl2_id = Uuid::new_v4().to_string();
        state.templates.insert(tpl2_id.clone(), Template {
            id: tpl2_id,
            name: "debian-12".to_string(),
            size_bytes: 2 * 1024 * 1024 * 1024,
            clone_count: 2,
        });
        let tpl3_id = Uuid::new_v4().to_string();
        state.templates.insert(tpl3_id.clone(), Template {
            id: tpl3_id,
            name: "alpine-3.19".to_string(),
            size_bytes: 150 * 1024 * 1024,
            clone_count: 8,
        });

        // ===== NICs =====
        // neonwave NICs (4)
        let nic_neon1 = Uuid::new_v4().to_string();
        state.nics.insert(nic_neon1.clone(), Nic {
            id: nic_neon1,
            project_id: proj1_id.clone(),
            name: "neon-api-eth0".to_string(),
            mac_address: "52:54:00:12:34:56".to_string(),
            network_id: net_neon1.clone(),
            vm_id: Some(vm_neon1.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.0.0.10".to_string()),
            ipv6_address: Some("fd00::10".to_string()),
        });
        let nic_neon2 = Uuid::new_v4().to_string();
        state.nics.insert(nic_neon2.clone(), Nic {
            id: nic_neon2,
            project_id: proj1_id.clone(),
            name: "neon-db-eth0".to_string(),
            mac_address: "52:54:00:ab:cd:ef".to_string(),
            network_id: net_neon2.clone(),
            vm_id: Some(vm_neon2.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.0.1.20".to_string()),
            ipv6_address: None,
        });
        let nic_neon3 = Uuid::new_v4().to_string();
        state.nics.insert(nic_neon3.clone(), Nic {
            id: nic_neon3,
            project_id: proj1_id.clone(),
            name: "neon-encoder-eth0".to_string(),
            mac_address: "52:54:00:11:22:33".to_string(),
            network_id: net_neon3.clone(),
            vm_id: Some(vm_neon3.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.0.2.30".to_string()),
            ipv6_address: Some("fd00:2::30".to_string()),
        });
        let nic_neon4 = Uuid::new_v4().to_string();
        state.nics.insert(nic_neon4.clone(), Nic {
            id: nic_neon4,
            project_id: proj1_id.clone(),
            name: "neon-spare".to_string(),
            mac_address: "52:54:00:44:55:66".to_string(),
            network_id: net_neon1.clone(),
            vm_id: None,
            state: NicState::Detached,
            ipv4_address: None,
            ipv6_address: None,
        });

        // pixelforge NICs (3)
        let nic_pixel1 = Uuid::new_v4().to_string();
        state.nics.insert(nic_pixel1.clone(), Nic {
            id: nic_pixel1,
            project_id: proj2_id.clone(),
            name: "pixel-web-eth0".to_string(),
            mac_address: "52:54:00:77:88:99".to_string(),
            network_id: net_pixel1.clone(),
            vm_id: Some(vm_pixel1.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.1.0.10".to_string()),
            ipv6_address: Some("fd01::10".to_string()),
        });
        let nic_pixel2 = Uuid::new_v4().to_string();
        state.nics.insert(nic_pixel2.clone(), Nic {
            id: nic_pixel2,
            project_id: proj2_id.clone(),
            name: "pixel-render-eth0".to_string(),
            mac_address: "52:54:00:aa:bb:cc".to_string(),
            network_id: net_pixel2.clone(),
            vm_id: Some(vm_pixel2.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.1.1.20".to_string()),
            ipv6_address: None,
        });
        let nic_pixel3 = Uuid::new_v4().to_string();
        state.nics.insert(nic_pixel3.clone(), Nic {
            id: nic_pixel3,
            project_id: proj2_id.clone(),
            name: "pixel-collab-eth0".to_string(),
            mac_address: "52:54:00:dd:ee:ff".to_string(),
            network_id: net_pixel1.clone(),
            vm_id: Some(vm_pixel3.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.1.0.30".to_string()),
            ipv6_address: Some("fd01::30".to_string()),
        });

        // cloudharbor NICs (5)
        let nic_cloud1 = Uuid::new_v4().to_string();
        state.nics.insert(nic_cloud1.clone(), Nic {
            id: nic_cloud1,
            project_id: proj3_id.clone(),
            name: "harbor-ctrl-eth0".to_string(),
            mac_address: "52:54:00:01:02:03".to_string(),
            network_id: net_cloud1.clone(),
            vm_id: Some(vm_cloud1.clone()),
            state: NicState::Attached,
            ipv4_address: Some("192.168.0.10".to_string()),
            ipv6_address: Some("fd02::10".to_string()),
        });
        let nic_cloud2 = Uuid::new_v4().to_string();
        state.nics.insert(nic_cloud2.clone(), Nic {
            id: nic_cloud2,
            project_id: proj3_id.clone(),
            name: "harbor-worker1-eth0".to_string(),
            mac_address: "52:54:00:04:05:06".to_string(),
            network_id: net_cloud3.clone(),
            vm_id: Some(vm_cloud2.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.244.0.20".to_string()),
            ipv6_address: None,
        });
        let nic_cloud3 = Uuid::new_v4().to_string();
        state.nics.insert(nic_cloud3.clone(), Nic {
            id: nic_cloud3,
            project_id: proj3_id.clone(),
            name: "harbor-worker2-eth0".to_string(),
            mac_address: "52:54:00:07:08:09".to_string(),
            network_id: net_cloud3.clone(),
            vm_id: Some(vm_cloud3.clone()),
            state: NicState::Attached,
            ipv4_address: Some("10.244.0.21".to_string()),
            ipv6_address: None,
        });
        let nic_cloud4 = Uuid::new_v4().to_string();
        state.nics.insert(nic_cloud4.clone(), Nic {
            id: nic_cloud4,
            project_id: proj3_id.clone(),
            name: "harbor-etcd-eth0".to_string(),
            mac_address: "52:54:00:0a:0b:0c".to_string(),
            network_id: net_cloud1.clone(),
            vm_id: Some(vm_cloud4.clone()),
            state: NicState::Attached,
            ipv4_address: Some("192.168.0.11".to_string()),
            ipv6_address: Some("fd02::11".to_string()),
        });
        let nic_cloud5 = Uuid::new_v4().to_string();
        state.nics.insert(nic_cloud5.clone(), Nic {
            id: nic_cloud5,
            project_id: proj3_id.clone(),
            name: "harbor-vpn-eth0".to_string(),
            mac_address: "52:54:00:0d:0e:0f".to_string(),
            network_id: net_cloud4.clone(),
            vm_id: None,
            state: NicState::Detached,
            ipv4_address: None,
            ipv6_address: None,
        });

        // ===== PODS =====
        // neonwave pods (3)
        let pod_neon1 = Uuid::new_v4().to_string();
        state.pods.insert(pod_neon1.clone(), Pod {
            id: pod_neon1.clone(),
            project_id: proj1_id.clone(),
            name: "neon-web-frontend".to_string(),
            state: PodState::Running,
            network_id: net_neon1.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "nginx".to_string(),
                    state: ContainerState::Running,
                    image: "nginx:1.25-alpine".to_string(),
                    exit_code: None,
                    error_message: None,
                },
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "react-app".to_string(),
                    state: ContainerState::Running,
                    image: "neonwave/frontend:v2.3.1".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("10.0.0.100".to_string()),
            created_at: "2024-01-15T11:00:00Z".to_string(),
            started_at: Some("2024-01-15T11:00:05Z".to_string()),
            error_message: None,
        });
        let pod_neon2 = Uuid::new_v4().to_string();
        state.pods.insert(pod_neon2.clone(), Pod {
            id: pod_neon2.clone(),
            project_id: proj1_id.clone(),
            name: "neon-audio-processor".to_string(),
            state: PodState::Running,
            network_id: net_neon3.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "ffmpeg-worker".to_string(),
                    state: ContainerState::Running,
                    image: "neonwave/audio-worker:v1.8.0".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("10.0.2.100".to_string()),
            created_at: "2024-01-12T14:30:00Z".to_string(),
            started_at: Some("2024-01-12T14:30:08Z".to_string()),
            error_message: None,
        });
        let pod_neon3 = Uuid::new_v4().to_string();
        state.pods.insert(pod_neon3.clone(), Pod {
            id: pod_neon3.clone(),
            project_id: proj1_id.clone(),
            name: "neon-recommendation".to_string(),
            state: PodState::Stopped,
            network_id: net_neon1.clone(),
            vm_id: None,
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "ml-inference".to_string(),
                    state: ContainerState::Stopped,
                    image: "neonwave/ml-rec:v0.9.2".to_string(),
                    exit_code: Some(0),
                    error_message: None,
                },
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "redis".to_string(),
                    state: ContainerState::Stopped,
                    image: "redis:7-alpine".to_string(),
                    exit_code: Some(0),
                    error_message: None,
                },
            ],
            ip_address: None,
            created_at: "2024-01-10T09:00:00Z".to_string(),
            started_at: None,
            error_message: None,
        });

        // pixelforge pods (2)
        let pod_pixel1 = Uuid::new_v4().to_string();
        state.pods.insert(pod_pixel1.clone(), Pod {
            id: pod_pixel1.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-websocket-hub".to_string(),
            state: PodState::Running,
            network_id: net_pixel1.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "ws-server".to_string(),
                    state: ContainerState::Running,
                    image: "pixelforge/ws-hub:v3.1.0".to_string(),
                    exit_code: None,
                    error_message: None,
                },
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "redis".to_string(),
                    state: ContainerState::Running,
                    image: "redis:7-alpine".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("10.1.0.100".to_string()),
            created_at: "2024-01-08T12:00:00Z".to_string(),
            started_at: Some("2024-01-08T12:00:06Z".to_string()),
            error_message: None,
        });
        let pod_pixel2 = Uuid::new_v4().to_string();
        state.pods.insert(pod_pixel2.clone(), Pod {
            id: pod_pixel2.clone(),
            project_id: proj2_id.clone(),
            name: "pixel-asset-processor".to_string(),
            state: PodState::Running,
            network_id: net_pixel2.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "imagemagick".to_string(),
                    state: ContainerState::Running,
                    image: "pixelforge/asset-proc:v2.0.0".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("10.1.1.100".to_string()),
            created_at: "2024-01-06T10:00:00Z".to_string(),
            started_at: Some("2024-01-06T10:00:04Z".to_string()),
            error_message: None,
        });

        // cloudharbor pods (4)
        let pod_cloud1 = Uuid::new_v4().to_string();
        state.pods.insert(pod_cloud1.clone(), Pod {
            id: pod_cloud1.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-api-gateway".to_string(),
            state: PodState::Running,
            network_id: net_cloud1.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "envoy".to_string(),
                    state: ContainerState::Running,
                    image: "envoyproxy/envoy:v1.28".to_string(),
                    exit_code: None,
                    error_message: None,
                },
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "auth-sidecar".to_string(),
                    state: ContainerState::Running,
                    image: "cloudharbor/auth:v1.2.0".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("192.168.0.100".to_string()),
            created_at: "2024-01-02T11:00:00Z".to_string(),
            started_at: Some("2024-01-02T11:00:07Z".to_string()),
            error_message: None,
        });
        let pod_cloud2 = Uuid::new_v4().to_string();
        state.pods.insert(pod_cloud2.clone(), Pod {
            id: pod_cloud2.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-metrics".to_string(),
            state: PodState::Running,
            network_id: net_cloud1.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "prometheus".to_string(),
                    state: ContainerState::Running,
                    image: "prom/prometheus:v2.48".to_string(),
                    exit_code: None,
                    error_message: None,
                },
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "grafana".to_string(),
                    state: ContainerState::Running,
                    image: "grafana/grafana:10.2".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("192.168.0.101".to_string()),
            created_at: "2024-01-02T11:30:00Z".to_string(),
            started_at: Some("2024-01-02T11:30:12Z".to_string()),
            error_message: None,
        });
        let pod_cloud3 = Uuid::new_v4().to_string();
        state.pods.insert(pod_cloud3.clone(), Pod {
            id: pod_cloud3.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-ci-runner".to_string(),
            state: PodState::Failed,
            network_id: net_cloud3.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "runner".to_string(),
                    state: ContainerState::Failed,
                    image: "cloudharbor/runner:v0.8.0".to_string(),
                    exit_code: Some(137),
                    error_message: Some("OOMKilled".to_string()),
                },
            ],
            ip_address: Some("10.244.0.100".to_string()),
            created_at: "2024-01-19T16:00:00Z".to_string(),
            started_at: Some("2024-01-19T16:00:05Z".to_string()),
            error_message: Some("Container runner was OOMKilled".to_string()),
        });
        let pod_cloud4 = Uuid::new_v4().to_string();
        state.pods.insert(pod_cloud4.clone(), Pod {
            id: pod_cloud4.clone(),
            project_id: proj3_id.clone(),
            name: "harbor-registry-cache".to_string(),
            state: PodState::Running,
            network_id: net_cloud2.clone(),
            vm_id: Some(Uuid::new_v4().to_string()),
            containers: vec![
                Container {
                    id: Uuid::new_v4().to_string(),
                    name: "registry".to_string(),
                    state: ContainerState::Running,
                    image: "registry:2".to_string(),
                    exit_code: None,
                    error_message: None,
                },
            ],
            ip_address: Some("192.168.1.100".to_string()),
            created_at: "2024-01-03T09:00:00Z".to_string(),
            started_at: Some("2024-01-03T09:00:03Z".to_string()),
            error_message: None,
        });

        // Create mock logs - multiple per project
        let now = chrono::Utc::now();

        // neonwave logs
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj1_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(60))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "VM neon-db-primary started".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_neon2.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj1_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(45))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "VM neon-api-01 started".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_neon1.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj1_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(30))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Pod neon-web-frontend started".to_string(),
            level: LogLevel::Audit,
            component: "pod".to_string(),
            related_object_ids: vec![pod_neon1.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj1_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(10))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Snapshot 'hourly-2024-01-20-13' created for volume neon-db-data".to_string(),
            level: LogLevel::Audit,
            component: "zfs".to_string(),
            related_object_ids: vec![vol_neon2.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj1_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(5))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Audio encoder scaling up to handle peak traffic".to_string(),
            level: LogLevel::Info,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_neon3.clone()],
        });

        // pixelforge logs
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj2_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(120))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "VM pixel-render-node started".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_pixel2.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj2_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(90))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Pod pixel-websocket-hub started".to_string(),
            level: LogLevel::Audit,
            component: "pod".to_string(),
            related_object_ids: vec![pod_pixel1.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj2_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(20))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Render job completed: 1847 frames processed".to_string(),
            level: LogLevel::Info,
            component: "pod".to_string(),
            related_object_ids: vec![pod_pixel2.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj2_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(2))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "VM pixel-collab-server starting".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_pixel3.clone()],
        });

        // cloudharbor logs
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj3_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(180))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Cluster initialized with 2 worker nodes".to_string(),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_cloud1.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj3_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(60))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Snapshot 'daily-2024-01-20' created for etcd".to_string(),
            level: LogLevel::Audit,
            component: "zfs".to_string(),
            related_object_ids: vec![vol_cloud2.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj3_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(25))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Pod harbor-ci-runner failed: OOMKilled".to_string(),
            level: LogLevel::Error,
            component: "pod".to_string(),
            related_object_ids: vec![pod_cloud3.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj3_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(15))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Registry cache hit ratio: 94.2%".to_string(),
            level: LogLevel::Info,
            component: "pod".to_string(),
            related_object_ids: vec![pod_cloud4.clone()],
        });
        state.logs.push(LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: proj3_id.clone(),
            timestamp_ns: (now - chrono::Duration::minutes(3))
                .timestamp_nanos_opt()
                .unwrap_or(0),
            message: "Worker node harbor-worker-01 health check passed".to_string(),
            level: LogLevel::Debug,
            component: "vmm".to_string(),
            related_object_ids: vec![vm_cloud2.clone()],
        });
    }
}
