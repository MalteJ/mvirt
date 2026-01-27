//! Pod API Handler - gRPC/vsock API implementation.

use super::Command;
use crate::error::PodError;
use crate::proto::{
    CreatePodRequest, DeletePodRequest, Empty, ExecInput, ExecOutput, GetPodRequest,
    HealthResponse, InterfaceInfo, ListPodsResponse, LogsRequest, LogsResponse, NetworkInfo, Pod,
    ShutdownRequest, StartPodRequest, StopPodRequest, uos_service_server::UosService,
};
use crate::utils::network;
use log::info;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

/// Pod API Handler for gRPC/vsock requests.
pub struct PodApiHandler {
    command_tx: mpsc::Sender<Command>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl PodApiHandler {
    /// Create a new Pod API handler.
    pub fn new(command_tx: mpsc::Sender<Command>, shutdown_tx: mpsc::Sender<()>) -> Self {
        Self {
            command_tx,
            shutdown_tx: Some(shutdown_tx),
        }
    }
}

fn pod_error_to_status(e: PodError) -> Status {
    match e {
        PodError::NotFound(_) => Status::not_found(e.to_string()),
        PodError::InvalidState { .. } => Status::failed_precondition(e.to_string()),
        PodError::ContainerFailed { .. } => Status::internal(e.to_string()),
    }
}

#[tonic::async_trait]
impl UosService for PodApiHandler {
    async fn create_pod(
        &self,
        request: Request<CreatePodRequest>,
    ) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        info!("API: CreatePod name={}", req.name);

        let (responder, rx) = oneshot::channel();
        let cmd = Command::Create {
            id: req.id,
            name: req.name,
            containers: req.containers,
            responder,
        };

        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| Status::unavailable("Service unavailable"))?;

        let result = rx
            .await
            .map_err(|_| Status::internal("Service error"))?
            .map_err(pod_error_to_status)?;

        Ok(Response::new(result))
    }

    async fn start_pod(&self, request: Request<StartPodRequest>) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        info!("API: StartPod id={}", req.id);

        let (responder, rx) = oneshot::channel();
        let cmd = Command::Start {
            id: req.id,
            responder,
        };

        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| Status::unavailable("Service unavailable"))?;

        let result = rx
            .await
            .map_err(|_| Status::internal("Service error"))?
            .map_err(pod_error_to_status)?;

        Ok(Response::new(result))
    }

    async fn stop_pod(&self, request: Request<StopPodRequest>) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        info!("API: StopPod id={}", req.id);

        let timeout = if req.timeout_seconds == 0 {
            10
        } else {
            req.timeout_seconds
        };

        let (responder, rx) = oneshot::channel();
        let cmd = Command::Stop {
            id: req.id,
            timeout_seconds: timeout,
            responder,
        };

        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| Status::unavailable("Service unavailable"))?;

        let result = rx
            .await
            .map_err(|_| Status::internal("Service error"))?
            .map_err(pod_error_to_status)?;

        Ok(Response::new(result))
    }

    async fn delete_pod(
        &self,
        request: Request<DeletePodRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        info!("API: DeletePod id={}", req.id);

        let (responder, rx) = oneshot::channel();
        let cmd = Command::Delete {
            id: req.id,
            force: req.force,
            responder,
        };

        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| Status::unavailable("Service unavailable"))?;

        rx.await
            .map_err(|_| Status::internal("Service error"))?
            .map_err(pod_error_to_status)?;

        Ok(Response::new(Empty {}))
    }

    async fn get_pod(&self, request: Request<GetPodRequest>) -> Result<Response<Pod>, Status> {
        let req = request.into_inner();
        info!("API: GetPod id={}", req.id);

        let (responder, rx) = oneshot::channel();
        let cmd = Command::Get {
            id: req.id,
            responder,
        };

        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| Status::unavailable("Service unavailable"))?;

        let result = rx
            .await
            .map_err(|_| Status::internal("Service error"))?
            .map_err(pod_error_to_status)?;

        Ok(Response::new(result))
    }

    async fn list_pods(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListPodsResponse>, Status> {
        info!("API: ListPods");

        let (responder, rx) = oneshot::channel();
        let cmd = Command::List { responder };

        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| Status::unavailable("Service unavailable"))?;

        let pods = rx.await.map_err(|_| Status::internal("Service error"))?;

        Ok(Response::new(ListPodsResponse { pods }))
    }

    type LogsStream = ReceiverStream<Result<LogsResponse, Status>>;

    async fn logs(
        &self,
        _request: Request<LogsRequest>,
    ) -> Result<Response<Self::LogsStream>, Status> {
        // TODO: Implement logs streaming
        Err(Status::unimplemented("Logs not yet implemented"))
    }

    type ExecStream = ReceiverStream<Result<ExecOutput, Status>>;

    async fn exec(
        &self,
        _request: Request<Streaming<ExecInput>>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        // TODO: Implement exec streaming
        Err(Status::unimplemented("Exec not yet implemented"))
    }

    async fn shutdown(&self, request: Request<ShutdownRequest>) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        info!("API: Shutdown timeout={}s", req.timeout_seconds);

        // TODO: Gracefully stop all pods first

        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(()).await;
        }

        Ok(Response::new(Empty {}))
    }

    async fn health(&self, _request: Request<Empty>) -> Result<Response<HealthResponse>, Status> {
        // Get pod count
        let (responder, rx) = oneshot::channel();
        let cmd = Command::List { responder };

        let (running_pods, running_containers) = if self.command_tx.send(cmd).await.is_ok() {
            if let Ok(pods) = rx.await {
                let running = pods
                    .iter()
                    .filter(|p| p.state == crate::proto::PodState::Running as i32)
                    .count() as u32;
                let containers: u32 = pods
                    .iter()
                    .map(|p| {
                        p.containers
                            .iter()
                            .filter(|c| c.state == crate::proto::ContainerState::Running as i32)
                            .count() as u32
                    })
                    .sum();
                (running, containers)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        Ok(Response::new(HealthResponse {
            healthy: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            running_pods,
            running_containers,
        }))
    }

    async fn get_network_info(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<NetworkInfo>, Status> {
        info!("API: GetNetworkInfo");

        let state = network::get_network_state().await;

        let interfaces = state
            .interfaces
            .into_iter()
            .map(|iface| InterfaceInfo {
                name: iface.name,
                mac_address: iface.mac_address,
                ipv4_address: iface
                    .ipv4_address
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                ipv4_netmask: iface
                    .ipv4_netmask
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                ipv4_gateway: iface
                    .ipv4_gateway
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                ipv4_dns: iface.ipv4_dns.iter().map(|a| a.to_string()).collect(),
                ipv6_address: iface
                    .ipv6_address
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                ipv6_gateway: iface
                    .ipv6_gateway
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                ipv6_dns: iface.ipv6_dns.iter().map(|a| a.to_string()).collect(),
                delegated_prefix: iface.delegated_prefix.unwrap_or_default(),
            })
            .collect();

        Ok(Response::new(NetworkInfo { interfaces }))
    }
}
