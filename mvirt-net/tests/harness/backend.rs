//! Test vhost-user backend implementation
//!
//! Provides a mock vhost-user backend for integration testing.

use std::io;
use std::net::Ipv4Addr;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use nix::libc;
use tempfile::TempDir;
use vhost::vhost_user::message::VhostUserProtocolFeatures;
use vhost::vhost_user::Listener;
use vhost_user_backend::{VhostUserBackend, VhostUserDaemon, VringRwLock, VringT};
use virtio_queue::QueueT;
use vm_memory::{Address, ByteValued, Bytes, GuestAddressSpace, GuestMemoryAtomic, GuestMemoryMmap, Le16};
use vmm_sys_util::epoll::EventSet;
use vmm_sys_util::eventfd::EventFd;

use super::VhostTestClient;

// ============================================================================
// Constants
// ============================================================================

/// Virtio feature flags
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;
const VIRTIO_F_RING_INDIRECT_DESC: u64 = 1 << 28;
const VHOST_USER_F_PROTOCOL_FEATURES: u64 = 1 << 30;

const VIRTIO_NET_HDR_SIZE: usize = 12;
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

/// Gateway MAC for ARP responses
pub const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
pub const GATEWAY_IP: [u8; 4] = [169, 254, 0, 1];

// ============================================================================
// Virtio Net Header
// ============================================================================

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: Le16,
    gso_size: Le16,
    csum_start: Le16,
    csum_offset: Le16,
    num_buffers: Le16,
}

unsafe impl ByteValued for VirtioNetHdr {}

// ============================================================================
// Test Backend Implementation
// ============================================================================

type PacketHandler = Box<dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync>;

struct BackendInner {
    mac: [u8; 6],
    mem: RwLock<GuestMemoryAtomic<GuestMemoryMmap>>,
    event_idx: RwLock<bool>,
    packet_handler: Mutex<Option<PacketHandler>>,
    rx_queue: Mutex<Vec<Vec<u8>>>,
}

#[derive(Clone)]
struct TestVhostNetBackend {
    inner: Arc<BackendInner>,
}

impl TestVhostNetBackend {
    fn new(mac: [u8; 6], ipv4: Option<Ipv4Addr>) -> Self {
        let inner = Arc::new(BackendInner {
            mac,
            mem: RwLock::new(GuestMemoryAtomic::new(GuestMemoryMmap::new())),
            event_idx: RwLock::new(false),
            packet_handler: Mutex::new(None),
            rx_queue: Mutex::new(Vec::new()),
        });

        let backend = Self { inner };

        // Setup handlers
        if let Some(ip) = ipv4 {
            let mac_copy = mac;
            let handler: PacketHandler = Box::new(move |packet| {
                if let Some(reply) = process_arp(packet, mac_copy) {
                    return Some(reply);
                }
                if let Some(reply) = process_dhcp(packet, mac_copy, ip) {
                    return Some(reply);
                }
                None
            });
            *backend.inner.packet_handler.lock().unwrap() = Some(handler);
        }

        backend
    }

    fn inject_packet(&self, packet: Vec<u8>) {
        self.inner.rx_queue.lock().unwrap().push(packet);
    }

    fn process_tx(&self, vring: &VringRwLock) -> io::Result<bool> {
        let mut used_descs = false;
        let mem_guard = self.inner.mem.read().unwrap();
        let mem = mem_guard.memory();

        loop {
            let mut vring_state = vring.get_mut();
            let avail_desc = match vring_state.get_queue_mut().pop_descriptor_chain(mem.clone()) {
                Some(desc) => desc,
                None => break,
            };

            let mut packet = Vec::new();
            for desc in avail_desc.clone() {
                if !desc.is_write_only() {
                    let len = desc.len() as usize;
                    let mut buf = vec![0u8; len];
                    mem.read(&mut buf, desc.addr())
                        .map_err(|e| io::Error::other(format!("read: {e}")))?;
                    packet.extend_from_slice(&buf);
                }
            }

            if packet.len() > VIRTIO_NET_HDR_SIZE {
                let eth_frame = &packet[VIRTIO_NET_HDR_SIZE..];
                let handler_guard = self.inner.packet_handler.lock().unwrap();
                if let Some(ref handler) = *handler_guard {
                    if let Some(response) = handler(eth_frame) {
                        self.inject_packet(response);
                    }
                }
            }

            let desc_idx = avail_desc.head_index();
            vring_state
                .get_queue_mut()
                .add_used(&*mem, desc_idx, 0)
                .map_err(|e| io::Error::other(format!("add_used: {e}")))?;

            used_descs = true;
        }

        Ok(used_descs)
    }

