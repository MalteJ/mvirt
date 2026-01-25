//! Pod Service - gRPC service for managing container pods in MicroVMs.

use crate::hypervisor::Hypervisor;
use crate::proto::{
    BootMode, Container, ContainerSpec, ContainerState, CreatePodRequest, DeletePodRequest,
    DeletePodResponse, GetPodRequest, ListPodsRequest, ListPodsResponse, LogChunk, Pod,
    PodExecInput, PodExecOutput, PodLogsRequest, PodState, StartPodRequest, StopPodRequest,
    VmConfig, pod_service_server::PodService,
};
use crate::store::VmStore;
use crate::vsock_client::{UosClient, pid_to_cid, wait_for_uos};
use mvirt_log::AuditLogger;
use mvirt_one::proto::{
    ContainerSpec as OneContainerSpec, CreatePodRequest as OneCreatePodRequest,
    StartPodRequest as OneStartPodRequest, StopPodRequest as OneStopPodRequest,
    uos_service_client::UosServiceClient,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Default kernel path for MicroVM boot.
const UOS_KERNEL_PATH: &str = "/var/lib/mvirt/one/vmlinux";
/// Default initramfs path for MicroVM boot.
const UOS_INITRAMFS_PATH: &str = "/var/lib/mvirt/one/initramfs.cpio.gz";
/// Default kernel command line for MicroVM.
const UOS_CMDLINE: &str = "console=ttyS0 quiet";
/// Timeout for waiting for uos to become ready.
const UOS_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
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
    ip_address: String,
    created_at: i64,
    started_at: Option<i64>,
    error_message: Option<String>,
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

        // TODO: Stop the MicroVM if running
        if let Some(vm_id) = &pod.vm_id {
            // Remove uos client
            let mut clients = self.uos_clients.write().await;
            clients.remove(&req.id);

            // Kill the MicroVM
            if let Err(e) = self.hypervisor.kill(vm_id).await {
                warn!(vm_id = %vm_id, error = %e, "Failed to kill VM for pod");
            }
        }

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
        let (pod_id, pod_name, containers) = {
            let mut pods = self.pods.write().await;
            let pod = pods
                .get_mut(&req.id)
                .ok_or_else(|| Status::not_found(format!("Pod {} not found", req.id)))?;

            // Check state
            if pod.state == PodState::Running {
                return Err(Status::failed_precondition("Pod is already running"));
            }

            pod.state = PodState::Starting;
            (pod.id.clone(), pod.name.clone(), pod.containers.clone())
        };

        // Create VM ID based on pod ID
        let vm_id = format!("pod-{}", pod_id);

        // Create VM config for kernel boot with uos
        let vm_config = VmConfig {
            vcpus: POD_DEFAULT_VCPUS,
            memory_mb: POD_DEFAULT_MEMORY_MB,
            boot_mode: BootMode::Kernel.into(),
            kernel: Some(UOS_KERNEL_PATH.to_string()),
            initramfs: Some(UOS_INITRAMFS_PATH.to_string()),
            cmdline: Some(UOS_CMDLINE.to_string()),
            disks: vec![],
            nics: vec![],
            user_data: None,
            nested_virt: false,
        };

        // Start the MicroVM
        info!(vm_id = %vm_id, pod_id = %pod_id, "Starting MicroVM for pod");
        if let Err(e) = self
            .hypervisor
            .start(&vm_id, Some(&pod_name), &vm_config)
            .await
        {
            error!(vm_id = %vm_id, error = %e, "Failed to start MicroVM");
            let mut pods = self.pods.write().await;
            if let Some(pod) = pods.get_mut(&pod_id) {
                pod.state = PodState::Failed;
                pod.error_message = Some(format!("Failed to start MicroVM: {}", e));
            }
            return Err(Status::internal(format!("Failed to start MicroVM: {}", e)));
        }

        // Get PID and convert to CID
        let runtime = self.store.get_runtime(&vm_id).await.map_err(|e| {
            error!(vm_id = %vm_id, error = %e, "Failed to get VM runtime");
            Status::internal("Failed to get VM runtime info")
        })?;

        let runtime = runtime.ok_or_else(|| {
            error!(vm_id = %vm_id, "No runtime info for VM");
            Status::internal("No runtime info for VM")
        })?;

        let cid = pid_to_cid(runtime.pid);
        debug!(vm_id = %vm_id, pid = runtime.pid, cid = cid, "VM started, waiting for uos");

        // Wait for uos to become ready via vsock
        let uos_client = match wait_for_uos(cid, UOS_CONNECT_TIMEOUT).await {
            Ok(client) => client,
            Err(e) => {
                error!(cid = cid, error = %e, "Failed to connect to uos");
                // Kill the VM since we can't communicate with it
                let _ = self.hypervisor.kill(&vm_id).await;
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("Failed to connect to uos: {}", e));
                }
                return Err(Status::internal(format!("Failed to connect to uos: {}", e)));
            }
        };

        // Create gRPC client to uos
        let mut uos = UosServiceClient::new(uos_client.channel());

        // Convert container specs to uos format
        let one_containers: Vec<OneContainerSpec> = containers
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

        // Create pod in uos
        info!(pod_id = %pod_id, "Creating pod in uos");
        let create_req = OneCreatePodRequest {
            id: pod_id.clone(),
            name: pod_name.clone(),
            containers: one_containers,
        };

        if let Err(e) = uos.create_pod(create_req).await {
            error!(pod_id = %pod_id, error = %e, "Failed to create pod in uos");
            let _ = self.hypervisor.kill(&vm_id).await;
            let mut pods = self.pods.write().await;
            if let Some(pod) = pods.get_mut(&pod_id) {
                pod.state = PodState::Failed;
                pod.error_message = Some(format!("Failed to create pod in uos: {}", e));
            }
            return Err(Status::internal(format!(
                "Failed to create pod in uos: {}",
                e
            )));
        }

        // Start pod in uos
        info!(pod_id = %pod_id, "Starting pod in uos");
        let start_req = OneStartPodRequest { id: pod_id.clone() };

        match uos.start_pod(start_req).await {
            Ok(response) => {
                let one_pod = response.into_inner();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                // Update pod state
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Running;
                    pod.vm_id = Some(vm_id.clone());
                    pod.started_at = Some(now);
                    pod.ip_address = one_pod.ip_address;
                    pod.error_message = None;
                }

                // Store uos client for future communication
                let mut clients = self.uos_clients.write().await;
                clients.insert(pod_id.clone(), uos_client);

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
            Err(e) => {
                error!(pod_id = %pod_id, error = %e, "Failed to start pod in uos");
                let _ = self.hypervisor.kill(&vm_id).await;
                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(&pod_id) {
                    pod.state = PodState::Failed;
                    pod.error_message = Some(format!("Failed to start pod in uos: {}", e));
                }
                Err(Status::internal(format!(
                    "Failed to start pod in uos: {}",
                    e
                )))
            }
        }
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
