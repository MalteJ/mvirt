use crate::hugepage::HugePagePool;
use crate::inter_reactor::{CompletionNotify, PacketRef};
use crate::reactor::{
    InterfaceType, NicConfig, Reactor, ReactorHandle, ReactorId, ReactorInfo, ReactorRegistry,
};
use crate::tun::TunDevice;
use crate::vhost_user::{VhostHandshake, VhostUserNetDevice};
use crate::virtqueue::SimpleRxTxQueues;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::io::IntoRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use tracing::info;

/// Buffer size for TUN device reads/writes.
/// This size is required to hold full 64 KiB GSO packets plus virtio/ethernet headers
/// without truncation. Page-aligned for efficient DMA.
pub const TUN_BUFFER_SIZE: usize = 68 * 1024; // 69632 bytes (68 KiB)

/// Number of RX/TX buffers per queue.
pub const TUN_BUFFER_COUNT: usize = 256;

pub struct Router {
    reactor_handle: ReactorHandle,
    reactor_thread: JoinHandle<()>,
    vhost_thread: Option<JoinHandle<io::Result<()>>>,
    tun_name: String,
    /// TUN interface index for kernel route management
    tun_if_index: u32,
    vhost_socket: Option<String>,
    /// Shared reactor registry for inter-reactor communication
    registry: Arc<ReactorRegistry>,
    /// Unique ID of this router's reactor
    reactor_id: ReactorId,
    /// Shutdown flag shared with vhost thread
    shutdown_flag: Arc<AtomicBool>,
}

/// Configuration for a vhost-user device
pub struct VhostConfig {
    pub socket_path: String,
    pub mac: [u8; 6],

    // IP configuration for DHCP
    /// IPv4 address the VM will receive via DHCP
    pub ipv4_address: Option<Ipv4Addr>,
    /// IPv4 gateway from the VM's perspective
    pub ipv4_gateway: Option<Ipv4Addr>,
    /// IPv4 subnet prefix length
    pub ipv4_prefix_len: u8,

    /// IPv6 address the VM will receive via DHCPv6
    pub ipv6_address: Option<Ipv6Addr>,
    /// IPv6 gateway from the VM's perspective
    pub ipv6_gateway: Option<Ipv6Addr>,
    /// IPv6 subnet prefix length
    pub ipv6_prefix_len: u8,

    /// DNS servers for DHCP option 6 / DHCPv6 option 23
    pub dns_servers: Vec<IpAddr>,
}

impl VhostConfig {
    /// Create a new VhostConfig with minimal required fields.
    /// IP configuration can be added via builder-style methods.
    pub fn new(socket_path: impl Into<String>, mac: [u8; 6]) -> Self {
        VhostConfig {
            socket_path: socket_path.into(),
            mac,
            ipv4_address: None,
            ipv4_gateway: None,
            ipv4_prefix_len: 24,
            ipv6_address: None,
            ipv6_gateway: None,
            ipv6_prefix_len: 64,
            dns_servers: Vec::new(),
        }
    }

    /// Set IPv4 configuration for DHCP.
    pub fn with_ipv4(mut self, address: Ipv4Addr, gateway: Ipv4Addr, prefix_len: u8) -> Self {
        self.ipv4_address = Some(address);
        self.ipv4_gateway = Some(gateway);
        self.ipv4_prefix_len = prefix_len;
        self
    }

    /// Set IPv6 configuration for DHCPv6.
    pub fn with_ipv6(mut self, address: Ipv6Addr, gateway: Ipv6Addr, prefix_len: u8) -> Self {
        self.ipv6_address = Some(address);
        self.ipv6_gateway = Some(gateway);
        self.ipv6_prefix_len = prefix_len;
        self
    }

    /// Add DNS servers for DHCP.
    pub fn with_dns(mut self, servers: Vec<IpAddr>) -> Self {
        self.dns_servers = servers;
        self
    }

    /// Convert to NicConfig for the reactor.
    pub fn to_nic_config(&self) -> NicConfig {
        NicConfig {
            mac: self.mac,
            ipv4_address: self.ipv4_address,
            ipv4_gateway: self.ipv4_gateway,
            ipv4_prefix_len: self.ipv4_prefix_len,
            ipv6_address: self.ipv6_address,
            ipv6_gateway: self.ipv6_gateway,
            ipv6_prefix_len: self.ipv6_prefix_len,
            dns_servers: self.dns_servers.clone(),
        }
    }
}

