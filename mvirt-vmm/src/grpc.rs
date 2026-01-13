use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info};

use crate::hypervisor::Hypervisor;
use crate::proto::vm_service_server::VmService;
use crate::proto::*;
use crate::store::VmStore;

pub struct VmServiceImpl {
    store: Arc<VmStore>,
    hypervisor: Arc<Hypervisor>,
}

impl VmServiceImpl {
    pub fn new(store: Arc<VmStore>, hypervisor: Arc<Hypervisor>) -> Self {
        Self { store, hypervisor }
    }
}

#[tonic::async_trait]
impl VmService for VmServiceImpl {
    // System

    async fn get_system_info(
        &self,
        _request: Request<GetSystemInfoRequest>,
    ) -> Result<Response<SystemInfo>, Status> {
        use sysinfo::System;

        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();

        let total_cpus = sys.cpus().len() as u32;
        let total_memory_mb = sys.total_memory() / 1024 / 1024;

        // Calculate allocated resources from running VMs
        let entries = self
            .store
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let (allocated_cpus, allocated_memory_mb) = entries
            .iter()
            .filter(|e| e.state == VmState::Running)
            .fold((0u32, 0u64), |(cpus, mem), e| {
                (cpus + e.config.vcpus, mem + e.config.memory_mb)
            });

        let load_avg = System::load_average();

        Ok(Response::new(SystemInfo {
            total_cpus,
            total_memory_mb,
            allocated_cpus,
            allocated_memory_mb,
            load_1: load_avg.one as f32,
            load_5: load_avg.five as f32,
            load_15: load_avg.fifteen as f32,
        }))
    }

    // CRUD

    async fn create_vm(&self, request: Request<CreateVmRequest>) -> Result<Response<Vm>, Status> {
        let req = request.into_inner();
        let config = req
            .config
            .ok_or_else(|| Status::invalid_argument("config is required"))?;

        // Validate boot configuration
        let boot_mode = BootMode::try_from(config.boot_mode).unwrap_or(BootMode::Disk);
        match boot_mode {
            BootMode::Disk | BootMode::Unspecified => {
                if config.disks.is_empty() {
                    return Err(Status::invalid_argument(
                        "Disk boot mode requires at least one disk",
                    ));
                }
            }
            BootMode::Kernel => {
                if config.kernel.is_none() {
                    return Err(Status::invalid_argument(
                        "Kernel boot mode requires kernel path",
                    ));
                }
            }
        }

        info!(name = ?req.name, vcpus = config.vcpus, memory_mb = config.memory_mb, boot_mode = ?boot_mode, "Creating VM");

        let entry = self
            .store
            .create(req.name, config)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(id = %entry.id, "VM created");
        Ok(Response::new(entry.to_proto()))
    }

