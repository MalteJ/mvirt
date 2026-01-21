//! vhost-user frontend device for testing
//!
//! This simulates the VM side of a virtio-net device, connecting to
//! a vhost-user backend via Unix socket.

use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::Duration;

use nix::libc;
use vhost::vhost_user::message::{VhostUserProtocolFeatures, VhostUserVirtioFeatures};
use vhost::vhost_user::{Frontend, VhostUserFrontend};
use vhost::{VhostBackend, VhostUserMemoryRegionInfo, VringConfigData};
use vm_memory::{
    Bytes, FileOffset, GuestAddress, GuestMemory, GuestMemoryMmap, GuestMemoryRegion,
    GuestRegionMmap, MemoryRegionAddress,
};
use vmm_sys_util::eventfd::EventFd;

use super::virtqueue::{VirtqueueDriver, wait_for_call};

const QUEUE_SIZE: u16 = 256;
const MEM_SIZE: usize = 16 * 1024 * 1024; // 16 MB

// Virtio feature flags
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_F_RING_EVENT_IDX: u64 = 1 << 29;

// Queue indices
const RX_QUEUE: usize = 0;
const TX_QUEUE: usize = 1;

/// Virtio net header (without mergeable buffers)
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

pub const VIRTIO_NET_HDR_SIZE: usize = std::mem::size_of::<VirtioNetHdr>();

/// vhost-user frontend device (VM simulator)
///
/// This provides a high-level interface for testing vhost-user backends
/// by simulating a VM's virtio-net driver.
pub struct VhostUserFrontendDevice {
    frontend: Frontend,
    mem: GuestMemoryMmap,
    rx_queue: VirtqueueDriver,
    tx_queue: VirtqueueDriver,
    buf_region_start: u64,
    /// Track next buffer slot for TX
    next_tx_buf: u16,
    /// Track next buffer slot for RX
    next_rx_buf: u16,
    /// Whether event_idx feature is enabled
    event_idx: bool,
}

impl VhostUserFrontendDevice {
    /// Connect to a vhost-user backend
    ///
    /// # Arguments
    /// * `socket_path` - Path to the vhost-user Unix socket
    pub fn connect(socket_path: &str) -> io::Result<Self> {
        Self::connect_with_event_idx(socket_path, false)
    }

