//! Pod Service - gRPC service for managing container pods in MicroVMs.

use crate::hypervisor::Hypervisor;
use crate::proto::{
    BootMode, Container, ContainerSpec, ContainerState, CreatePodRequest, DeletePodRequest,
    DeletePodResponse, DiskConfig, GetPodNetworkInfoRequest, GetPodRequest, ListPodsRequest,
    ListPodsResponse, LogChunk, NicConfig, Pod, PodExecInput, PodExecOutput, PodInterfaceInfo,
    PodLogsRequest, PodNetworkInfo, PodResources, PodState, StartPodRequest, StopPodRequest,
    VmConfig, pod_service_server::PodService,
};
use crate::ready_listener::ReadySignalListener;
use crate::store::VmStore;
use crate::vsock_client::{OneClient, vm_id_to_cid, vsock_socket_path};
use mvirt_log::AuditLogger;
use mvirt_one::proto::{
    ContainerSpec as OneContainerSpec, CreatePodRequest as OneCreatePodRequest, Empty as OneEmpty,
    StartPodRequest as OneStartPodRequest, StopPodRequest as OneStopPodRequest,
    one_service_client::OneServiceClient,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Kernel path for MicroVM boot.
const ONE_KERNEL_PATH: &str = "/usr/share/mvirt/one/bzImage";
/// Rootfs template path for pod volumes.
const ONE_ROOTFS_TEMPLATE: &str = "/usr/share/mvirt/one/rootfs.raw";
/// Cmdline file path.
const ONE_CMDLINE_PATH: &str = "/usr/share/mvirt/one/cmdline";
/// Default kernel command line for MicroVM (disk boot).
const ONE_DISK_CMDLINE: &str = "console=ttyS0 quiet root=/dev/vda rw init=/init";
/// Default memory for pod MicroVMs (MB).
const POD_DEFAULT_MEMORY_MB: u64 = 256;
/// Default vCPUs for pod MicroVMs.
const POD_DEFAULT_VCPUS: u32 = 1;
/// Timeout for waiting for mvirt-one to boot.
const ONE_BOOT_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// vhost-user socket path for NIC (from mvirt-net/mvirt-ebpf).
    nic_socket_path: Option<String>,
    /// MAC address for the NIC (required for DHCP to work).
    nic_mac_address: Option<String>,
    ip_address: String,
    created_at: i64,
    started_at: Option<i64>,
    error_message: Option<String>,
}

/// Read kernel cmdline from file, falling back to default.
fn read_one_cmdline() -> String {
    std::fs::read_to_string(ONE_CMDLINE_PATH)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| ONE_DISK_CMDLINE.to_string())
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
    /// Map of pod_id -> OneClient for communicating with MicroVMs
    one_clients: Arc<RwLock<HashMap<String, OneClient>>>,
}

