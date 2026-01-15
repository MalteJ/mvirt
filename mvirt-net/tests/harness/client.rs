//! vhost-user test client
//!
//! Simulates the VM/frontend side of a vhost-user connection for testing.

use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::time::Duration;

use nix::libc;
use vhost::vhost_user::message::{
    VhostUserConfigFlags, VhostUserHeaderFlag, VhostUserProtocolFeatures,
};
use vhost::vhost_user::{Frontend, VhostUserFrontend};
use vhost::{VhostBackend, VhostUserMemoryRegionInfo, VringConfigData};
use vm_memory::{Address, GuestAddress};
use vmm_sys_util::eventfd::EventFd;

use super::memory::{SharedMemory, MEMORY_SIZE};
use super::virtio::{VirtioQueue, QUEUE_SIZE};

/// Virtio feature flags
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;
const VHOST_USER_F_PROTOCOL_FEATURES: u64 = 1 << 30;

/// Queue indices
const RX_QUEUE_IDX: usize = 0;
const TX_QUEUE_IDX: usize = 1;

/// RX buffer size
const RX_BUFFER_SIZE: usize = 2048;

/// vhost-user test client
pub struct VhostTestClient {
    /// vhost-user frontend connection
    frontend: Frontend,
    /// Shared memory region
    memory: SharedMemory,
    /// RX queue (device writes, we read)
    rx_queue: VirtioQueue,
    /// TX queue (we write, device reads)
    tx_queue: VirtioQueue,
    /// RX queue kick eventfd (we kick to signal new buffers)
    rx_kick: EventFd,
    /// TX queue kick eventfd (we kick to signal new buffers)
    tx_kick: EventFd,
    /// RX queue call eventfd (device signals when it used buffers)
    rx_call: EventFd,
    /// TX queue call eventfd (device signals when it used buffers)
    tx_call: EventFd,
    /// Negotiated features
    features: u64,
    /// Next data buffer offset
    next_buffer_offset: u64,
    /// Pre-allocated RX buffer addresses
    rx_buffer_addrs: Vec<GuestAddress>,
}

impl VhostTestClient {
    /// Connect to a vhost-user backend and perform handshake
    pub fn connect<P: AsRef<Path>>(socket_path: P) -> std::io::Result<Self> {
        let path = socket_path.as_ref().to_string_lossy().to_string();

        // Connect to backend (max_queue_num = 2 for RX + TX queues)
        let frontend = Frontend::connect(&path, 2)
            .map_err(|e| std::io::Error::other(format!("connect failed: {e}")))?;

        // Create shared memory
        let memory = SharedMemory::new(MEMORY_SIZE)?;

        // Create eventfds for queue signaling
        let rx_kick = EventFd::new(libc::EFD_NONBLOCK)
            .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;
        let tx_kick = EventFd::new(libc::EFD_NONBLOCK)
            .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;
        let rx_call = EventFd::new(libc::EFD_NONBLOCK)
            .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;
        let tx_call = EventFd::new(libc::EFD_NONBLOCK)
            .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;

        // Create queue structures
        // Memory layout:
        // 0x0000 - 0x0FFF: RX desc table (4KB)
        // 0x1000 - 0x1FFF: RX avail ring (4KB)
        // 0x2000 - 0x2FFF: RX used ring (4KB)
        // 0x3000 - 0x3FFF: TX desc table (4KB)
        // 0x4000 - 0x4FFF: TX avail ring (4KB)
        // 0x5000 - 0x5FFF: TX used ring (4KB)
        // 0x6000+: data buffers
        let rx_queue = VirtioQueue::new(
            RX_QUEUE_IDX as u16,
            GuestAddress(0x0000),
            GuestAddress(0x1000),
            GuestAddress(0x2000),
        );

        let tx_queue = VirtioQueue::new(
            TX_QUEUE_IDX as u16,
            GuestAddress(0x3000),
            GuestAddress(0x4000),
            GuestAddress(0x5000),
        );

        let mut client = Self {
            frontend,
            memory,
            rx_queue,
            tx_queue,
            rx_kick,
            tx_kick,
            rx_call,
            tx_call,
            features: 0,
            next_buffer_offset: 0x6000,
            rx_buffer_addrs: Vec::new(),
        };

        // Initialize queue memory
        let guest_mem = client.memory.to_guest_memory()?;
        client.rx_queue.init(&guest_mem)?;
        client.tx_queue.init(&guest_mem)?;

        // Perform handshake
        client.handshake()?;

        Ok(client)
    }