    /// Connect to a vhost-user backend with event_idx support
    ///
    /// # Arguments
    /// * `socket_path` - Path to the vhost-user Unix socket
    /// * `event_idx` - Whether to enable VIRTIO_F_RING_EVENT_IDX
    pub fn connect_with_event_idx(socket_path: &str, event_idx: bool) -> io::Result<Self> {
        // Create file-backed memory using memfd
        let memfd = unsafe {
            let fd = libc::memfd_create(c"vhost-test-mem".as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            File::from_raw_fd(fd)
        };

        memfd.set_len(MEM_SIZE as u64)?;

        // Create guest memory region from file
        let mmap_region = vm_memory::MmapRegion::from_file(FileOffset::new(memfd, 0), MEM_SIZE)
            .map_err(|e| io::Error::other(format!("mmap region: {:?}", e)))?;

        let region = GuestRegionMmap::new(mmap_region, GuestAddress(0))
            .ok_or_else(|| io::Error::other("guest region creation failed"))?;

        let mem = GuestMemoryMmap::from_regions(vec![region])
            .map_err(|e| io::Error::other(format!("mmap failed: {:?}", e)))?;

        // Memory layout:
        // 0x0000_0000 - RX queue structures
        // 0x0001_0000 - TX queue structures
        // 0x0010_0000 - TX buffer region (1MB)
        // 0x0020_0000 - RX buffer region (1MB+)
        let rx_queue_addr = 0x0000_0000u64;
        let tx_queue_addr = 0x0001_0000u64;
        let buf_region_start = 0x0010_0000u64;

        let mut rx_queue = VirtqueueDriver::new(QUEUE_SIZE, rx_queue_addr, event_idx)?;
        let mut tx_queue = VirtqueueDriver::new(QUEUE_SIZE, tx_queue_addr, event_idx)?;

        // Initialize queue structures
        rx_queue.init(&mem)?;
        tx_queue.init(&mem)?;

        // Connect to backend
        let frontend = Frontend::connect(socket_path, 2)
            .map_err(|e| io::Error::other(format!("connect failed: {:?}", e)))?;

        Ok(VhostUserFrontendDevice {
            frontend,
            mem,
            rx_queue,
            tx_queue,
            buf_region_start,
            next_tx_buf: 0,
            next_rx_buf: 0,
            event_idx,
        })
    }

    /// Set up the vhost-user connection
    ///
    /// This negotiates features, sets up memory mapping, and configures
    /// the virtqueues.
    pub fn setup(&mut self) -> io::Result<()> {
        // Get and negotiate features
        let features = self
            .frontend
            .get_features()
            .map_err(|e| io::Error::other(format!("get_features: {:?}", e)))?;

        let mut negotiated =
            features & (VIRTIO_F_VERSION_1 | VhostUserVirtioFeatures::PROTOCOL_FEATURES.bits());

        if self.event_idx && (features & VIRTIO_F_RING_EVENT_IDX) != 0 {
            negotiated |= VIRTIO_F_RING_EVENT_IDX;
        }

        self.frontend
            .set_features(negotiated)
            .map_err(|e| io::Error::other(format!("set_features: {:?}", e)))?;

        // Set owner
        self.frontend
            .set_owner()
            .map_err(|e| io::Error::other(format!("set_owner: {:?}", e)))?;

        // Get protocol features
        let proto_features = self
            .frontend
            .get_protocol_features()
            .map_err(|e| io::Error::other(format!("get_proto_features: {:?}", e)))?;
        self.frontend
            .set_protocol_features(proto_features & VhostUserProtocolFeatures::CONFIG)
            .map_err(|e| io::Error::other(format!("set_proto_features: {:?}", e)))?;

        // Set memory table
        let region = self.mem.iter().next().unwrap();
        let host_addr = region.get_host_address(MemoryRegionAddress(0)).unwrap() as u64;
        let mem_region = VhostUserMemoryRegionInfo {
            guest_phys_addr: region.start_addr().0,
            memory_size: region.len(),
            userspace_addr: host_addr,
            mmap_offset: 0,
            mmap_handle: region.file_offset().unwrap().file().as_raw_fd(),
        };
        self.frontend
            .set_mem_table(&[mem_region])
            .map_err(|e| io::Error::other(format!("set_mem_table: {:?}", e)))?;

        // Wait for backend to process memory table
        std::thread::sleep(Duration::from_millis(50));

        // Setup queues
        self.setup_queue(RX_QUEUE, &self.rx_queue)?;
        self.setup_queue(TX_QUEUE, &self.tx_queue)?;

        // Enable queues
        self.frontend
            .set_vring_enable(RX_QUEUE, true)
            .map_err(|e| io::Error::other(format!("enable rx: {:?}", e)))?;
        self.frontend
            .set_vring_enable(TX_QUEUE, true)
            .map_err(|e| io::Error::other(format!("enable tx: {:?}", e)))?;

        // Small delay to ensure backend has fully initialized
        std::thread::sleep(Duration::from_millis(100));

        Ok(())
    }

    /// Set up a single virtqueue
    fn setup_queue(&self, queue_idx: usize, queue: &VirtqueueDriver) -> io::Result<()> {
        self.frontend
            .set_vring_num(queue_idx, queue.size())
            .map_err(|e| io::Error::other(format!("set_vring_num: {:?}", e)))?;

        // vhost-user protocol expects Host Virtual Addresses (HVA)
        let region = self.mem.iter().next().unwrap();
        let host_base = region.get_host_address(MemoryRegionAddress(0)).unwrap() as u64;
        let guest_base = region.start_addr().0;

        let desc_hva = host_base + (queue.desc_addr() - guest_base);
        let avail_hva = host_base + (queue.avail_addr() - guest_base);
        let used_hva = host_base + (queue.used_addr() - guest_base);

        let config = VringConfigData {
            queue_max_size: queue.size(),
            queue_size: queue.size(),
            flags: 0,
            desc_table_addr: desc_hva,
            used_ring_addr: used_hva,
            avail_ring_addr: avail_hva,
            log_addr: None,
        };
        self.frontend
            .set_vring_addr(queue_idx, &config)
            .map_err(|e| io::Error::other(format!("set_vring_addr: {:?}", e)))?;

        self.frontend
            .set_vring_base(queue_idx, 0)
            .map_err(|e| io::Error::other(format!("set_vring_base: {:?}", e)))?;

        self.frontend
            .set_vring_kick(queue_idx, queue.kick_fd())
            .map_err(|e| io::Error::other(format!("set_vring_kick: {:?}", e)))?;

        self.frontend
            .set_vring_call(queue_idx, queue.call_fd())
            .map_err(|e| io::Error::other(format!("set_vring_call: {:?}", e)))?;

        Ok(())
    }

    /// Send a packet through the TX queue
    ///
    /// The packet should include the virtio-net header.
    pub fn send_packet(&mut self, data: &[u8]) -> io::Result<()> {
        // Allocate buffer in guest memory (TX region)
        let buf_addr = self.buf_region_start + (self.next_tx_buf as u64 * 4096);
        self.next_tx_buf = (self.next_tx_buf + 1) % QUEUE_SIZE;

        // Write packet to buffer
        self.mem
            .write_slice(data, GuestAddress(buf_addr))
            .map_err(|e| io::Error::other(format!("write packet: {:?}", e)))?;

        // Add to TX queue as a single buffer
        self.tx_queue
            .add_outbuf(&self.mem, &[(buf_addr, data.len() as u32)], buf_addr)?;

        // Kick the backend
        self.tx_queue.kick()?;

        Ok(())
    }

    /// Send a packet with separate header and data buffers (scatter-gather)
    ///
    /// This demonstrates descriptor chaining.
    pub fn send_packet_sg(&mut self, header: &[u8], data: &[u8]) -> io::Result<()> {
        // Allocate two buffers
        let hdr_addr = self.buf_region_start + (self.next_tx_buf as u64 * 4096);
        self.next_tx_buf = (self.next_tx_buf + 1) % QUEUE_SIZE;
        let data_addr = self.buf_region_start + (self.next_tx_buf as u64 * 4096);
        self.next_tx_buf = (self.next_tx_buf + 1) % QUEUE_SIZE;

        // Write header and data
        self.mem
            .write_slice(header, GuestAddress(hdr_addr))
            .map_err(|e| io::Error::other(format!("write header: {:?}", e)))?;
        self.mem
            .write_slice(data, GuestAddress(data_addr))
            .map_err(|e| io::Error::other(format!("write data: {:?}", e)))?;

        // Add chained buffers
        self.tx_queue.add_outbuf(
            &self.mem,
            &[
                (hdr_addr, header.len() as u32),
                (data_addr, data.len() as u32),
            ],
            hdr_addr,
        )?;

        self.tx_queue.kick()?;

        Ok(())
    }

    /// Provide a buffer for RX
    ///
    /// Returns the buffer address for later reading.
    pub fn provide_rx_buffer(&mut self, size: u32) -> io::Result<u64> {
        // RX buffers go in the second half of buffer region
        let buf_addr = self.buf_region_start + 0x100000 + (self.next_rx_buf as u64 * 4096);
        self.next_rx_buf = (self.next_rx_buf + 1) % QUEUE_SIZE;

        self.rx_queue
            .add_inbuf(&self.mem, buf_addr, size, buf_addr)?;
        self.rx_queue.kick()?;
        Ok(buf_addr)
    }

    /// Check for and receive a packet from RX queue
    ///
    /// Returns None if no packets are available.
    pub fn recv_packet(&mut self) -> io::Result<Option<Vec<u8>>> {
        if let Some(used) = self.rx_queue.pop_used(&self.mem)? {
            // The token is the buffer address
            let buf_addr = used.token;
            let mut data = vec![0u8; used.len as usize];
            self.mem
                .read_slice(&mut data, GuestAddress(buf_addr))
                .map_err(|e| io::Error::other(format!("read: {:?}", e)))?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    /// Wait for TX completion
    ///
    /// Returns true if a buffer was completed.
    pub fn wait_tx_complete(&mut self) -> io::Result<bool> {
        if self.tx_queue.pop_used(&self.mem)?.is_some() {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get a reference to the RX queue's call eventfd
    pub fn rx_call_fd(&self) -> &EventFd {
        self.rx_queue.call_fd()
    }

    /// Get a reference to the TX queue's call eventfd
    pub fn tx_call_fd(&self) -> &EventFd {
        self.tx_queue.call_fd()
    }

    /// Wait for a call eventfd with timeout
    pub fn wait_for_rx(&self, timeout_ms: u64) -> io::Result<bool> {
        wait_for_call(self.rx_queue.call_fd(), timeout_ms)
    }

    /// Wait for TX call eventfd with timeout
    pub fn wait_for_tx(&self, timeout_ms: u64) -> io::Result<bool> {
        wait_for_call(self.tx_queue.call_fd(), timeout_ms)
    }

    /// Get number of free TX descriptors
    pub fn tx_free(&self) -> u16 {
        self.tx_queue.num_free()
    }

    /// Get number of free RX descriptors
    pub fn rx_free(&self) -> u16 {
        self.rx_queue.num_free()
    }

    /// Check if there are used RX buffers
    pub fn has_rx_used(&self) -> io::Result<bool> {
        self.rx_queue.has_used(&self.mem)
    }

    /// Check if there are used TX buffers
    pub fn has_tx_used(&self) -> io::Result<bool> {
        self.tx_queue.has_used(&self.mem)
    }
}
