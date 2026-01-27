//! Pod Service - gRPC service for managing container pods in MicroVMs.

use crate::hypervisor::Hypervisor;
use crate::proto::{
    BootMode, Container, ContainerSpec, ContainerState, CreatePodRequest, DeletePodRequest,
    DeletePodResponse, DiskConfig, GetPodRequest, ListPodsRequest, ListPodsResponse, LogChunk,
    NicConfig, Pod, PodExecInput, PodExecOutput, PodLogsRequest, PodResources, PodState,
    StartPodRequest, StopPodRequest, VmConfig, pod_service_server::PodService,
};
use crate::store::VmStore;
use crate::vsock_client::UosClient;
use mvirt_log::AuditLogger;
use mvirt_one::proto::{StopPodRequest as OneStopPodRequest, uos_service_client::UosServiceClient};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Kernel path for MicroVM boot.
const UOS_KERNEL_PATH: &str = "/usr/share/mvirt/one/bzImage";
/// Rootfs template path for pod volumes.
const UOS_ROOTFS_TEMPLATE: &str = "/usr/share/mvirt/one/rootfs.raw";
/// Cmdline file path.
const UOS_CMDLINE_PATH: &str = "/usr/share/mvirt/one/cmdline";
/// Default kernel command line for MicroVM (disk boot).
const UOS_DISK_CMDLINE: &str = "console=ttyS0 quiet root=/dev/vda rw init=/init";
/// Default memory for pod MicroVMs (MB).
const POD_DEFAULT_MEMORY_MB: u64 = 256;
/// Default vCPUs for pod MicroVMs.
const POD_DEFAULT_VCPUS: u32 = 1;

/// Internal pod data stored by the service.
#[derive(Debug, Clone)]
struct PodData {
    id: String,
    name: String,
    state: PodState,
    vm_id: Option<String>,
    containers: Vec<ContainerSpec>,
    resources: Option<PodResources>,
    /// Path to root disk volume (ZFS volume created by CLI, rootfs written by VMM).
    root_disk_path: Option<String>,
    /// vhost-user socket path for NIC (from mvirt-net).
    nic_socket_path: Option<String>,
    ip_address: String,
    created_at: i64,
    started_at: Option<i64>,
    error_message: Option<String>,
}

/// Read kernel cmdline from file, falling back to default.
fn read_uos_cmdline() -> String {
    std::fs::read_to_string(UOS_CMDLINE_PATH)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| UOS_DISK_CMDLINE.to_string())
}

impl From<PodData> for Pod {
    fn from(data: PodData) -> Self {
        Pod {
            id: data.id,
            name: data.name,
            state: data.state.into(),
            vm_id: data.vm_id.unwrap_or_default(),
            containers: data
                .containers
                .into_iter()
                .map(|spec| Container {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    state: ContainerState::Unspecified.into(),
                    image: spec.image.clone(),
                    exit_code: 0,
                    error_message: None,
                })
                .collect(),
            ip_address: data.ip_address,
            created_at: data.created_at,
            started_at: data.started_at,
            error_message: data.error_message,
        }
    }
}

/// gRPC implementation of the Pod Service.
pub struct PodServiceImpl {
    #[allow(dead_code)]
    store: Arc<VmStore>,
    hypervisor: Arc<Hypervisor>,
    audit: Arc<AuditLogger>,
    pods: Arc<RwLock<HashMap<String, PodData>>>,
    /// Map of pod_id -> UosClient for communicating with MicroVMs
    uos_clients: Arc<RwLock<HashMap<String, UosClient>>>,
}