    /// Perform vhost-user handshake
    fn handshake(&mut self) -> std::io::Result<()> {
        // 1. Set owner
        self.frontend
            .set_owner()
            .map_err(|e| std::io::Error::other(format!("set_owner: {e}")))?;

        // 2. Get and negotiate features
        let backend_features = self
            .frontend
            .get_features()
            .map_err(|e| std::io::Error::other(format!("get_features: {e}")))?;

        eprintln!("[TEST] Backend features: {:#x}", backend_features);

        // Negotiate features
        self.features = backend_features
            & (VIRTIO_F_VERSION_1
                | VIRTIO_NET_F_MAC
                | VIRTIO_NET_F_STATUS
                | VIRTIO_RING_F_EVENT_IDX
                | VHOST_USER_F_PROTOCOL_FEATURES);

        self.frontend
            .set_features(self.features)
            .map_err(|e| std::io::Error::other(format!("set_features: {e}")))?;

        eprintln!("[TEST] Negotiated features: {:#x}", self.features);

        // 3. If protocol features supported, negotiate them
        if self.features & VHOST_USER_F_PROTOCOL_FEATURES != 0 {
            let proto_features = self
                .frontend
                .get_protocol_features()
                .map_err(|e| std::io::Error::other(format!("get_protocol_features: {e}")))?;

            eprintln!("[TEST] Backend protocol features: {:?}", proto_features);

            // Request CONFIG and REPLY_ACK features if available
            let negotiated = proto_features
                & (VhostUserProtocolFeatures::CONFIG | VhostUserProtocolFeatures::REPLY_ACK);
            self.frontend
                .set_protocol_features(negotiated)
                .map_err(|e| std::io::Error::other(format!("set_protocol_features: {e}")))?;

            // Enable NEED_REPLY header flag if REPLY_ACK was negotiated
            if negotiated.contains(VhostUserProtocolFeatures::REPLY_ACK) {
                self.frontend.set_hdr_flags(VhostUserHeaderFlag::NEED_REPLY);
            }
        }

        // 4. Set memory table
        let regions = vec![VhostUserMemoryRegionInfo {
            guest_phys_addr: 0,
            memory_size: self.memory.size,
            userspace_addr: self.memory.ptr as u64,
            mmap_offset: 0,
            mmap_handle: self.memory.fd,
        }];

        self.frontend
            .set_mem_table(&regions)
            .map_err(|e| std::io::Error::other(format!("set_mem_table: {e}")))?;

        // 5. Configure queues
        self.setup_queue(RX_QUEUE_IDX)?;
        self.setup_queue(TX_QUEUE_IDX)?;

        // 6. Pre-allocate RX buffers
        self.fill_rx_queue()?;

        Ok(())
    }

    /// Setup a single virtio queue
    fn setup_queue(&mut self, queue_idx: usize) -> std::io::Result<()> {
        let (queue, kick, call) = if queue_idx == RX_QUEUE_IDX {
            (&self.rx_queue, &self.rx_kick, &self.rx_call)
        } else {
            (&self.tx_queue, &self.tx_kick, &self.tx_call)
        };

        // Set queue size
        self.frontend
            .set_vring_num(queue_idx, QUEUE_SIZE)
            .map_err(|e| std::io::Error::other(format!("set_vring_num: {e}")))?;

        // Set queue addresses using VringConfigData
        // Addresses must be host virtual addresses, not GPAs!
        // The backend converts these to GPAs using the memory mapping.
        let base = self.memory.ptr as u64;
        let config = VringConfigData {
            queue_max_size: QUEUE_SIZE,
            queue_size: QUEUE_SIZE,
            flags: 0,
            desc_table_addr: base + queue.desc_table.raw_value(),
            used_ring_addr: base + queue.used_ring.raw_value(),
            avail_ring_addr: base + queue.avail_ring.raw_value(),
            log_addr: None,
        };

        self.frontend
            .set_vring_addr(queue_idx, &config)
            .map_err(|e| std::io::Error::other(format!("set_vring_addr: {e}")))?;

        // Set base index
        self.frontend
            .set_vring_base(queue_idx, 0)
            .map_err(|e| std::io::Error::other(format!("set_vring_base: {e}")))?;

        // Set kick fd (we write to signal new buffers)
        self.frontend
            .set_vring_kick(queue_idx, kick)
            .map_err(|e| std::io::Error::other(format!("set_vring_kick: {e}")))?;

        // Set call fd (device writes to signal used buffers)
        self.frontend
            .set_vring_call(queue_idx, call)
            .map_err(|e| std::io::Error::other(format!("set_vring_call: {e}")))?;

        // Enable the vring
        self.frontend
            .set_vring_enable(queue_idx, true)
            .map_err(|e| std::io::Error::other(format!("set_vring_enable: {e}")))?;

        Ok(())
    }

