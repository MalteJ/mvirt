//! vhost-user frontend test - simulates the VM side to test the backend

#![allow(dead_code)]

use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::atomic::{Ordering, fence};
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

use iou::router::{Router, VhostConfig};

const QUEUE_SIZE: u16 = 256;
const MEM_SIZE: usize = 16 * 1024 * 1024; // 16 MB

// Virtio feature flags
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_F_RING_EVENT_IDX: u64 = 1 << 29;

// Queue indices
const RX_QUEUE: usize = 0;
const TX_QUEUE: usize = 1;

// Virtqueue layout constants
const DESC_SIZE: u64 = 16;
const AVAIL_RING_HEADER: u64 = 4;
const AVAIL_RING_ELEM: u64 = 2;
const USED_RING_HEADER: u64 = 4;
const USED_RING_ELEM: u64 = 8;

/// Virtio descriptor flags
const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Virtio net header (without mergeable buffers)
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    num_buffers: u16,
}

const VIRTIO_NET_HDR_SIZE: usize = std::mem::size_of::<VirtioNetHdr>();

/// A simple virtqueue implementation for the frontend (driver) side
struct VirtQueue {
    /// Queue size (number of descriptors)
    size: u16,
    /// Guest address of descriptor table
    desc_addr: u64,
    /// Guest address of available ring
    avail_addr: u64,
    /// Guest address of used ring
    used_addr: u64,
    /// Next descriptor to allocate
    next_desc: u16,
    /// Next index in available ring
    next_avail: u16,
    /// Last seen used index
    last_used: u16,
    /// Kick eventfd (signal backend)
    kick: EventFd,
    /// Call eventfd (signaled by backend)
    call: EventFd,
}

impl VirtQueue {
    fn new(size: u16, base_addr: u64) -> io::Result<Self> {
        let desc_addr = base_addr;
        let avail_addr = desc_addr + (size as u64 * DESC_SIZE);
        // Align used ring to 4 bytes
        let used_addr = (avail_addr + AVAIL_RING_HEADER + (size as u64 * AVAIL_RING_ELEM) + 3) & !3;

        Ok(VirtQueue {
            size,
            desc_addr,
            avail_addr,
            used_addr,
            next_desc: 0,
            next_avail: 0,
            last_used: 0,
            kick: EventFd::new(0).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
            call: EventFd::new(0).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
        })
    }

    /// Total size needed for this queue's rings
    fn total_size(size: u16) -> u64 {
        let desc_size = size as u64 * DESC_SIZE;
        let avail_size = AVAIL_RING_HEADER + (size as u64 * AVAIL_RING_ELEM);
        let used_size = USED_RING_HEADER + (size as u64 * USED_RING_ELEM);
        // Include alignment padding
        desc_size + ((avail_size + 3) & !3) + ((used_size + 3) & !3)
    }

    /// Initialize the queue structures in guest memory
    fn init<M: GuestMemory>(&self, mem: &M) -> io::Result<()> {
        // Zero out the queue area
        let total = Self::total_size(self.size);
        let zeros = vec![0u8; total as usize];
        mem.write_slice(&zeros, GuestAddress(self.desc_addr))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write failed: {:?}", e)))?;
        Ok(())
    }

    /// Add a buffer (descriptor chain) to the available ring
    fn add_buffer<M: GuestMemory>(
        &mut self,
        mem: &M,
        buf_addr: u64,
        buf_len: u32,
        write_only: bool,
    ) -> io::Result<u16> {
        let desc_idx = self.next_desc;
        self.next_desc = (self.next_desc + 1) % self.size;

        // Write descriptor
        let desc_offset = self.desc_addr + (desc_idx as u64 * DESC_SIZE);
        let flags: u16 = if write_only { VIRTQ_DESC_F_WRITE } else { 0 };

        // Descriptor: addr (8) + len (4) + flags (2) + next (2)
        mem.write_obj(buf_addr, GuestAddress(desc_offset))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write addr: {:?}", e)))?;
        mem.write_obj(buf_len, GuestAddress(desc_offset + 8))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write len: {:?}", e)))?;
        mem.write_obj(flags, GuestAddress(desc_offset + 12))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write flags: {:?}", e)))?;
        mem.write_obj(0u16, GuestAddress(desc_offset + 14))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write next: {:?}", e)))?;

        // Add to available ring
        let avail_idx = self.next_avail % self.size;
        let ring_offset =
            self.avail_addr + AVAIL_RING_HEADER + (avail_idx as u64 * AVAIL_RING_ELEM);
        mem.write_obj(desc_idx, GuestAddress(ring_offset))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write avail: {:?}", e)))?;

        // Memory barrier
        fence(Ordering::SeqCst);

        // Update available index
        self.next_avail = self.next_avail.wrapping_add(1);
        mem.write_obj(self.next_avail, GuestAddress(self.avail_addr + 2))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write idx: {:?}", e)))?;

        Ok(desc_idx)
    }