impl PodServiceImpl {
    /// Create a new Pod Service.
    pub fn new(store: Arc<VmStore>, hypervisor: Arc<Hypervisor>, audit: Arc<AuditLogger>) -> Self {
        Self {
            store,
            hypervisor,
            audit,
            pods: Arc::new(RwLock::new(HashMap::new())),
            one_clients: Arc::new(RwLock::new(HashMap::new())),
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
            nic_mac_address: req.nic_mac_address,
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
            // Remove one client
            let mut clients = self.one_clients.write().await;
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
        let (
            pod_id,
            pod_name,
            _containers,
            resources,
            root_disk_path,
            nic_socket_path,
            nic_mac_address,
        ) = {
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
                pod.nic_mac_address.clone(),
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
        // Use conv=notrunc to preserve the volume size (important for raw files in /tmp for tests)
        info!(pod_id = %pod_id, volume = %root_disk_path, "Writing rootfs template to volume");
        let dd_status = std::process::Command::new("dd")
            .args([
                &format!("if={}", ONE_ROOTFS_TEMPLATE),
                &format!("of={}", root_disk_path),
                "bs=4M",
                "conv=fsync,notrunc",
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
        let cmdline = read_one_cmdline();

        // Build NIC config if socket path provided
        let nics = if let Some(socket_path) = nic_socket_path {
            vec![NicConfig {
                tap: None,
                mac: nic_mac_address, // Must match what mvirt-ebpf expects for DHCP
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
            kernel: Some(ONE_KERNEL_PATH.to_string()),
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

        // Calculate vsock CID for this pod
        let cid = vm_id_to_cid(&pod_id);
        info!(pod_id = %pod_id, cid = cid, "Calculated vsock CID");

        // Get vsock socket path for this VM
        let vsock_socket = vsock_socket_path(self.hypervisor.data_dir(), &pod_id);

        // Prepare VM directory before creating the ready listener
        if let Err(e) = self.hypervisor.prepare_vm_dir(&pod_id).await {
            error!(pod_id = %pod_id, error = %e, "Failed to prepare VM directory");
            let mut pods = self.pods.write().await;
            if let Some(pod) = pods.get_mut(&pod_id) {
                pod.state = PodState::Failed;
                pod.error_message = Some(format!("Failed to prepare VM directory: {}", e));
            }
            return Err(Status::internal(format!(
                "Failed to prepare VM directory: {}",
                e
            )));
        }

        // Create ready signal listener BEFORE starting the VM to avoid race condition
        // Cloud-hypervisor proxies guest→host connections to `<vsock_socket>_<port>`
        let ready_listener = match ReadySignalListener::new(&vsock_socket).await {
            Ok(l) => l,
            Err(e) => {
                error!(pod_id = %pod_id, error = %e, "Failed to create ready listener");
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("Failed to create ready listener: {}", e));
                }
                return Err(Status::internal(format!(
                    "Failed to create ready listener: {}",
                    e
                )));
            }
        };

        // Update VM state to starting
        self.store
            .update_state(&vm_id, crate::proto::VmState::Starting)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Start the MicroVM with vsock enabled
        info!(vm_id = %vm_id, pod_id = %pod_id, cid = cid, "Starting MicroVM for pod");
        if let Err(e) = self
            .hypervisor
            .start(&vm_id, Some(&pod_name), &vm_config, Some(cid))
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

        // Wait for mvirt-one to signal it's ready via vsock
        match ready_listener.wait(ONE_BOOT_TIMEOUT).await {
            Ok(()) => {
                info!(pod_id = %pod_id, cid = cid, "Received ready signal from mvirt-one");
            }
            Err(e) => {
                error!(pod_id = %pod_id, cid = cid, error = %e, "Failed waiting for ready signal");
                let _ = self.hypervisor.kill(&vm_id).await;
                let _ = self
                    .store
                    .update_state(&vm_id, crate::proto::VmState::Stopped)
                    .await;
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("Ready signal failed: {}", e));
                }
                return Err(Status::internal(format!("Ready signal failed: {}", e)));
            }
        }

        // Now connect to mvirt-one via vsock
        let one_client = match OneClient::connect(&vsock_socket).await {
            Ok(client) => {
                info!(pod_id = %pod_id, "Connected to mvirt-one via vsock");
                client
            }
            Err(e) => {
                error!(pod_id = %pod_id, error = %e, "Failed to connect to mvirt-one");
                let _ = self.hypervisor.kill(&vm_id).await;
                let _ = self
                    .store
                    .update_state(&vm_id, crate::proto::VmState::Stopped)
                    .await;
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("vsock connection failed: {}", e));
                }
                return Err(Status::internal(format!("vsock connection failed: {}", e)));
            }
        };

        // Store the one client for later use
        self.one_clients
            .write()
            .await
            .insert(pod_id.clone(), one_client);

        // Get the stored client's channel
        let channel = {
            let clients = self.one_clients.read().await;
            clients.get(&pod_id).map(|c| c.channel())
        };

        // Send CreatePod and StartPod commands to mvirt-one
        if let Some(channel) = channel {
            let mut one = OneServiceClient::new(channel);

            // Convert container specs to mvirt-one format
            let one_containers: Vec<OneContainerSpec> = _containers
                .iter()
                .map(|c| OneContainerSpec {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    image: c.image.clone(),
                    command: c.command.clone(),
                    args: c.args.clone(),
                    env: c.env.clone(),
                    working_dir: c.working_dir.clone(),
                })
                .collect();

            // Create pod in mvirt-one
            let create_req = OneCreatePodRequest {
                id: pod_id.clone(),
                name: pod_name.clone(),
                containers: one_containers,
            };

            match one.create_pod(create_req).await {
                Ok(_) => {
                    debug!(pod_id = %pod_id, "Pod created in mvirt-one");
                }
                Err(e) => {
                    warn!(pod_id = %pod_id, error = %e, "Failed to create pod in mvirt-one");
                }
            }

            // Start pod in mvirt-one
            let start_req = OneStartPodRequest { id: pod_id.clone() };

            match one.start_pod(start_req).await {
                Ok(_) => {
                    debug!(pod_id = %pod_id, "Pod started in mvirt-one");
                }
                Err(e) => {
                    warn!(pod_id = %pod_id, error = %e, "Failed to start pod in mvirt-one");
                    // Set error_message but don't fail - the VM is still running
                    let mut pods = self.pods.write().await;
                    if let Some(pod) = pods.get_mut(&pod_id) {
                        pod.error_message = Some(format!("Container start failed: {}", e));
                    }
                }
            }
        }

        // Mark pod as running
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

        // Try to gracefully stop the pod via one
        if let Some(ref vm_id) = vm_id {
            let mut clients = self.one_clients.write().await;
            if let Some(one_client) = clients.remove(&pod_id) {
                let mut one = OneServiceClient::new(one_client.channel());
                let stop_req = OneStopPodRequest {
                    id: pod_id.clone(),
                    timeout_seconds: timeout_secs,
                };

                debug!(pod_id = %pod_id, "Sending stop request to one");
                if let Err(e) = one.stop_pod(stop_req).await {
                    warn!(pod_id = %pod_id, error = %e, "Failed to stop pod via one, will kill VM");
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

    async fn get_pod_network_info(
        &self,
        request: Request<GetPodNetworkInfoRequest>,
    ) -> Result<Response<PodNetworkInfo>, Status> {
        let req = request.into_inner();
        info!(pod_id = %req.pod_id, "Getting pod network info");

        // Verify pod exists and is running
        let pod_id = {
            let pods = self.pods.read().await;
            let pod = pods
                .get(&req.pod_id)
                .ok_or_else(|| Status::not_found(format!("Pod {} not found", req.pod_id)))?;

            if pod.state != PodState::Running {
                return Err(Status::failed_precondition("Pod is not running"));
            }

            pod.id.clone()
        };

        // Get the one client for this pod
        let clients = self.one_clients.read().await;
        let one_client = clients
            .get(&pod_id)
            .ok_or_else(|| Status::unavailable("No connection to pod"))?;

        let mut one = OneServiceClient::new(one_client.channel());

        // Call GetNetworkInfo on mvirt-one
        let response = one
            .get_network_info(OneEmpty {})
            .await
            .map_err(|e| Status::internal(format!("Failed to get network info: {}", e)))?
            .into_inner();

        // Convert mvirt-one NetworkInfo to mvirt-vmm PodNetworkInfo
        let interfaces = response
            .interfaces
            .into_iter()
            .map(|iface| PodInterfaceInfo {
                name: iface.name,
                mac_address: iface.mac_address,
                ipv4_address: iface.ipv4_address,
                ipv4_netmask: iface.ipv4_netmask,
                ipv4_gateway: iface.ipv4_gateway,
                ipv4_dns: iface.ipv4_dns,
                ipv6_address: iface.ipv6_address,
                ipv6_gateway: iface.ipv6_gateway,
                ipv6_dns: iface.ipv6_dns,
                delegated_prefix: iface.delegated_prefix,
            })
            .collect();

        Ok(Response::new(PodNetworkInfo { interfaces }))
    }
}
