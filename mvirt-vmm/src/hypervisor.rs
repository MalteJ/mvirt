use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

use crate::proto::{BootMode, VmConfig};
use crate::store::VmStore;

const CLOUD_HYPERVISOR_BIN: &str = "/usr/bin/cloud-hypervisor";
const FIRMWARE_PATH: &str = "/usr/share/mvirt/CLOUDHV.fd";
const HUGEPAGES_FREE_PATH: &str = "/sys/kernel/mm/hugepages/hugepages-2048kB/free_hugepages";
const HUGEPAGE_SIZE_KB: u64 = 2048;

pub struct Hypervisor {
    data_dir: PathBuf,
    processes: Arc<RwLock<HashMap<String, Child>>>,
    store: Arc<VmStore>,
}

impl Hypervisor {
    pub async fn new(data_dir: PathBuf, store: Arc<VmStore>) -> Result<Self> {
        Ok(Self {
            data_dir,
            processes: Arc::new(RwLock::new(HashMap::new())),
            store,
        })
    }

    /// Check if enough 2MiB hugepages are available for the given memory size
    fn hugepages_available(memory_mb: u64) -> bool {
        let required_pages = (memory_mb * 1024) / HUGEPAGE_SIZE_KB;
        match fs::read_to_string(HUGEPAGES_FREE_PATH) {
            Ok(content) => {
                if let Ok(free_pages) = content.trim().parse::<u64>() {
                    free_pages >= required_pages
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    fn vm_dir(&self, vm_id: &str) -> PathBuf {
        self.data_dir.join("vm").join(vm_id)
    }

    /// Get the data directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn api_socket(&self, vm_id: &str) -> PathBuf {
        self.vm_dir(vm_id).join("api.sock")
    }

    fn cloudinit_iso(&self, vm_id: &str) -> PathBuf {
        self.vm_dir(vm_id).join("cloudinit.iso")
    }

    fn firmware_path(&self) -> PathBuf {
        PathBuf::from(FIRMWARE_PATH)
    }

    async fn create_cloudinit_iso(
        &self,
        vm_id: &str,
        vm_name: Option<&str>,
        user_data: &str,
    ) -> Result<PathBuf> {
        let vm_dir = self.vm_dir(vm_id);
        let iso_path = self.cloudinit_iso(vm_id);

        // Write user-data file
        let user_data_path = vm_dir.join("user-data");
        tokio::fs::write(&user_data_path, user_data).await?;

        // Write meta-data file
        let meta_data_path = vm_dir.join("meta-data");
        let hostname = vm_name.unwrap_or(&vm_id[..8]);
        let meta_data = format!("instance-id: {}\nlocal-hostname: {}\n", vm_id, hostname);
        tokio::fs::write(&meta_data_path, meta_data).await?;

        // Write network-config file (DHCP on all ethernet interfaces)
        let network_config_path = vm_dir.join("network-config");
        let network_config = r#"version: 2
ethernets:
  all:
    match:
      name: "*"
    dhcp4: true
    dhcp6: true
"#;
        tokio::fs::write(&network_config_path, network_config).await?;

        // Generate ISO using genisoimage
        let output = Command::new("genisoimage")
            .args([
                "-output",
                iso_path.to_str().unwrap(),
                "-volid",
                "cidata",
                "-joliet",
                "-rock",
                user_data_path.to_str().unwrap(),
                meta_data_path.to_str().unwrap(),
                network_config_path.to_str().unwrap(),
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("genisoimage failed: {}", stderr));
        }

        info!(vm_id = %vm_id, iso = %iso_path.display(), "Created cloud-init ISO");
        Ok(iso_path)
    }

    /// Prepare the VM directory (create if needed).
    ///
    /// Call this before creating the ready signal listener so the socket
    /// path exists before the VM starts.
    pub async fn prepare_vm_dir(&self, vm_id: &str) -> Result<()> {
        let vm_dir = self.vm_dir(vm_id);
        debug!(vm_dir = %vm_dir.display(), "Creating VM directory");
        tokio::fs::create_dir_all(&vm_dir).await?;
        Ok(())
    }

    pub async fn start(
        &self,
        vm_id: &str,
        vm_name: Option<&str>,
        config: &VmConfig,
        vsock_cid: Option<u32>,
    ) -> Result<()> {
        let vm_dir = self.vm_dir(vm_id);
        let api_socket = self.api_socket(vm_id);

        // Ensure VM directory exists (may have been created by prepare_vm_dir)
        tokio::fs::create_dir_all(&vm_dir).await?;

        // Clean up any stale socket files from previous runs
        if api_socket.exists() {
            debug!(path = %api_socket.display(), "Removing stale API socket");
            let _ = tokio::fs::remove_file(&api_socket).await;
        }

        debug!(vm_dir = %vm_dir.display(), "VM directory created");

        // Build cloud-hypervisor command
        let mut cmd = Command::new(CLOUD_HYPERVISOR_BIN);

        cmd.arg("--api-socket")
            .arg(format!("path={}", api_socket.display()));

        // Boot configuration and serial output based on boot_mode
        let serial_path = match BootMode::try_from(config.boot_mode).unwrap_or(BootMode::Disk) {
            BootMode::Disk | BootMode::Unspecified => {
                // Disk boot: use rust-hypervisor-firmware + serial socket (bidirectional)
                cmd.arg("--kernel").arg(self.firmware_path());
                let serial_socket = vm_dir.join("serial.sock");
                cmd.arg("--serial")
                    .arg(format!("socket={}", serial_socket.display()));
                serial_socket
            }
            BootMode::Kernel => {
                // Kernel boot: use provided kernel + serial file (read-only log)
                if let Some(kernel) = &config.kernel {
                    cmd.arg("--kernel").arg(kernel);
                } else {
                    return Err(anyhow!("Kernel boot mode requires kernel path"));
                }
                let console_log = vm_dir.join("console.log");
                cmd.arg("--serial")
                    .arg(format!("file={}", console_log.display()));
                console_log
            }
        };
        cmd.arg("--console").arg("off");

        let cpus_arg = if config.nested_virt {
            format!("boot={},nested=on", config.vcpus)
        } else {
            format!("boot={}", config.vcpus)
        };
        cmd.arg("--cpus").arg(cpus_arg);

        // Check if any NIC uses vhost-user (requires shared memory)
        let uses_vhost_user = config.nics.iter().any(|nic| nic.vhost_socket.is_some());

        let memory_arg = if Self::hugepages_available(config.memory_mb) {
            info!("Using hugepages for VM memory ({}MB)", config.memory_mb);
            if uses_vhost_user {
                format!("size={}M,hugepages=on,shared=on", config.memory_mb)
            } else {
                format!("size={}M,hugepages=on", config.memory_mb)
            }
        } else if uses_vhost_user {
            format!("size={}M,shared=on", config.memory_mb)
        } else {
            format!("size={}M", config.memory_mb)
        };
        cmd.arg("--memory").arg(memory_arg);

        // Collect all disks
        let mut disk_args: Vec<String> = Vec::new();

        for disk in &config.disks {
            let mut disk_arg = format!("path={}", disk.path);
            if disk.readonly {
                disk_arg.push_str(",readonly=on");
            }
            disk_args.push(disk_arg);
        }

        // Generate and attach cloud-init ISO if user_data is provided
        if let Some(user_data) = &config.user_data {
            let iso_path = self.create_cloudinit_iso(vm_id, vm_name, user_data).await?;
            disk_args.push(format!("path={},readonly=on", iso_path.display()));
        }

        // Add all disks in one --disk argument
        if !disk_args.is_empty() {
            cmd.arg("--disk");
            for arg in disk_args {
                cmd.arg(arg);
            }
        }

        // Add kernel cmdline and initramfs (only for kernel boot)
        if matches!(BootMode::try_from(config.boot_mode), Ok(BootMode::Kernel)) {
            if let Some(cmdline) = &config.cmdline {
                cmd.arg("--cmdline").arg(cmdline);
            }
            if let Some(initramfs) = &config.initramfs {
                cmd.arg("--initramfs").arg(initramfs);
            }
        }

        // Add network interfaces (vhost-user via mvirt-net)
        for nic in &config.nics {
            if let Some(ref socket) = nic.vhost_socket {
                let mut net_arg = format!("vhost_user=true,socket={},num_queues=2", socket);
                if let Some(ref mac) = nic.mac {
                    net_arg.push_str(&format!(",mac={}", mac));
                }
                cmd.arg("--net").arg(net_arg);
                info!(vm_id = %vm_id, socket = %socket, "Using vhost-user network");
            }
        }

        // Add vsock device for MicroVMs (host<->guest communication)
        if let Some(cid) = vsock_cid {
            let vsock_socket = vm_dir.join("vsock.sock");
            cmd.arg("--vsock")
                .arg(format!("cid={},socket={}", cid, vsock_socket.display()));
            info!(vm_id = %vm_id, cid = cid, "Enabling vsock");
        }

        info!(vm_id = %vm_id, cmd = ?cmd.as_std(), "Spawning cloud-hypervisor");

        // Log stdout/stderr to files in VM directory
        let stdout_path = vm_dir.join("cloud-hypervisor.stdout");
        let stderr_path = vm_dir.join("cloud-hypervisor.stderr");
        let stdout_file = std::fs::File::create(&stdout_path)?;
        let stderr_file = std::fs::File::create(&stderr_path)?;

        cmd.stdout(stdout_file);
        cmd.stderr(stderr_file);

        let mut child = cmd.spawn()?;
        let pid = child.id().ok_or_else(|| anyhow!("Failed to get PID"))?;

        info!(vm_id = %vm_id, pid = pid, "cloud-hypervisor started");

        // Wait briefly to check for immediate failure
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(Some(status)) = child.try_wait() {
            let stderr_output = tokio::fs::read_to_string(&stderr_path)
                .await
                .unwrap_or_default();
            error!(vm_id = %vm_id, status = ?status, stderr = %stderr_output, "cloud-hypervisor exited immediately");
            return Err(anyhow!(
                "cloud-hypervisor failed to start: {}",
                stderr_output
            ));
        }

        // Store runtime info with serial path (socket or log file depending on boot mode)
        self.store
            .set_runtime(
                vm_id,
                pid,
                api_socket.to_str().unwrap(),
                serial_path.to_str().unwrap(),
            )
            .await?;

        // Track the process
        self.processes
            .write()
            .await
            .insert(vm_id.to_string(), child);

        Ok(())
    }

    pub async fn stop(&self, vm_id: &str, timeout: Duration) -> Result<()> {
        let api_socket = self.api_socket(vm_id);

        // Try graceful shutdown via API
        if api_socket.exists() {
            info!(vm_id = %vm_id, "Sending shutdown request");
            if let Err(e) = self.send_shutdown(&api_socket).await {
                warn!(vm_id = %vm_id, error = %e, "Shutdown request failed");
            }
        }

        // Wait for process to exit
        if timeout.as_secs() > 0 {
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                if !self.is_running(vm_id).await {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    warn!(vm_id = %vm_id, "Timeout waiting for graceful shutdown, killing");
                    self.kill(vm_id).await?;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        self.cleanup(vm_id).await?;
        Ok(())
    }

    pub async fn kill(&self, vm_id: &str) -> Result<()> {
        let mut processes = self.processes.write().await;

        if let Some(mut child) = processes.remove(vm_id) {
            info!(vm_id = %vm_id, "Killing cloud-hypervisor process");
            child.kill().await?;
            // Wait for the process to actually exit
            let _ = child.wait().await;
            info!(vm_id = %vm_id, "cloud-hypervisor process terminated");
        } else {
            // Try to kill by PID from runtime
            if let Some(runtime) = self.store.get_runtime(vm_id).await? {
                info!(vm_id = %vm_id, pid = runtime.pid, "Killing process by PID");
                let kill_result = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(runtime.pid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
                match kill_result {
                    Ok(_) => {
                        // Wait a bit for process to exit
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(nix::errno::Errno::ESRCH) => {
                        // Process already gone, that's fine - just clean up
                        info!(vm_id = %vm_id, "Process already terminated");
                    }
                    Err(e) => {
                        warn!(vm_id = %vm_id, pid = runtime.pid, error = %e, "Failed to kill process");
                    }
                }
            } else {
                info!(vm_id = %vm_id, "No runtime info, assuming process already terminated");
            }
        }

        // Always update state and clean up
        drop(processes); // Release lock before async calls
        let _ = self
            .store
            .update_state(vm_id, crate::proto::VmState::Stopped)
            .await;
        self.cleanup(vm_id).await?;
        Ok(())
    }

    async fn cleanup(&self, vm_id: &str) -> Result<()> {
        // Remove from tracked processes
        self.processes.write().await.remove(vm_id);

        // Clear runtime from DB
        self.store.clear_runtime(vm_id).await?;

        // Remove socket files
        let vm_dir = self.vm_dir(vm_id);
        if vm_dir.exists() {
            let _ = tokio::fs::remove_dir_all(&vm_dir).await;
        }

        Ok(())
    }

    async fn is_running(&self, vm_id: &str) -> bool {
        let processes = self.processes.read().await;
        if let Some(child) = processes.get(vm_id) {
            // Check if process is still running
            if let Some(pid) = child.id() {
                return is_process_alive(pid);
            }
        }

        // Fallback: check by PID from runtime
        if let Ok(Some(runtime)) = self.store.get_runtime(vm_id).await {
            return is_process_alive(runtime.pid);
        }

        false
    }

    async fn send_shutdown(&self, api_socket: &Path) -> Result<()> {
        use hyper::body::Bytes;
        use hyper::{Method, Request};
        use hyper_util::client::legacy::Client;
        use hyper_util::rt::TokioExecutor;

        let connector = hyperlocal::UnixConnector;
        let client: Client<_, http_body_util::Empty<Bytes>> =
            Client::builder(TokioExecutor::new()).build(connector);

        let uri = hyperlocal::Uri::new(api_socket, "/api/v1/vm.shutdown");

        let req = Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .body(http_body_util::Empty::new())?;

        let _resp = client.request(req).await?;
        Ok(())
    }

    /// Spawn a background task that watches for process exits
    pub fn spawn_watcher(self: Arc<Self>) -> mpsc::Sender<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Watcher shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        self.check_processes().await;
                    }
                }
            }
        });

        shutdown_tx
    }

    async fn check_processes(&self) {
        let mut processes = self.processes.write().await;
        let mut exited = Vec::new();

        // Check VMs with Child handles
        for (vm_id, child) in processes.iter_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    info!(vm_id = %vm_id, status = ?status, "VM process exited");
                    exited.push(vm_id.clone());
                }
                Ok(None) => {} // Still running
                Err(e) => {
                    error!(vm_id = %vm_id, error = %e, "Failed to check process status");
                }
            }
        }

        // Get list of tracked VM IDs to exclude from orphan check
        let tracked_ids: std::collections::HashSet<_> = processes.keys().cloned().collect();

        drop(processes);

        // Check adopted VMs (running but no Child handle) by PID
        if let Ok(vms) = self.store.list_all().await {
            for vm in vms {
                if vm.state != crate::proto::VmState::Running {
                    continue;
                }
                if tracked_ids.contains(&vm.id) {
                    continue;
                }

                // This VM is marked as running but we don't have a Child handle
                // Check if the process is still alive by PID
                if let Ok(Some(runtime)) = self.store.get_runtime(&vm.id).await {
                    if !is_process_alive(runtime.pid) {
                        info!(vm_id = %vm.id, pid = runtime.pid, "Adopted VM process exited");
                        exited.push(vm.id.clone());
                    }
                } else {
                    // No runtime info but marked as running - inconsistent state
                    warn!(vm_id = %vm.id, "VM marked as running but no runtime info");
                    exited.push(vm.id.clone());
                }
            }
        }

        // Update state for exited VMs
        for vm_id in exited {
            if let Err(e) = self.handle_vm_exit(&vm_id).await {
                error!(vm_id = %vm_id, error = %e, "Failed to handle VM exit");
            }
        }
    }

    async fn handle_vm_exit(&self, vm_id: &str) -> Result<()> {
        use crate::proto::VmState;

        self.processes.write().await.remove(vm_id);
        self.store.clear_runtime(vm_id).await?;
        self.store.update_state(vm_id, VmState::Stopped).await?;

        // Cleanup socket dir
        let vm_dir = self.vm_dir(vm_id);
        if vm_dir.exists() {
            let _ = tokio::fs::remove_dir_all(&vm_dir).await;
        }

        Ok(())
    }

    /// Get the console path (PTY) for the VM from runtime info
    pub async fn console_path(&self, vm_id: &str) -> Option<PathBuf> {
        if let Ok(Some(runtime)) = self.store.get_runtime(vm_id).await {
            Some(PathBuf::from(runtime.serial_socket))
        } else {
            None
        }
    }

    /// Check all "running" VMs on startup and clean up stale ones
    pub async fn recover_vms(&self) -> Result<()> {
        use crate::proto::VmState;

        info!("Checking running VMs...");

        let vms = self.store.list_all().await?;
        let running_vms: Vec<_> = vms
            .into_iter()
            .filter(|vm| {
                vm.state == VmState::Running
                    || vm.state == VmState::Starting
                    || vm.state == VmState::Stopping
            })
            .collect();

        if running_vms.is_empty() {
            info!("No running VMs to recover");
            return Ok(());
        }

        for vm in running_vms {
            info!(vm_id = %vm.id, state = ?vm.state, "Checking VM");

            let runtime = self.store.get_runtime(&vm.id).await?;

            let process_alive = if let Some(ref rt) = runtime {
                is_process_alive(rt.pid)
            } else {
                false
            };

            let api_exists = self.api_socket(&vm.id).exists();
            // PTY path is stored in runtime, check if it exists and is accessible
            let console_accessible = if let Some(ref rt) = runtime {
                Path::new(&rt.serial_socket).exists()
            } else {
                false
            };

            if process_alive && console_accessible && api_exists {
                info!(vm_id = %vm.id, pid = runtime.as_ref().map(|r| r.pid), "VM still running");
                // Process is alive but we don't have the Child handle
                // The watcher will track it by PID via runtime info
            } else {
                warn!(
                    vm_id = %vm.id,
                    process_alive = process_alive,
                    console_accessible = console_accessible,
                    api_exists = api_exists,
                    "VM in inconsistent state, cleaning up"
                );

                // Kill process if still alive
                if process_alive && let Some(ref rt) = runtime {
                    let _ = nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(rt.pid as i32),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }

                // Clean up
                self.store.update_state(&vm.id, VmState::Stopped).await?;
                self.store.clear_runtime(&vm.id).await?;

                let vm_dir = self.vm_dir(&vm.id);
                if vm_dir.exists() {
                    let _ = tokio::fs::remove_dir_all(&vm_dir).await;
                }

                info!(vm_id = %vm.id, "VM cleaned up, state set to stopped");
            }
        }

        Ok(())
    }
}

fn is_process_alive(pid: u32) -> bool {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        None, // Signal 0 = just check if process exists
    )
    .is_ok()
}