    /// Pre-fill the RX queue with buffers
    fn fill_rx_queue(&mut self) -> std::io::Result<()> {
        let guest_mem = self.memory.to_guest_memory()?;

        // Add several RX buffers
        for _ in 0..16 {
            let addr = self.alloc_buffer(RX_BUFFER_SIZE);
            self.rx_buffer_addrs.push(addr);
            self.rx_queue
                .add_rx_buffer(&guest_mem, addr, RX_BUFFER_SIZE)?;
        }

        // Kick RX queue to notify device of new buffers
        self.rx_kick
            .write(1)
            .map_err(|e| std::io::Error::other(format!("rx kick: {e}")))?;

        Ok(())
    }

    /// Allocate a data buffer
    fn alloc_buffer(&mut self, size: usize) -> GuestAddress {
        let addr = GuestAddress(self.next_buffer_offset);
        // Align to 16 bytes
        self.next_buffer_offset += ((size + 15) & !15) as u64;
        addr
    }

    /// Check if a feature is negotiated
    pub fn has_feature(&self, feature: u64) -> bool {
        self.features & feature != 0
    }

    /// Get MAC address from device config
    pub fn get_mac_from_config(&mut self) -> std::io::Result<[u8; 6]> {
        // Use empty flags for reading config (WRITABLE is only for writing)
        let (_, payload) = self
            .frontend
            .get_config(0, 6, VhostUserConfigFlags::empty(), &[0u8; 6])
            .map_err(|e| std::io::Error::other(format!("get_config: {e}")))?;

        let mut mac = [0u8; 6];
        mac.copy_from_slice(&payload.as_slice()[..6]);
        Ok(mac)
    }

    /// Send a packet through the TX queue
    pub fn send_packet(&mut self, frame: &[u8]) -> std::io::Result<()> {
        let guest_mem = self.memory.to_guest_memory()?;

        // Allocate buffer for packet
        let data_addr = self.alloc_buffer(frame.len() + 12); // +12 for virtio header

        // Add to TX queue
        self.tx_queue.add_tx_buffer(&guest_mem, data_addr, frame)?;

        // Kick TX queue
        self.tx_kick
            .write(1)
            .map_err(|e| std::io::Error::other(format!("tx kick: {e}")))?;

        // Wait for TX completion (brief poll)
        std::thread::sleep(Duration::from_millis(10));

        Ok(())
    }

    /// Wait for and receive a packet from the RX queue
    pub fn recv_packet(&mut self, timeout_ms: u64) -> std::io::Result<Vec<u8>> {
        let guest_mem = self.memory.to_guest_memory()?;
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        loop {
            // Check for used RX descriptors
            if let Some((desc_idx, len)) = self.rx_queue.pop_used(&guest_mem)? {
                // Find the buffer address for this descriptor
                let addr = self
                    .rx_buffer_addrs
                    .get(desc_idx as usize)
                    .copied()
                    .ok_or_else(|| std::io::Error::other("invalid desc_idx"))?;

                // Read the data
                let data = self.rx_queue.read_rx_data(&guest_mem, addr, len)?;

                // Replenish RX queue with new buffer
                let new_addr = self.alloc_buffer(RX_BUFFER_SIZE);
                if (desc_idx as usize) < self.rx_buffer_addrs.len() {
                    self.rx_buffer_addrs[desc_idx as usize] = new_addr;
                } else {
                    self.rx_buffer_addrs.push(new_addr);
                }
                self.rx_queue
                    .add_rx_buffer(&guest_mem, new_addr, RX_BUFFER_SIZE)?;
                self.rx_kick.write(1).ok();

                return Ok(data);
            }

            // Check timeout
            if start.elapsed() > timeout {
                return Err(std::io::Error::other("recv timeout"));
            }

            // Poll the call eventfd
            let mut pollfd = libc::pollfd {
                fd: self.rx_call.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };

            let remaining = timeout.saturating_sub(start.elapsed());
            let poll_timeout = remaining.as_millis().min(100) as i32;

            let ret = unsafe { libc::poll(&mut pollfd, 1, poll_timeout) };
            if ret > 0 && pollfd.revents & libc::POLLIN != 0 {
                // Clear the eventfd
                let _ = self.rx_call.read();
            }
        }
    }

    /// Wait for an RX event without consuming it
    pub fn wait_for_rx(&self, timeout_ms: u64) -> std::io::Result<bool> {
        let mut pollfd = libc::pollfd {
            fd: self.rx_call.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        let ret = unsafe { libc::poll(&mut pollfd, 1, timeout_ms as i32) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(ret > 0 && pollfd.revents & libc::POLLIN != 0)
    }
}

impl Drop for VhostTestClient {
    fn drop(&mut self) {
        // Disable vrings before disconnecting
        let _ = self.frontend.set_vring_enable(RX_QUEUE_IDX, false);
        let _ = self.frontend.set_vring_enable(TX_QUEUE_IDX, false);
    }
}