impl PodServiceImpl {
    /// Create a new Pod Service.
    pub fn new(store: Arc<VmStore>, hypervisor: Arc<Hypervisor>, audit: Arc<AuditLogger>) -> Self {
        Self {
            store,
            hypervisor,
            audit,
            pods: Arc::new(RwLock::new(HashMap::new())),
            uos_clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[tonic::async_trait]
impl PodService for PodServiceImpl {
    async fn create_pod(
        &self,
        request: Request<CreatePodRequest>,
    ) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        let pod_id = Uuid::new_v4().to_string();
        let name = req.name.unwrap_or_else(|| format!("pod-{}", &pod_id[..8]));

        info!(pod_id = %pod_id, name = %name, "Creating pod");

        // Validate containers
        if req.containers.is_empty() {
            return Err(Status::invalid_argument(
                "At least one container is required",
            ));
        }

        // Assign IDs to containers if not provided
        let containers: Vec<ContainerSpec> = req
            .containers
            .into_iter()
            .map(|mut spec| {
                if spec.id.is_empty() {
                    spec.id = Uuid::new_v4().to_string();
                }
                spec
            })
            .collect();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let pod_data = PodData {
            id: pod_id.clone(),
            name: name.clone(),
            state: PodState::Created,
            vm_id: None,
            containers,
            resources: req.resources,
            root_disk_path: req.root_disk_path,
            nic_socket_path: req.nic_socket_path,
            ip_address: String::new(),
            created_at: now,
            started_at: None,
            error_message: None,
        };

        // Store pod
        {
            let mut pods = self.pods.write().await;
            pods.insert(pod_id.clone(), pod_data.clone());
        }

        self.audit
            .log(
                mvirt_log::LogLevel::Audit,
                &format!("Pod {} ({}) created", name, pod_id),
                vec![pod_id.clone()],
            )
            .await;

        Ok(Response::new(pod_data.into()))
    }

    async fn get_pod(&self, request: Request<GetPodRequest>) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        let pods = self.pods.read().await;

        let pod = pods
            .get(&req.id)
            .ok_or_else(|| Status::not_found(format!("Pod {} not found", req.id)))?;

        Ok(Response::new(pod.clone().into()))
    }

    async fn list_pods(
        &self,
        _request: Request<ListPodsRequest>,
    ) -> Result<Response<ListPodsResponse>, Status> {
        let pods = self.pods.read().await;
        let pod_list: Vec<Pod> = pods.values().cloned().map(|p| p.into()).collect();

        Ok(Response::new(ListPodsResponse { pods: pod_list }))
    }

    async fn delete_pod(
        &self,
        request: Request<DeletePodRequest>,
    ) -> Result<Response<DeletePodResponse>, Status> {
        let req = request.into_inner();
        info!(pod_id = %req.id, force = req.force, "Deleting pod");

        let mut pods = self.pods.write().await;
        let pod = pods
            .get(&req.id)
            .ok_or_else(|| Status::not_found(format!("Pod {} not found", req.id)))?;

        // Check state
        if pod.state == PodState::Running && !req.force {
            return Err(Status::failed_precondition(
                "Pod is running. Use force=true to delete anyway",
            ));
        }

        // Stop the MicroVM if running
        if let Some(vm_id) = &pod.vm_id {
            // Remove uos client
            let mut clients = self.uos_clients.write().await;
            clients.remove(&req.id);

            // Kill the MicroVM
            if let Err(e) = self.hypervisor.kill(vm_id).await {
                warn!(vm_id = %vm_id, error = %e, "Failed to kill VM for pod");
            }
        }

        // Delete the VM entry from database (pod_id == vm_id)
        if let Err(e) = self.store.delete(&req.id).await {
            warn!(pod_id = %req.id, error = %e, "Failed to delete VM entry for pod");
        }

        // Note: ZFS volume cleanup is the CLI's responsibility

        let pod_name = pod.name.clone();
        pods.remove(&req.id);

        self.audit
            .log(
                mvirt_log::LogLevel::Audit,
                &format!("Pod {} ({}) deleted", pod_name, req.id),
                vec![req.id.clone()],
            )
            .await;

        Ok(Response::new(DeletePodResponse {}))
    }

    async fn start_pod(&self, request: Request<StartPodRequest>) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        info!(pod_id = %req.id, "Starting pod");

        // Get pod data (we need to clone to avoid holding the lock)
        let (pod_id, pod_name, _containers, resources, root_disk_path, nic_socket_path) = {
            let mut pods = self.pods.write().await;
            let pod = pods
                .get_mut(&req.id)
                .ok_or_else(|| Status::not_found(format!("Pod {} not found", req.id)))?;

            // Check state
            if pod.state == PodState::Running {
                return Err(Status::failed_precondition("Pod is already running"));
            }

            pod.state = PodState::Starting;
            (
                pod.id.clone(),
                pod.name.clone(),
                pod.containers.clone(),
                pod.resources,
                pod.root_disk_path.clone(),
                pod.nic_socket_path.clone(),
            )
        };

        // Root disk is required (created by CLI via mvirt-zfs)
        let root_disk_path = match root_disk_path {
            Some(path) => path,
            None => {
                error!(pod_id = %pod_id, "No root disk path provided");
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message =
                        Some("No root disk path provided. Create volume first.".to_string());
                }
                return Err(Status::failed_precondition(
                    "No root disk path provided. Create volume with CLI first.",
                ));
            }
        };

