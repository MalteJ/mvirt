//! VM-to-VM routing integration test
//!
//! Tests packet forwarding between two vhost-user devices through the router.
//! VM A sends a packet destined for VM B, which should be routed through
//! the shared reactor registry.

#![allow(dead_code)]

use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::Arc;
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

use mvirt_net::reactor::ReactorRegistry;
use mvirt_net::router::{Router, VhostConfig};
use mvirt_net::routing::{IpPrefix, RouteTarget};

const QUEUE_SIZE: u16 = 256;
const MEM_SIZE: usize = 16 * 1024 * 1024; // 16 MB

// Virtio feature flags
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

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
    size: u16,
    desc_addr: u64,
    avail_addr: u64,
    used_addr: u64,
    next_desc: u16,
    next_avail: u16,
    last_used: u16,
    kick: EventFd,
    call: EventFd,
}

impl VirtQueue {
    fn new(size: u16, base_addr: u64) -> io::Result<Self> {
        let desc_addr = base_addr;
        let avail_addr = desc_addr + (size as u64 * DESC_SIZE);
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

    fn total_size(size: u16) -> u64 {
        let desc_size = size as u64 * DESC_SIZE;
        let avail_size = AVAIL_RING_HEADER + (size as u64 * AVAIL_RING_ELEM);
        let used_size = USED_RING_HEADER + (size as u64 * USED_RING_ELEM);
        desc_size + ((avail_size + 3) & !3) + ((used_size + 3) & !3)
    }

    fn init<M: GuestMemory>(&self, mem: &M) -> io::Result<()> {
        let total = Self::total_size(self.size);
        let zeros = vec![0u8; total as usize];
        mem.write_slice(&zeros, GuestAddress(self.desc_addr))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write failed: {:?}", e)))?;
        Ok(())
    }

    fn add_buffer<M: GuestMemory>(
        &mut self,
        mem: &M,
        buf_addr: u64,
        buf_len: u32,
        write_only: bool,
    ) -> io::Result<u16> {
        let desc_idx = self.next_desc;
        self.next_desc = (self.next_desc + 1) % self.size;

        let desc_offset = self.desc_addr + (desc_idx as u64 * DESC_SIZE);
        let flags: u16 = if write_only { VIRTQ_DESC_F_WRITE } else { 0 };

        mem.write_obj(buf_addr, GuestAddress(desc_offset))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write addr: {:?}", e)))?;
        mem.write_obj(buf_len, GuestAddress(desc_offset + 8))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write len: {:?}", e)))?;
        mem.write_obj(flags, GuestAddress(desc_offset + 12))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write flags: {:?}", e)))?;
        mem.write_obj(0u16, GuestAddress(desc_offset + 14))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write next: {:?}", e)))?;

        let avail_idx = self.next_avail % self.size;
        let ring_offset =
            self.avail_addr + AVAIL_RING_HEADER + (avail_idx as u64 * AVAIL_RING_ELEM);
        mem.write_obj(desc_idx, GuestAddress(ring_offset))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write avail: {:?}", e)))?;

        fence(Ordering::SeqCst);

        self.next_avail = self.next_avail.wrapping_add(1);
        mem.write_obj(self.next_avail, GuestAddress(self.avail_addr + 2))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write idx: {:?}", e)))?;

        Ok(desc_idx)
    }

    fn kick(&self) -> io::Result<()> {
        self.kick
            .write(1)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

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

/// Create an ICMP echo request packet with virtio-net header
fn create_icmp_echo_request(seq: u16, src_ip: [u8; 4], dst_ip: [u8; 4]) -> Vec<u8> {
    let mut packet = vec![0u8; VIRTIO_NET_HDR_SIZE + 20 + 8 + 56]; // hdr + IP + ICMP + data

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
    packet[ip_start + 12..ip_start + 16].copy_from_slice(&src_ip);
    packet[ip_start + 16..ip_start + 20].copy_from_slice(&dst_ip);

    // IP checksum
    let ip_csum = ip_checksum(&packet[ip_start..ip_start + 20]);
    packet[ip_start + 10..ip_start + 12].copy_from_slice(&ip_csum.to_be_bytes());

    // ICMP header
    packet[icmp_start] = 8; // Type: Echo Request
    packet[icmp_start + 1] = 0; // Code
    packet[icmp_start + 4..icmp_start + 6].copy_from_slice(&0x1234u16.to_be_bytes()); // ID
    packet[icmp_start + 6..icmp_start + 8].copy_from_slice(&seq.to_be_bytes());

    // ICMP checksum
    let icmp_csum = ip_checksum(&packet[icmp_start..]);
    packet[icmp_start + 2..icmp_start + 4].copy_from_slice(&icmp_csum.to_be_bytes());

    packet
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

        memfd.set_len(MEM_SIZE as u64)?;

        let mmap_region = vm_memory::MmapRegion::from_file(FileOffset::new(memfd, 0), MEM_SIZE)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("mmap region: {:?}", e)))?;

        let region = GuestRegionMmap::new(mmap_region, GuestAddress(0))
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "guest region creation failed"))?;

        let mem = GuestMemoryMmap::from_regions(vec![region])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("mmap failed: {:?}", e)))?;

        let rx_queue_addr = 0x0000_0000u64;
        let tx_queue_addr = 0x0001_0000u64;
        let buf_region_start = 0x0010_0000u64;

        let rx_queue = VirtQueue::new(QUEUE_SIZE, rx_queue_addr)?;
        let tx_queue = VirtQueue::new(QUEUE_SIZE, tx_queue_addr)?;

        rx_queue.init(&mem)?;
        tx_queue.init(&mem)?;

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
        let features = self
            .frontend
            .get_features()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("get_features: {:?}", e)))?;

        let negotiated =
            features & (VIRTIO_F_VERSION_1 | VhostUserVirtioFeatures::PROTOCOL_FEATURES.bits());
        self.frontend
            .set_features(negotiated)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_features: {:?}", e)))?;

        self.frontend
            .set_owner()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_owner: {:?}", e)))?;

        let proto_features = self.frontend.get_protocol_features().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("get_proto_features: {:?}", e))
        })?;
        self.frontend
            .set_protocol_features(proto_features & VhostUserProtocolFeatures::CONFIG)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_proto_features: {:?}", e))
            })?;

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
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_mem_table: {:?}", e)))?;

        std::thread::sleep(Duration::from_millis(50));

        self.setup_queue(RX_QUEUE, &self.rx_queue)?;
        self.setup_queue(TX_QUEUE, &self.tx_queue)?;

        self.frontend
            .set_vring_enable(RX_QUEUE, true)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("enable rx: {:?}", e)))?;
        self.frontend
            .set_vring_enable(TX_QUEUE, true)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("enable tx: {:?}", e)))?;

        std::thread::sleep(Duration::from_millis(100));

        Ok(())
    }

    fn setup_queue(&self, queue_idx: usize, queue: &VirtQueue) -> io::Result<()> {
        self.frontend
            .set_vring_num(queue_idx, queue.size)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("set_vring_num: {:?}", e)))?;

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
        self.frontend
            .set_vring_addr(queue_idx, &config)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_vring_addr: {:?}", e))
            })?;

        self.frontend.set_vring_base(queue_idx, 0).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("set_vring_base: {:?}", e))
        })?;

        self.frontend
            .set_vring_kick(queue_idx, &queue.kick)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_vring_kick: {:?}", e))
            })?;

        self.frontend
            .set_vring_call(queue_idx, &queue.call)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("set_vring_call: {:?}", e))
            })?;

        Ok(())
    }

    fn send_packet(&mut self, data: &[u8]) -> io::Result<()> {
        let buf_addr = self.buf_region_start + (self.tx_queue.next_avail as u64 * 4096);

        self.mem
            .write_slice(data, GuestAddress(buf_addr))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("write packet: {:?}", e)))?;

        self.tx_queue
            .add_buffer(&self.mem, buf_addr, data.len() as u32, false)?;
        self.tx_queue.kick()?;

        Ok(())
    }

    fn provide_rx_buffer(&mut self, size: u32) -> io::Result<u64> {
        let buf_addr = self.buf_region_start + 0x100000 + (self.rx_queue.next_avail as u64 * 4096);
        self.rx_queue.add_buffer(&self.mem, buf_addr, size, true)?;
        self.rx_queue.kick()?;
        Ok(buf_addr)
    }

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

    let borrowed = unsafe { BorrowedFd::borrow_raw(call.as_raw_fd()) };
    let poll_fd = PollFd::new(borrowed, PollFlags::POLLIN);
    let result = poll(&mut [poll_fd], PollTimeout::try_from(timeout_ms).unwrap())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    if result > 0 {
        let _ = call.read();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Test VM-to-VM packet forwarding through the router.
///
/// Setup:
/// - VM A (10.200.1.2) connected via vhost socket A
/// - VM B (10.200.2.2) connected via vhost socket B
/// - Shared reactor registry between both routers
/// - Routes configured so VM A's traffic to 10.200.2.0/24 goes to VM B's reactor
///
/// Test:
/// - VM A sends ICMP echo request to VM B (10.200.2.2)
/// - Verify VM B receives the packet
#[tokio::test]
async fn test_vm_to_vm_routing() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_a = "/tmp/iou-vm-a.sock";
    let socket_b = "/tmp/iou-vm-b.sock";

    // IP addresses
    let router_a_ip = std::net::Ipv4Addr::new(10, 200, 1, 1);
    let router_b_ip = std::net::Ipv4Addr::new(10, 200, 2, 1);
    let vm_a_ip: [u8; 4] = [10, 200, 1, 2];
    let vm_b_ip: [u8; 4] = [10, 200, 2, 2];

    let mac_a = [0x52, 0x54, 0x00, 0xAA, 0xBB, 0x01];
    let mac_b = [0x52, 0x54, 0x00, 0xAA, 0xBB, 0x02];

    // Clean up any stale sockets
    let _ = std::fs::remove_file(socket_a);
    let _ = std::fs::remove_file(socket_b);

    // Create shared registry for VM-to-VM communication
    let registry = Arc::new(ReactorRegistry::new());

    // Start router A with vhost-user backend
    println!("Starting Router A...");
    let router_a = Router::with_shared_registry(
        "tun_vm_a",
        Some((router_a_ip, 24)),
        4096,
        256,
        256,
        Some(VhostConfig::new(socket_a.to_string(), mac_a)),
        Arc::clone(&registry),
    )
    .await
    .expect("Failed to start router A");

    let reactor_a_id = router_a.reactor_id();
    println!("Router A started with reactor ID: {}", reactor_a_id);

    // Start router B with vhost-user backend
    println!("Starting Router B...");
    let router_b = Router::with_shared_registry(
        "tun_vm_b",
        Some((router_b_ip, 24)),
        4096,
        256,
        256,
        Some(VhostConfig::new(socket_b.to_string(), mac_b)),
        Arc::clone(&registry),
    )
    .await
    .expect("Failed to start router B");

    let reactor_b_id = router_b.reactor_id();
    println!("Router B started with reactor ID: {}", reactor_b_id);

    // Configure routes:
    // Router A: 10.200.2.0/24 -> Router B's reactor
    // Router B: 10.200.1.0/24 -> Router A's reactor
    println!("Configuring routes...");

    // Create routing tables and add routes
    let table_id = uuid::Uuid::new_v4();
    router_a.reactor_handle().create_table(table_id, "default");
    router_a.reactor_handle().add_route(
        table_id,
        IpPrefix::V4(ipnet::Ipv4Net::new(std::net::Ipv4Addr::new(10, 200, 2, 0), 24).unwrap()),
        RouteTarget::Reactor { id: reactor_b_id },
    );
    router_a.reactor_handle().set_default_table(table_id);

    let table_id_b = uuid::Uuid::new_v4();
    router_b
        .reactor_handle()
        .create_table(table_id_b, "default");
    router_b.reactor_handle().add_route(
        table_id_b,
        IpPrefix::V4(ipnet::Ipv4Net::new(std::net::Ipv4Addr::new(10, 200, 1, 0), 24).unwrap()),
        RouteTarget::Reactor { id: reactor_a_id },
    );
    router_b.reactor_handle().set_default_table(table_id_b);

    // Give routers time to process route updates
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Give backends time to create sockets
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect VM A frontend
    println!("Connecting VM A frontend...");
    let mut frontend_a =
        VhostUserFrontendDevice::connect(socket_a).expect("Failed to connect frontend A");
    frontend_a.setup().expect("Failed to setup frontend A");

    // Connect VM B frontend
    println!("Connecting VM B frontend...");
    let mut frontend_b =
        VhostUserFrontendDevice::connect(socket_b).expect("Failed to connect frontend B");
    frontend_b.setup().expect("Failed to setup frontend B");

    // Provide RX buffers for VM B to receive the packet
    println!("Providing RX buffers for VM B...");
    for _ in 0..8 {
        frontend_b
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer");
    }

    // VM A sends ICMP echo request to VM B
    let packet = create_icmp_echo_request(1, vm_a_ip, vm_b_ip);
    println!(
        "VM A sending ICMP echo request to VM B ({} bytes)...",
        packet.len()
    );
    frontend_a
        .send_packet(&packet)
        .expect("Failed to send packet");

    // For VM-to-VM routing, the TX completion on VM A comes AFTER:
    // 1. Router A routes packet to Router B
    // 2. Router B receives and copies to VM B's RX
    // 3. Router B sends CompletionNotify back to Router A
    // 4. Router A returns descriptor to VM A
    //
    // So we first wait for VM B to receive, then check TX completion on VM A

    // Wait for RX on VM B
    println!("Waiting for VM B to receive packet...");
    let rx_signaled = wait_for_call(&frontend_b.rx_queue.call, 5000).expect("RX call wait failed");

    if rx_signaled {
        println!("VM B RX call signaled");
    }

    // Try to receive the packet on VM B
    if let Some(received) = frontend_b.recv_packet().expect("RX recv failed") {
        println!("VM B received packet: {} bytes", received.len());

        // Verify it's the ICMP echo request we sent
        let ip_start = VIRTIO_NET_HDR_SIZE;
        let icmp_start = ip_start + 20;

        assert!(
            received.len() >= icmp_start + 8,
            "Received packet too short"
        );

        // Check IP source and destination
        let src_ip = &received[ip_start + 12..ip_start + 16];
        let dst_ip = &received[ip_start + 16..ip_start + 20];

        assert_eq!(src_ip, &vm_a_ip, "Source IP mismatch");
        assert_eq!(dst_ip, &vm_b_ip, "Destination IP mismatch");

        // Check ICMP type (should be Echo Request = 8)
        assert_eq!(
            received[icmp_start], 8,
            "Expected ICMP Echo Request (type 8), got {}",
            received[icmp_start]
        );

        println!("VM-to-VM routing test PASSED: VM B received ICMP Echo Request from VM A");

        // Now check TX completion on VM A (should be complete after RX delivery)
        println!("Checking VM A TX completion...");

        // Give a little time for completion notification to propagate
        std::thread::sleep(Duration::from_millis(100));

        // Check for TX call signal (completion notification triggers this)
        let tx_signaled =
            wait_for_call(&frontend_a.tx_queue.call, 2000).expect("TX call wait failed");
        if tx_signaled {
            println!("VM A TX call signaled");
        }

        if frontend_a.wait_tx_complete().expect("TX check failed") {
            println!("VM A TX completed (descriptor returned)");
        } else {
            println!("Warning: VM A TX descriptor not yet returned");
        }
    } else {
        panic!("VM B did not receive any packet - routing may have failed");
    }

    // Cleanup - signal shutdown before dropping frontends to suppress
    // expected "Disconnected" error messages
    router_a.prepare_shutdown();
    router_b.prepare_shutdown();

    drop(frontend_a);
    drop(frontend_b);

    router_a
        .shutdown()
        .await
        .expect("Failed to shutdown router A");
    router_b
        .shutdown()
        .await
        .expect("Failed to shutdown router B");
}