    async fn get_vm(&self, request: Request<GetVmRequest>) -> Result<Response<Vm>, Status> {
        let req = request.into_inner();
        let entry = self
            .store
            .get(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("VM {} not found", req.id)))?;
        Ok(Response::new(entry.to_proto()))
    }

    async fn list_vms(
        &self,
        _request: Request<ListVmsRequest>,
    ) -> Result<Response<ListVmsResponse>, Status> {
        let entries = self
            .store
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let vms = entries.into_iter().map(|e| e.to_proto()).collect();
        Ok(Response::new(ListVmsResponse { vms }))
    }

    async fn delete_vm(
        &self,
        request: Request<DeleteVmRequest>,
    ) -> Result<Response<DeleteVmResponse>, Status> {
        let req = request.into_inner();
        info!(id = %req.id, "Deleting VM");

        let entry = self
            .store
            .get(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("VM {} not found", req.id)))?;

        if entry.state == VmState::Running {
            return Err(Status::failed_precondition("Cannot delete running VM"));
        }

        self.store
            .delete(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(id = %req.id, "VM deleted");
        Ok(Response::new(DeleteVmResponse {}))
    }

    // Lifecycle

    async fn start_vm(&self, request: Request<StartVmRequest>) -> Result<Response<Vm>, Status> {
        let req = request.into_inner();
        info!(id = %req.id, "Starting VM");

        let entry = self
            .store
            .get(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("VM {} not found", req.id)))?;

        if entry.state == VmState::Running {
            return Err(Status::failed_precondition("VM is already running"));
        }

        // Update state to starting
        self.store
            .update_state(&req.id, VmState::Starting)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Start the VM via hypervisor
        if let Err(e) = self
            .hypervisor
            .start(&req.id, entry.name.as_deref(), &entry.config)
            .await
        {
            // Revert state on failure
            let _ = self.store.update_state(&req.id, VmState::Stopped).await;
            return Err(Status::internal(format!("Failed to start VM: {}", e)));
        }

        // Update state to running
        let entry = self
            .store
            .update_state(&req.id, VmState::Running)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::internal("Failed to update VM state"))?;

        info!(id = %req.id, "VM started");
        Ok(Response::new(entry.to_proto()))
    }

    async fn stop_vm(&self, request: Request<StopVmRequest>) -> Result<Response<Vm>, Status> {
        let req = request.into_inner();
        info!(id = %req.id, timeout = req.timeout_seconds, "Stopping VM");

        let entry = self
            .store
            .get(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("VM {} not found", req.id)))?;

        if entry.state != VmState::Running {
            return Err(Status::failed_precondition("VM is not running"));
        }

        // Update state to stopping
        self.store
            .update_state(&req.id, VmState::Stopping)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Stop the VM via hypervisor
        let timeout = Duration::from_secs(req.timeout_seconds as u64);
        if let Err(e) = self.hypervisor.stop(&req.id, timeout).await {
            return Err(Status::internal(format!("Failed to stop VM: {}", e)));
        }

        // Update state to stopped
        let entry = self
            .store
            .update_state(&req.id, VmState::Stopped)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::internal("Failed to update VM state"))?;

        info!(id = %req.id, "VM stopped");
        Ok(Response::new(entry.to_proto()))
    }

    async fn kill_vm(&self, request: Request<KillVmRequest>) -> Result<Response<Vm>, Status> {
        let req = request.into_inner();
        info!(id = %req.id, "Killing VM");

        let entry = self
            .store
            .get(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("VM {} not found", req.id)))?;

        if entry.state != VmState::Running && entry.state != VmState::Stopping {
            return Err(Status::failed_precondition("VM is not running"));
        }

        // Kill the VM via hypervisor
        if let Err(e) = self.hypervisor.kill(&req.id).await {
            return Err(Status::internal(format!("Failed to kill VM: {}", e)));
        }

        // Update state to stopped
        let entry = self
            .store
            .update_state(&req.id, VmState::Stopped)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::internal("Failed to update VM state"))?;

        info!(id = %req.id, "VM killed");
        Ok(Response::new(entry.to_proto()))
    }

    // Hot-plug (Phase 2 - stubs)

    async fn attach_disk(
        &self,
        _request: Request<AttachDiskRequest>,
    ) -> Result<Response<Vm>, Status> {
        Err(Status::unimplemented("AttachDisk not yet implemented"))
    }

    async fn detach_disk(
        &self,
        _request: Request<DetachDiskRequest>,
    ) -> Result<Response<Vm>, Status> {
        Err(Status::unimplemented("DetachDisk not yet implemented"))
    }

    async fn attach_nic(
        &self,
        _request: Request<AttachNicRequest>,
    ) -> Result<Response<Vm>, Status> {
        Err(Status::unimplemented("AttachNic not yet implemented"))
    }

    async fn detach_nic(
        &self,
        _request: Request<DetachNicRequest>,
    ) -> Result<Response<Vm>, Status> {
        Err(Status::unimplemented("DetachNic not yet implemented"))
    }

    // Console

    type ConsoleStream = ReceiverStream<Result<ConsoleOutput, Status>>;

    async fn console(
        &self,
        request: Request<tonic::Streaming<ConsoleInput>>,
    ) -> Result<Response<Self::ConsoleStream>, Status> {
        let mut input_stream = request.into_inner();

        // Get VM ID from first message
        let first_msg = input_stream
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("No input received"))?
            .map_err(|e| Status::internal(e.to_string()))?;

        let vm_id = first_msg.vm_id;
        if vm_id.is_empty() {
            return Err(Status::invalid_argument(
                "vm_id is required in first message",
            ));
        }

        info!(vm_id = %vm_id, "Console connection requested");

        // Check VM exists and is running
        let entry = self
            .store
            .get(&vm_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("VM {} not found", vm_id)))?;

        if entry.state != VmState::Running {
            return Err(Status::failed_precondition("VM is not running"));
        }

        // Get serial socket path
        let serial_socket = self.hypervisor.serial_socket_path(&vm_id);
        if !serial_socket.exists() {
            return Err(Status::unavailable("Serial socket not available"));
        }

        // Connect to serial socket
        let socket = UnixStream::connect(&serial_socket)
            .await
            .map_err(|e| Status::internal(format!("Failed to connect to serial: {}", e)))?;

        let (mut socket_read, mut socket_write) = socket.into_split();

        // Channel for output to client
        let (tx, rx) = mpsc::channel::<Result<ConsoleOutput, Status>>(32);

        // Send first message data if any
        if !first_msg.data.is_empty() {
            let _ = socket_write.write_all(&first_msg.data).await;
        }

        // Task: Read from socket -> send to client
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                match socket_read.read(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let output = ConsoleOutput {
                            data: buf[..n].to_vec(),
                        };
                        if tx_clone.send(Ok(output)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Error reading from serial socket");
                        break;
                    }
                }
            }
        });

        // Task: Read from client -> write to socket
        tokio::spawn(async move {
            while let Some(result) = input_stream.next().await {
                match result {
                    Ok(input) => {
                        if let Err(e) = socket_write.write_all(&input.data).await {
                            error!(error = %e, "Error writing to serial socket");
                            break;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Error receiving from client");
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    // Events (Phase 2 - stub)

    type WatchVmsStream = ReceiverStream<Result<VmEvent, Status>>;

    async fn watch_vms(
        &self,
        _request: Request<WatchVmsRequest>,
    ) -> Result<Response<Self::WatchVmsStream>, Status> {
        Err(Status::unimplemented("WatchVms not yet implemented"))
    }
}