        // Write rootfs template to volume
        info!(pod_id = %pod_id, volume = %root_disk_path, "Writing rootfs template to volume");
        let dd_status = std::process::Command::new("dd")
            .args([
                &format!("if={}", UOS_ROOTFS_TEMPLATE),
                &format!("of={}", root_disk_path),
                "bs=4M",
                "conv=fsync",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match dd_status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                error!(pod_id = %pod_id, "dd failed with exit code {:?}", status.code());
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message =
                        Some("Failed to write rootfs template to volume".to_string());
                }
                return Err(Status::internal(
                    "Failed to write rootfs template to volume",
                ));
            }
            Err(e) => {
                error!(pod_id = %pod_id, error = %e, "Failed to execute dd");
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("Failed to execute dd: {}", e));
                }
                return Err(Status::internal(format!("Failed to execute dd: {}", e)));
            }
        }

        // Check and resize filesystem to fill volume
        info!(pod_id = %pod_id, volume = %root_disk_path, "Checking and resizing filesystem");

        // Run e2fsck first (required before resize2fs)
        let _ = std::process::Command::new("e2fsck")
            .args(["-f", "-y", &root_disk_path])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        // Now resize the filesystem
        let resize_status = std::process::Command::new("resize2fs")
            .arg(&root_disk_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match resize_status {
            Ok(status) if !status.success() => {
                warn!(pod_id = %pod_id, "resize2fs failed with exit code {:?} (continuing anyway)", status.code());
            }
            Err(e) => {
                warn!(pod_id = %pod_id, error = %e, "Failed to resize filesystem (continuing anyway)");
            }
            _ => {}
        }

        // Determine resource values
        let vcpus = resources
            .as_ref()
            .map(|r| {
                if r.vcpus > 0 {
                    r.vcpus
                } else {
                    POD_DEFAULT_VCPUS
                }
            })
            .unwrap_or(POD_DEFAULT_VCPUS);
        let memory_mb = resources
            .as_ref()
            .map(|r| {
                if r.memory_mb > 0 {
                    r.memory_mb
                } else {
                    POD_DEFAULT_MEMORY_MB
                }
            })
            .unwrap_or(POD_DEFAULT_MEMORY_MB);

        // Read kernel cmdline
        let cmdline = read_uos_cmdline();

        // Build NIC config if socket path provided
        let nics = if let Some(socket_path) = nic_socket_path {
            vec![NicConfig {
                tap: None,
                mac: None, // auto-generated
                vhost_socket: Some(socket_path),
            }]
        } else {
            vec![]
        };

        // Create VM config for kernel boot with disk
        let vm_config = VmConfig {
            vcpus,
            memory_mb,
            boot_mode: BootMode::Kernel.into(),
            kernel: Some(UOS_KERNEL_PATH.to_string()),
            initramfs: None, // No initramfs - boot from disk
            cmdline: Some(cmdline),
            disks: vec![DiskConfig {
                path: root_disk_path.clone(),
                readonly: false,
            }],
            nics,
            user_data: None,
            nested_virt: false,
        };

        // Create a VM entry in the database (so console works via standard VM API)
        // Mark as microvm=true so it doesn't show up in ListVms
        let vm_entry = match self
            .store
            .create_microvm(&pod_id, Some(pod_name.clone()), vm_config.clone())
            .await
        {
            Ok(entry) => entry,
            Err(e) => {
                error!(pod_id = %pod_id, error = %e, "Failed to create VM entry for pod");
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("Failed to create VM entry: {}", e));
                }
                return Err(Status::internal(format!(
                    "Failed to create VM entry: {}",
                    e
                )));
            }
        };

        let vm_id = vm_entry.id.clone();

        // Update VM state to starting
        self.store
            .update_state(&vm_id, crate::proto::VmState::Starting)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Start the MicroVM
        info!(vm_id = %vm_id, pod_id = %pod_id, "Starting MicroVM for pod");
        if let Err(e) = self
            .hypervisor
            .start(&vm_id, Some(&pod_name), &vm_config)
            .await
        {
            error!(vm_id = %vm_id, error = %e, "Failed to start MicroVM");
            // Revert VM state and clean up
            let _ = self
                .store
                .update_state(&vm_id, crate::proto::VmState::Stopped)
                .await;
            let mut pods = self.pods.write().await;
            if let Some(pod) = pods.get_mut(&pod_id) {
                pod.state = PodState::Failed;
                pod.error_message = Some(format!("Failed to start MicroVM: {}", e));
            }
            return Err(Status::internal(format!("Failed to start MicroVM: {}", e)));
        }

        // Update VM state to running
        self.store
            .update_state(&vm_id, crate::proto::VmState::Running)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        debug!(vm_id = %vm_id, pod_id = %pod_id, "MicroVM started");

        // Mark pod as running (uos communication will happen in background)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        {
            let mut pods = self.pods.write().await;
            if let Some(pod) = pods.get_mut(&pod_id) {
                pod.state = PodState::Running;
                pod.vm_id = Some(vm_id.clone());
                pod.started_at = Some(now);
                pod.error_message = None;
            }
        }

        self.audit
            .log(
                mvirt_log::LogLevel::Audit,
                &format!("Pod {} ({}) started", pod_name, pod_id),
                vec![pod_id.clone(), vm_id],
            )
            .await;

        let pods = self.pods.read().await;
        let pod = pods.get(&pod_id).cloned().unwrap();
        Ok(Response::new(pod.into()))
    }

    async fn stop_pod(&self, request: Request<StopPodRequest>) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        info!(pod_id = %req.id, "Stopping pod");

        // Get pod data and client (clone to avoid holding locks)
        let (pod_id, pod_name, vm_id) = {
            let mut pods = self.pods.write().await;
            let pod = pods
                .get_mut(&req.id)
                .ok_or_else(|| Status::not_found(format!("Pod {} not found", req.id)))?;

            // Check state
            if pod.state != PodState::Running {
                return Err(Status::failed_precondition("Pod is not running"));
            }

            pod.state = PodState::Stopping;
            (pod.id.clone(), pod.name.clone(), pod.vm_id.clone())
        };

        let timeout_secs = if req.timeout_seconds > 0 {
            req.timeout_seconds
        } else {
            10 // Default timeout
        };

        // Try to gracefully stop the pod via uos
        if let Some(ref vm_id) = vm_id {
            let mut clients = self.uos_clients.write().await;
            if let Some(uos_client) = clients.remove(&pod_id) {
                let mut uos = UosServiceClient::new(uos_client.channel());
                let stop_req = OneStopPodRequest {
                    id: pod_id.clone(),
                    timeout_seconds: timeout_secs,
                };

                debug!(pod_id = %pod_id, "Sending stop request to uos");
                if let Err(e) = uos.stop_pod(stop_req).await {
                    warn!(pod_id = %pod_id, error = %e, "Failed to stop pod via uos, will kill VM");
                }
            }

            // Stop the MicroVM
            let timeout = Duration::from_secs(timeout_secs.into());
            if let Err(e) = self.hypervisor.stop(vm_id, timeout).await {
                warn!(vm_id = %vm_id, error = %e, "Failed to stop VM for pod");
            }

            // Update VM state in database
            let _ = self
                .store
                .update_state(vm_id, crate::proto::VmState::Stopped)
                .await;
        }

        // Update pod state
        {
            let mut pods = self.pods.write().await;
            if let Some(pod) = pods.get_mut(&pod_id) {
                pod.state = PodState::Stopped;
                pod.vm_id = None;
            }
        }

        self.audit
            .log(
                mvirt_log::LogLevel::Audit,
                &format!("Pod {} ({}) stopped", pod_name, pod_id),
                vec![pod_id.clone()],
            )
            .await;

        let pods = self.pods.read().await;
        let pod = pods.get(&pod_id).cloned().unwrap();
        Ok(Response::new(pod.into()))
    }

    type PodLogsStream = ReceiverStream<Result<LogChunk, Status>>;

    async fn pod_logs(
        &self,
        _request: Request<PodLogsRequest>,
    ) -> Result<Response<Self::PodLogsStream>, Status> {
        Err(Status::unimplemented("Pod logs not yet implemented"))
    }

    type PodExecStream = ReceiverStream<Result<PodExecOutput, Status>>;

    async fn pod_exec(
        &self,
        _request: Request<Streaming<PodExecInput>>,
    ) -> Result<Response<Self::PodExecStream>, Status> {
        Err(Status::unimplemented("Pod exec not yet implemented"))
    }
}