    fn process_rx(&self, vring: &VringRwLock) -> io::Result<bool> {
        let mut rx_queue = self.inner.rx_queue.lock().unwrap();
        if rx_queue.is_empty() {
            return Ok(false);
        }

        let mut used_descs = false;
        let mem_guard = self.inner.mem.read().unwrap();
        let mem = mem_guard.memory();

        while !rx_queue.is_empty() {
            let mut vring_state = vring.get_mut();
            let avail_desc = match vring_state.get_queue_mut().pop_descriptor_chain(mem.clone()) {
                Some(desc) => desc,
                None => break,
            };

            let packet = rx_queue.remove(0);
            let hdr = VirtioNetHdr::default();
            let hdr_bytes = hdr.as_slice();
            let total_len = hdr_bytes.len() + packet.len();

            let mut written = 0;
            for desc in avail_desc.clone() {
                if desc.is_write_only() && written < total_len {
                    let to_write = std::cmp::min(desc.len() as usize, total_len - written);

                    if written < hdr_bytes.len() {
                        let hdr_end = std::cmp::min(hdr_bytes.len() - written, to_write);
                        mem.write(&hdr_bytes[written..written + hdr_end], desc.addr())
                            .map_err(|e| io::Error::other(format!("write hdr: {e}")))?;

                        if hdr_end < to_write {
                            let pkt_end = to_write - hdr_end;
                            mem.write(
                                &packet[..pkt_end],
                                desc.addr().unchecked_add(hdr_end as u64),
                            )
                            .map_err(|e| io::Error::other(format!("write pkt: {e}")))?;
                        }
                    } else {
                        let pkt_offset = written - hdr_bytes.len();
                        mem.write(&packet[pkt_offset..pkt_offset + to_write], desc.addr())
                            .map_err(|e| io::Error::other(format!("write pkt: {e}")))?;
                    }

                    written += to_write;
                }
            }

            let desc_idx = avail_desc.head_index();
            vring_state
                .get_queue_mut()
                .add_used(&*mem, desc_idx, written as u32)
                .map_err(|e| io::Error::other(format!("add_used: {e}")))?;

            used_descs = true;
        }

        Ok(used_descs)
    }
}

impl VhostUserBackend for TestVhostNetBackend {
    type Bitmap = ();
    type Vring = VringRwLock;

    fn num_queues(&self) -> usize {
        2
    }

    fn max_queue_size(&self) -> usize {
        256
    }