impl Router {
    pub async fn with_config(
        name: &str,
        ip: Ipv4Addr,
        prefix_len: u8,
        buf_size: usize,
        rx_count: usize,
        tx_count: usize,
    ) -> io::Result<Self> {
        Self::with_config_and_vhost(name, ip, prefix_len, buf_size, rx_count, tx_count, None).await
    }

    pub async fn with_config_and_vhost(
        name: &str,
        ip: Ipv4Addr,
        prefix_len: u8,
        buf_size: usize,
        rx_count: usize,
        tx_count: usize,
        vhost_config: Option<VhostConfig>,
    ) -> io::Result<Self> {
        let registry = Arc::new(ReactorRegistry::new());
        Self::with_shared_registry(
            name,
            Some((ip, prefix_len)),
            buf_size,
            rx_count,
            tx_count,
            vhost_config,
            registry,
        )
        .await
    }

    /// Create a router with a shared registry for VM-to-VM communication.
    ///
    /// Multiple routers sharing the same registry can route packets between
    /// their VMs. Use this to create multi-VM setups where packets can flow
    /// between different vhost interfaces.
    pub async fn with_shared_registry(
        name: &str,
        ip: Option<(Ipv4Addr, u8)>,
        buf_size: usize,
        rx_count: usize,
        tx_count: usize,
        vhost_config: Option<VhostConfig>,
        registry: Arc<ReactorRegistry>,
    ) -> io::Result<Self> {
        // Create TUN device (L3 mode - no IP address, only routes)
        let tun = TunDevice::create(name).await?;
        // Note: No IP address assigned to TUN - use kernel routes instead
        // The ip parameter is kept for compatibility but will be removed
        let _ = ip; // Suppress unused warning
        tun.set_up().await?;

        // Store if_index before consuming TUN device
        let tun_if_index = tun.if_index;
        let tun_file = tun.into_file();

        // Allocate buffers
        let buffers = HugePagePool::new((rx_count + tx_count) * buf_size).ok_or_else(|| {
            io::Error::other(
                "Failed to allocate huge pages. Run: echo 64 | sudo tee /proc/sys/vm/nr_hugepages",
            )
        })?;

        // Create queues
        let queues = SimpleRxTxQueues::new(tun_file, buffers, buf_size, rx_count, tx_count);
        let (rx_queue, tx_queue) = queues.split();

        // Create inter-reactor communication channels
        let (packet_tx, packet_rx): (Sender<PacketRef>, Receiver<PacketRef>) = mpsc::channel();
        let (completion_tx, completion_rx): (Sender<CompletionNotify>, Receiver<CompletionNotify>) =
            mpsc::channel();

        // Create reactor with optional vhost handshake channel and registry
        let (reactor, reactor_handle, handshake_tx, reactor_id) =
            if let Some(ref config) = vhost_config {
                let (tx, rx) = mpsc::sync_channel::<VhostHandshake>(1);
                let nic_config = config.to_nic_config();
                let (reactor, handle) = Reactor::with_registry(
                    rx_queue,
                    tx_queue,
                    Some(rx),
                    Some(Arc::clone(&registry)),
                    Some(packet_rx),
                    Some(completion_rx),
                    Some(nic_config),
                );
                let id = reactor.id();
                (reactor, handle, Some(tx), id)
            } else {
                let (reactor, handle) = Reactor::with_registry(
                    rx_queue,
                    tx_queue,
                    None,
                    Some(Arc::clone(&registry)),
                    Some(packet_rx),
                    Some(completion_rx),
                    None, // No NIC config for TUN-only reactors
                );
                let id = reactor.id();
                (reactor, handle, None, id)
            };

        // Get reactor notify fd for vhost daemon (before spawning reactor thread)
        let reactor_notify = if vhost_config.is_some() {
            Some(reactor_handle.get_notify_fd())
        } else {
            None
        };

        // Register the reactor in the registry
        // Use into_raw_fd() to transfer ownership - otherwise the OwnedFd would close
        // the fd when dropped, leaving ReactorInfo with an invalid fd
        let notify_raw_fd = reactor_handle.get_notify_fd().into_raw_fd();
        let reactor_info = if let Some(ref config) = vhost_config {
            // Vhost interface - register with MAC address for Ethernet header construction
            let interface_type = InterfaceType::Vhost {
                device_id: uuid::Uuid::new_v4(), // TODO: Use actual device UUID
            };
            ReactorInfo::with_mac(
                reactor_id,
                notify_raw_fd,
                packet_tx,
                completion_tx,
                interface_type,
                config.mac,
            )
        } else {
            // TUN interface - no MAC address needed
            let interface_type = InterfaceType::Tun {
                if_index: tun_if_index,
            };
            ReactorInfo::new(
                reactor_id,
                notify_raw_fd,
                packet_tx,
                completion_tx,
                interface_type,
            )
        };
        registry.register(reactor_info);
        info!(id = %reactor_id, "Registered reactor in registry");

        // Spawn reactor thread
        let reactor_thread = thread::spawn(move || {
            reactor.run();
        });

        // Create shutdown flag for clean shutdown signaling
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        // Optionally spawn vhost-user device
        let (vhost_thread, vhost_socket) = if let Some(config) = vhost_config {
            let socket_path = config.socket_path.clone();
            let device = VhostUserNetDevice::with_reactor(
                &config.socket_path,
                config.mac,
                handshake_tx.expect("handshake_tx should be set"),
                reactor_notify.expect("reactor_notify should be set"),
            );
            let shutdown_flag_clone = Arc::clone(&shutdown_flag);
            let handle = thread::spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.run()));
                match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => {
                        // Only log as error if not a clean shutdown
                        if !shutdown_flag_clone.load(Ordering::Relaxed) {
                            tracing::error!(error = %e, "vhost-user device run failed");
                        }
                        Err(e)
                    }
                    Err(panic) => {
                        let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = panic.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        tracing::error!(panic = %msg, "vhost-user device panicked!");
                        Err(io::Error::other(msg))
                    }
                }
            });
            info!(socket = %socket_path, mac = ?config.mac, "vhost-user device started");
            (Some(handle), Some(socket_path))
        } else {
            (None, None)
        };

        info!(name, tun_if_index, "Router started (L3 mode)");

        Ok(Router {
            reactor_handle,
            reactor_thread,
            vhost_thread,
            tun_name: name.to_string(),
            tun_if_index,
            vhost_socket,
            registry,
            reactor_id,
            shutdown_flag,
        })
    }

    /// Get the TUN interface index for kernel route management.
    pub fn tun_if_index(&self) -> u32 {
        self.tun_if_index
    }

    /// Get the unique ID of this router's reactor.
    ///
    /// Use this ID when configuring routes to point to this router.
    pub fn reactor_id(&self) -> ReactorId {
        self.reactor_id
    }

    /// Get a reference to the shared reactor registry.
    ///
    /// This can be used to register additional reactors or configure routes
    /// that point to specific reactor IDs.
    pub fn registry(&self) -> &Arc<ReactorRegistry> {
        &self.registry
    }

    /// Get the reactor handle for controlling the reactor.
    pub fn reactor_handle(&self) -> &ReactorHandle {
        &self.reactor_handle
    }

    /// Signal that shutdown is imminent.
    ///
    /// Call this before disconnecting vhost-user frontends to suppress
    /// expected "Disconnected" error messages. The actual cleanup happens
    /// in `shutdown()`.
    pub fn prepare_shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
    }

    pub async fn shutdown(self) -> io::Result<()> {
        // Signal reactor to shutdown
        self.reactor_handle.shutdown();

        // Wait for reactor thread
        self.reactor_thread
            .join()
            .map_err(|_| io::Error::other("Reactor thread panicked"))?;

        // Signal clean shutdown to vhost thread before disconnecting
        self.shutdown_flag.store(true, Ordering::Relaxed);

        // Clean up vhost-user socket if present
        if let Some(socket_path) = &self.vhost_socket {
            // Remove the socket file to signal daemon to stop
            let _ = std::fs::remove_file(socket_path);
        }

        // Don't wait for vhost thread - it may be blocked on accept().
        // The thread will exit when we remove the socket file and the process exits.
        // This is safe because:
        // 1. The reactor thread has already exited
        // 2. The socket file has been removed
        // 3. The vhost thread is not holding any resources we need
        if self.vhost_thread.is_some() {
            info!("vhost-user thread will exit asynchronously");
        }

        // Delete TUN device
        TunDevice::delete(&self.tun_name).await?;

        info!(name = %self.tun_name, "Router stopped");

        Ok(())
    }
}