    /// Kick the backend to process the queue
    fn kick(&self) -> io::Result<()> {
        println!("    Kicking queue (fd={})", self.kick.as_raw_fd());
        self.kick
            .write(1)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        println!("    Kick sent");
        Ok(())
    }

    /// Check if there are used buffers available
    fn has_used<M: GuestMemory>(&self, mem: &M) -> io::Result<bool> {
        let used_idx: u16 = mem
            .read_obj(GuestAddress(self.used_addr + 2))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("read used idx: {:?}", e)))?;
        Ok(used_idx != self.last_used)
    }

    /// Get the next used buffer
    fn pop_used<M: GuestMemory>(&mut self, mem: &M) -> io::Result<Option<(u16, u32)>> {
        let used_idx: u16 = mem
            .read_obj(GuestAddress(self.used_addr + 2))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("read used idx: {:?}", e)))?;

        if used_idx == self.last_used {
            return Ok(None);
        }

        let ring_idx = self.last_used % self.size;
        let elem_offset = self.used_addr + USED_RING_HEADER + (ring_idx as u64 * USED_RING_ELEM);

        let id: u32 = mem
            .read_obj(GuestAddress(elem_offset))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("read id: {:?}", e)))?;
        let len: u32 = mem
            .read_obj(GuestAddress(elem_offset + 4))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("read len: {:?}", e)))?;

        self.last_used = self.last_used.wrapping_add(1);

        Ok(Some((id as u16, len)))
    }
}

/// Create an ICMP echo request packet with virtio-net header
fn create_icmp_echo_request(seq: u16, src_ip: [u8; 4], dst_ip: [u8; 4]) -> Vec<u8> {
    let mut packet = vec![0u8; VIRTIO_NET_HDR_SIZE + 20 + 8 + 56]; // hdr + IP + ICMP + data

    // Virtio net header (all zeros is fine)
    // ... already zeroed

    let ip_start = VIRTIO_NET_HDR_SIZE;
    let icmp_start = ip_start + 20;

    // IP header
    packet[ip_start] = 0x45; // version + IHL
    packet[ip_start + 1] = 0; // DSCP + ECN
    let total_len = (20 + 8 + 56) as u16;
    packet[ip_start + 2..ip_start + 4].copy_from_slice(&total_len.to_be_bytes());
    packet[ip_start + 4..ip_start + 6].copy_from_slice(&1u16.to_be_bytes()); // ID
    packet[ip_start + 6..ip_start + 8].copy_from_slice(&0u16.to_be_bytes()); // flags + frag
    packet[ip_start + 8] = 64; // TTL
    packet[ip_start + 9] = 1; // Protocol: ICMP
    // Checksum at 10-11, calculated later
    packet[ip_start + 12..ip_start + 16].copy_from_slice(&src_ip);
    packet[ip_start + 16..ip_start + 20].copy_from_slice(&dst_ip);

    // IP checksum
    let ip_csum = ip_checksum(&packet[ip_start..ip_start + 20]);
    packet[ip_start + 10..ip_start + 12].copy_from_slice(&ip_csum.to_be_bytes());

    // ICMP header
    packet[icmp_start] = 8; // Type: Echo Request
    packet[icmp_start + 1] = 0; // Code
    // Checksum at 2-3
    packet[icmp_start + 4..icmp_start + 6].copy_from_slice(&0x1234u16.to_be_bytes()); // ID
    packet[icmp_start + 6..icmp_start + 8].copy_from_slice(&seq.to_be_bytes());

    // ICMP checksum
    let icmp_csum = ip_checksum(&packet[icmp_start..]);
    packet[icmp_start + 2..icmp_start + 4].copy_from_slice(&icmp_csum.to_be_bytes());

    packet
}

fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        let word = if i + 1 < data.len() {
            ((data[i] as u32) << 8) | (data[i + 1] as u32)
        } else {
            (data[i] as u32) << 8
        };
        sum = sum.wrapping_add(word);
        i += 2;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// vhost-user frontend (VM simulator)
struct VhostUserFrontendDevice {
    frontend: Frontend,
    mem: GuestMemoryMmap,
    rx_queue: VirtQueue,
    tx_queue: VirtQueue,
    buf_region_start: u64,
}

impl VhostUserFrontendDevice {
    fn connect(socket_path: &str) -> io::Result<Self> {
        // Create file-backed memory using memfd
        let memfd = unsafe {
            let fd = libc::memfd_create(
                b"vhost-test-mem\0".as_ptr() as *const libc::c_char,
                libc::MFD_CLOEXEC,
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            File::from_raw_fd(fd)
        };

        // Set file size
        memfd.set_len(MEM_SIZE as u64)?;

        // Create guest memory region from file
        let mmap_region = vm_memory::MmapRegion::from_file(FileOffset::new(memfd, 0), MEM_SIZE)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("mmap region: {:?}", e)))?;

        let region = GuestRegionMmap::new(mmap_region, GuestAddress(0))
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "guest region creation failed"))?;

        let mem = GuestMemoryMmap::from_regions(vec![region])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("mmap failed: {:?}", e)))?;

        // Layout:
        // 0x0000_0000 - RX queue structures
        // 0x0001_0000 - TX queue structures
        // 0x0010_0000 - Buffer region (1MB+)
        let rx_queue_addr = 0x0000_0000u64;
        let tx_queue_addr = 0x0001_0000u64;
        let buf_region_start = 0x0010_0000u64;

        let rx_queue = VirtQueue::new(QUEUE_SIZE, rx_queue_addr)?;
        let tx_queue = VirtQueue::new(QUEUE_SIZE, tx_queue_addr)?;

        // Initialize queue structures
        rx_queue.init(&mem)?;
        tx_queue.init(&mem)?;

        // Connect to backend
        let frontend = Frontend::connect(socket_path, 2).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("connect failed: {:?}", e))
        })?;

        Ok(VhostUserFrontendDevice {
            frontend,
            mem,
            rx_queue,
            tx_queue,
            buf_region_start,
        })
    }

    fn setup(&mut self) -> io::Result<()> {
        println!("Frontend setup starting...");

        // Get and set features
        let features = self
            .frontend
            .get_features()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("get_features: {:?}", e)))?;
        println!("Backend features: 0x{:x}", features);

        let negotiated =
            features & (VIRTIO_F_VERSION_1 | VhostUserVirtioFeatures::PROTOCOL_FEATURES.bits());
        println!("Negotiated features: 0x{:x}", negotiated);
        self.frontend
            .set_features(negotiated)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_features: {:?}", e)))?;

        // Set owner
        self.frontend
            .set_owner()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_owner: {:?}", e)))?;
        println!("Owner set");

        // Get protocol features
        let proto_features = self.frontend.get_protocol_features().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("get_proto_features: {:?}", e))
        })?;
        println!("Protocol features: {:?}", proto_features);
        self.frontend
            .set_protocol_features(proto_features & VhostUserProtocolFeatures::CONFIG)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_proto_features: {:?}", e))
            })?;

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
        println!(
            "Memory region: gpa=0x{:x}, size={}, ua=0x{:x}, fd={}",
            mem_region.guest_phys_addr,
            mem_region.memory_size,
            mem_region.userspace_addr,
            mem_region.mmap_handle
        );
        self.frontend
            .set_mem_table(&[mem_region])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_mem_table: {:?}", e)))?;
        println!("Memory table set");

        // Wait for backend to process memory table
        std::thread::sleep(Duration::from_millis(50));
        println!("Memory table sync complete");

        // Setup RX queue
        println!("Setting up RX queue...");
        self.setup_queue(RX_QUEUE, &self.rx_queue)?;

        // Setup TX queue
        println!("Setting up TX queue...");
        self.setup_queue(TX_QUEUE, &self.tx_queue)?;

        // Enable queues
        println!("Enabling queues...");
        self.frontend
            .set_vring_enable(RX_QUEUE, true)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("enable rx: {:?}", e)))?;
        self.frontend
            .set_vring_enable(TX_QUEUE, true)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("enable tx: {:?}", e)))?;

        println!("Frontend setup complete");

        // Small delay to ensure backend has fully initialized
        std::thread::sleep(Duration::from_millis(100));
        println!("Post-setup delay complete");

        Ok(())
    }

    fn setup_queue(&self, queue_idx: usize, queue: &VirtQueue) -> io::Result<()> {
        println!("  set_vring_num({}, {})", queue_idx, queue.size);
        self.frontend
            .set_vring_num(queue_idx, queue.size)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_vring_num: {:?}", e)))?;

        // vhost-user protocol expects Host Virtual Addresses (HVA), not Guest Physical Addresses
        // Calculate HVA = host_base + (GPA - guest_base)
        let region = self.mem.iter().next().unwrap();
        let host_base = region.get_host_address(MemoryRegionAddress(0)).unwrap() as u64;
        let guest_base = region.start_addr().0;

        let desc_hva = host_base + (queue.desc_addr - guest_base);
        let avail_hva = host_base + (queue.avail_addr - guest_base);
        let used_hva = host_base + (queue.used_addr - guest_base);

        let config = VringConfigData {
            queue_max_size: queue.size,
            queue_size: queue.size,
            flags: 0,
            desc_table_addr: desc_hva,
            used_ring_addr: used_hva,
            avail_ring_addr: avail_hva,
            log_addr: None,
        };
        println!(
            "  set_vring_addr({}, desc=0x{:x} (HVA), avail=0x{:x}, used=0x{:x})",
            queue_idx, desc_hva, avail_hva, used_hva
        );
        self.frontend
            .set_vring_addr(queue_idx, &config)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_vring_addr: {:?}", e))
            })?;

        println!("  set_vring_base({}, 0)", queue_idx);
        self.frontend.set_vring_base(queue_idx, 0).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("set_vring_base: {:?}", e))
        })?;

        println!(
            "  set_vring_kick({}, fd={})",
            queue_idx,
            queue.kick.as_raw_fd()
        );
        self.frontend
            .set_vring_kick(queue_idx, &queue.kick)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_vring_kick: {:?}", e))
            })?;

        println!(
            "  set_vring_call({}, fd={})",
            queue_idx,
            queue.call.as_raw_fd()
        );
        self.frontend
            .set_vring_call(queue_idx, &queue.call)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_vring_call: {:?}", e))
            })?;

        println!("  Queue {} setup complete", queue_idx);
        Ok(())
    }

    /// Send a packet through the TX queue
    fn send_packet(&mut self, data: &[u8]) -> io::Result<()> {
        // Allocate buffer in guest memory
        let buf_addr = self.buf_region_start + (self.tx_queue.next_avail as u64 * 4096);

        // Write packet to buffer
        self.mem
            .write_slice(data, GuestAddress(buf_addr))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write packet: {:?}", e)))?;

        // Add to TX queue (read-only for device)
        self.tx_queue
            .add_buffer(&self.mem, buf_addr, data.len() as u32, false)?;

        // Kick the backend
        self.tx_queue.kick()?;

        Ok(())
    }

    /// Provide a buffer for RX
    fn provide_rx_buffer(&mut self, size: u32) -> io::Result<u64> {
        let buf_addr = self.buf_region_start + 0x100000 + (self.rx_queue.next_avail as u64 * 4096);
        self.rx_queue.add_buffer(&self.mem, buf_addr, size, true)?;
        self.rx_queue.kick()?;
        Ok(buf_addr)
    }

    /// Check for and receive a packet from RX queue
    fn recv_packet(&mut self) -> io::Result<Option<Vec<u8>>> {
        if let Some((desc_idx, len)) = self.rx_queue.pop_used(&self.mem)? {
            let buf_addr = self.buf_region_start + 0x100000 + (desc_idx as u64 * 4096);
            let mut data = vec![0u8; len as usize];
            self.mem
                .read_slice(&mut data, GuestAddress(buf_addr))
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("read: {:?}", e)))?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    /// Wait for TX completion
    fn wait_tx_complete(&mut self) -> io::Result<bool> {
        if let Some((_desc_idx, _len)) = self.tx_queue.pop_used(&self.mem)? {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Wait for a call eventfd to be signaled (with timeout)
fn wait_for_call(call: &EventFd, timeout_ms: u64) -> io::Result<bool> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::unix::io::BorrowedFd;

    // SAFETY: EventFd is valid for the duration of this function
    let borrowed = unsafe { BorrowedFd::borrow_raw(call.as_raw_fd()) };
    let poll_fd = PollFd::new(borrowed, PollFlags::POLLIN);
    let result = poll(&mut [poll_fd], PollTimeout::try_from(timeout_ms).unwrap())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    if result > 0 {
        // Read to clear the eventfd
        let _ = call.read();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tokio::test]
async fn test_vhost_user_ping() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-vhost-test.sock";
    let local_ip = std::net::Ipv4Addr::new(10, 99, 100, 1);
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend
    let router = Router::with_config_and_vhost(
        "tun_vhost",
        local_ip,
        24,
        4096,
        256,
        256,
        Some(VhostConfig::new(socket_path.to_string(), mac)),
    )
    .await
    .expect("Failed to start router");

    // Give backend time to create socket
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect frontend
    let mut frontend =
        VhostUserFrontendDevice::connect(socket_path).expect("Failed to connect frontend");

    if let Err(e) = frontend.setup() {
        eprintln!("Frontend setup failed: {:?}", e);
        // Give time for backend logs to appear
        tokio::time::sleep(Duration::from_millis(500)).await;
        router.shutdown().await.expect("Failed to shutdown router");
        panic!("Frontend setup failed: {}", e);
    }

    // Provide RX buffers for receiving the echo reply
    for _ in 0..8 {
        frontend
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer");
    }

    // Create and send an ICMP echo request
    // Source: 10.99.100.2 (simulated VM), Dest: 10.99.100.1 (router)
    let packet = create_icmp_echo_request(1, [10, 99, 100, 2], [10, 99, 100, 1]);
    println!("Sending ICMP echo request ({} bytes)", packet.len());
    frontend
        .send_packet(&packet)
        .expect("Failed to send packet");

    // Wait for TX call eventfd (backend signals completion)
    println!("Waiting for TX completion...");
    let tx_signaled = wait_for_call(&frontend.tx_queue.call, 1000).expect("TX call wait failed");

    if tx_signaled {
        println!("TX call signaled");
        assert!(
            frontend.wait_tx_complete().expect("TX check failed"),
            "TX not completed"
        );
        println!("TX completed");
    } else {
        // Check if maybe it completed without signal
        if frontend.wait_tx_complete().expect("TX check failed") {
            println!("TX completed (no signal)");
        } else {
            panic!("TX not completed - no signal and no used buffers");
        }
    }

    // Wait for RX call eventfd (backend sends echo reply)
    println!("Waiting for RX (echo reply)...");
    let rx_signaled = wait_for_call(&frontend.rx_queue.call, 2000).expect("RX call wait failed");

    if rx_signaled {
        println!("RX call signaled");
    }

    // Try to receive the echo reply
    if let Some(reply) = frontend.recv_packet().expect("RX recv failed") {
        println!("Received reply: {} bytes", reply.len());

        // Verify it's an ICMP echo reply
        // Skip virtio-net header (10 bytes), IP header starts at offset 10
        let ip_start = VIRTIO_NET_HDR_SIZE;
        let icmp_start = ip_start + 20;

        assert!(reply.len() >= icmp_start + 8, "Reply too short");
        assert_eq!(
            reply[icmp_start], 0,
            "Expected ICMP Echo Reply (type 0), got {}",
            reply[icmp_start]
        );
        println!("ICMP Echo Reply received!");
    } else {
        panic!("No echo reply received");
    }

    // Drop frontend first to close the connection
    // This allows the vhost daemon to exit cleanly
    drop(frontend);

    // Cleanup
    router.shutdown().await.expect("Failed to shutdown router");
}