    fn features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_NET_F_MAC
            | VIRTIO_NET_F_STATUS
            | VIRTIO_RING_F_EVENT_IDX
            | VIRTIO_F_RING_INDIRECT_DESC
            | VHOST_USER_F_PROTOCOL_FEATURES
    }

    fn protocol_features(&self) -> VhostUserProtocolFeatures {
        VhostUserProtocolFeatures::CONFIG
            | VhostUserProtocolFeatures::MQ
            | VhostUserProtocolFeatures::REPLY_ACK
    }

    fn set_event_idx(&self, enabled: bool) {
        *self.inner.event_idx.write().unwrap() = enabled;
    }

    fn update_memory(&self, mem: GuestMemoryAtomic<GuestMemoryMmap>) -> io::Result<()> {
        *self.inner.mem.write().unwrap() = mem;
        Ok(())
    }

    fn handle_event(
        &self,
        device_event: u16,
        evset: EventSet,
        vrings: &[Self::Vring],
        _thread_id: usize,
    ) -> io::Result<()> {
        if evset != EventSet::IN {
            return Ok(());
        }

        match device_event {
            RX_QUEUE => {
                if self.process_rx(&vrings[RX_QUEUE as usize])? {
                    vrings[RX_QUEUE as usize]
                        .signal_used_queue()
                        .map_err(|e| io::Error::other(format!("signal: {e}")))?;
                }
            }
            TX_QUEUE => {
                if self.process_tx(&vrings[TX_QUEUE as usize])? {
                    vrings[TX_QUEUE as usize]
                        .signal_used_queue()
                        .map_err(|e| io::Error::other(format!("signal: {e}")))?;

                    if self.process_rx(&vrings[RX_QUEUE as usize])? {
                        vrings[RX_QUEUE as usize]
                            .signal_used_queue()
                            .map_err(|e| io::Error::other(format!("signal: {e}")))?;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn get_config(&self, offset: u32, size: u32) -> Vec<u8> {
        let mut config = [0u8; 8];
        config[..6].copy_from_slice(&self.inner.mac);
        config[6..8].copy_from_slice(&[1, 0]); // LINK_UP

        let start = offset as usize;
        let end = std::cmp::min(start + size as usize, config.len());
        if start < config.len() {
            config[start..end].to_vec()
        } else {
            vec![]
        }
    }

    fn exit_event(&self, _thread_index: usize) -> Option<EventFd> {
        None
    }
}

// ============================================================================
// Packet Processing
// ============================================================================

fn process_arp(packet: &[u8], _nic_mac: [u8; 6]) -> Option<Vec<u8>> {
    use smoltcp::wire::{
        ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
        EthernetRepr, Ipv4Address,
    };

    let frame = EthernetFrame::new_checked(packet).ok()?;
    if frame.ethertype() != EthernetProtocol::Arp {
        return None;
    }

    let arp = ArpPacket::new_checked(frame.payload()).ok()?;
    let repr = ArpRepr::parse(&arp).ok()?;

    if let ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Request,
        source_hardware_addr,
        source_protocol_addr,
        target_protocol_addr,
        ..
    } = repr
    {
        if target_protocol_addr == Ipv4Address::from_bytes(&GATEWAY_IP) {
            let reply_arp = ArpRepr::EthernetIpv4 {
                operation: ArpOperation::Reply,
                source_hardware_addr: EthernetAddress::from_bytes(&GATEWAY_MAC),
                source_protocol_addr: target_protocol_addr,
                target_hardware_addr: source_hardware_addr,
                target_protocol_addr: source_protocol_addr,
            };

            let eth_repr = EthernetRepr {
                src_addr: EthernetAddress::from_bytes(&GATEWAY_MAC),
                dst_addr: source_hardware_addr,
                ethertype: EthernetProtocol::Arp,
            };

            let mut buffer = vec![0u8; eth_repr.buffer_len() + reply_arp.buffer_len()];
            let mut frame = EthernetFrame::new_unchecked(&mut buffer);
            eth_repr.emit(&mut frame);
            let mut arp_pkt = ArpPacket::new_unchecked(frame.payload_mut());
            reply_arp.emit(&mut arp_pkt);

            return Some(buffer);
        }
    }

    None
}

fn process_dhcp(packet: &[u8], client_mac: [u8; 6], assigned_ip: Ipv4Addr) -> Option<Vec<u8>> {
    use dhcproto::v4::{DhcpOption, Message, MessageType, Opcode, OptionCode};
    use dhcproto::{Decodable, Encodable};
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpProtocol, Ipv4Address,
        Ipv4Packet, Ipv4Repr, UdpPacket, UdpRepr,
    };

    let frame = EthernetFrame::new_checked(packet).ok()?;
    if frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ipv4 = Ipv4Packet::new_checked(frame.payload()).ok()?;
    if ipv4.next_header() != IpProtocol::Udp {
        return None;
    }

    let udp = UdpPacket::new_checked(ipv4.payload()).ok()?;
    if udp.dst_port() != 67 {
        return None;
    }

    let mut decoder = dhcproto::decoder::Decoder::new(udp.payload());
    let dhcp_msg = Message::decode(&mut decoder).ok()?;

    if dhcp_msg.opcode() != Opcode::BootRequest {
        return None;
    }

    let msg_type = dhcp_msg
        .opts()
        .get(OptionCode::MessageType)
        .and_then(|opt| {
            if let DhcpOption::MessageType(t) = opt {
                Some(*t)
            } else {
                None
            }
        })?;

    let response_type = match msg_type {
        MessageType::Discover => MessageType::Offer,
        MessageType::Request => MessageType::Ack,
        _ => return None,
    };

    let mut response = Message::default();
    response.set_opcode(Opcode::BootReply);
    response.set_htype(dhcp_msg.htype());
    response.set_xid(dhcp_msg.xid());
    response.set_flags(dhcp_msg.flags());
    response.set_yiaddr(assigned_ip);
    response.set_siaddr(Ipv4Addr::from(GATEWAY_IP));
    response.set_chaddr(dhcp_msg.chaddr());

    response
        .opts_mut()
        .insert(DhcpOption::MessageType(response_type));
    response
        .opts_mut()
        .insert(DhcpOption::ServerIdentifier(Ipv4Addr::from(GATEWAY_IP)));
    response
        .opts_mut()
        .insert(DhcpOption::AddressLeaseTime(86400));
    response
        .opts_mut()
        .insert(DhcpOption::SubnetMask(Ipv4Addr::new(255, 255, 255, 255)));
    response
        .opts_mut()
        .insert(DhcpOption::Router(vec![Ipv4Addr::from(GATEWAY_IP)]));
    response
        .opts_mut()
        .insert(DhcpOption::DomainNameServer(vec![
            Ipv4Addr::new(1, 1, 1, 1),
            Ipv4Addr::new(8, 8, 8, 8),
        ]));

    let dhcp_data = response.to_vec().ok()?;

    let gateway_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);
    let client_eth = EthernetAddress::from_bytes(&client_mac);

    let udp_repr = UdpRepr {
        src_port: 67,
        dst_port: 68,
    };
    let ipv4_repr = Ipv4Repr {
        src_addr: Ipv4Address::from_bytes(&GATEWAY_IP),
        dst_addr: Ipv4Address::BROADCAST,
        next_header: IpProtocol::Udp,
        payload_len: udp_repr.header_len() + dhcp_data.len(),
        hop_limit: 64,
    };
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: client_eth,
        ethertype: EthernetProtocol::Ipv4,
    };

    let total_len =
        eth_repr.buffer_len() + ipv4_repr.buffer_len() + udp_repr.header_len() + dhcp_data.len();
    let mut buffer = vec![0u8; total_len];

    let mut eth_frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut eth_frame);

    let mut ipv4_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
    ipv4_repr.emit(
        &mut ipv4_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    let mut udp_packet = UdpPacket::new_unchecked(ipv4_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &ipv4_repr.src_addr.into(),
        &ipv4_repr.dst_addr.into(),
        dhcp_data.len(),
        |buf| buf.copy_from_slice(&dhcp_data),
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    Some(buffer)
}

// ============================================================================
// TestBackend - Public API for Tests
// ============================================================================

/// Test backend that spawns a vhost-user daemon in a background thread.
///
/// # Example
///
/// ```ignore
/// let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));
/// let mut client = backend.connect().expect("connect failed");
/// // ... run tests
/// ```
pub struct TestBackend {
    _tmp_dir: TempDir,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    _thread: thread::JoinHandle<()>,
}

impl TestBackend {
    /// Create a new test backend with the given MAC and optional IPv4 address.
    ///
    /// If `ipv4` is provided, the backend will respond to ARP and DHCP requests.
    pub fn new(mac: &str, ipv4: Option<&str>) -> Self {
        let tmp_dir = TempDir::new().expect("Failed to create temp dir");
        let socket_path = tmp_dir.path().join("test.sock");
        let socket_str = socket_path.to_string_lossy().to_string();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let mac_bytes = parse_mac(mac).expect("Invalid MAC");
        let ipv4_addr = ipv4.map(|s| s.parse().expect("Invalid IP"));

        let thread = thread::spawn(move || {
            run_test_backend(&socket_str, mac_bytes, ipv4_addr, shutdown_clone);
        });

        // Wait for socket to appear
        for _ in 0..50 {
            if socket_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        Self {
            _tmp_dir: tmp_dir,
            socket_path,
            shutdown,
            _thread: thread,
        }
    }

    /// Connect a test client to this backend.
    pub fn connect(&self) -> std::io::Result<VhostTestClient> {
        VhostTestClient::connect(&self.socket_path)
    }
}

impl Drop for TestBackend {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 {
        return None;
    }

    let mut bytes = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        bytes[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(bytes)
}

fn run_test_backend(
    socket_path: &str,
    mac: [u8; 6],
    ipv4: Option<Ipv4Addr>,
    shutdown: Arc<AtomicBool>,
) {
    let backend = TestVhostNetBackend::new(mac, ipv4);

    let listener = Listener::new(socket_path, true).expect("Failed to create listener");

    eprintln!("[BACKEND] Listening on {}", socket_path);

    let mut daemon = VhostUserDaemon::new(
        "test-backend".to_string(),
        backend,
        GuestMemoryAtomic::new(GuestMemoryMmap::new()),
    )
    .expect("Failed to create daemon");

    while !shutdown.load(Ordering::SeqCst) {
        let mut pollfd = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        let ret = unsafe { libc::poll(&mut pollfd, 1, 500) };
        if ret <= 0 {
            continue;
        }

        eprintln!("[BACKEND] Accepting connection...");

        if let Err(e) = daemon.start(listener) {
            eprintln!("[BACKEND] Start error: {}", e);
            break;
        }

        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        break;
    }

    eprintln!("[BACKEND] Shutting down");
}
