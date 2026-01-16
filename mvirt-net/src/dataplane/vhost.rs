//! vhost-user backend implementation for virtio-net
//!
//! This module implements the VhostUserBackend trait for handling
//! virtio-net devices over the vhost-user protocol.

use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tracing::{debug, trace};
use vhost::vhost_user::message::VhostUserProtocolFeatures;
use vhost_user_backend::{VhostUserBackend, VringRwLock, VringT};
use virtio_queue::QueueT;
use vm_memory::{
    Address, ByteValued, Bytes, GuestAddressSpace, GuestMemoryAtomic, GuestMemoryMmap, Le16,
};
use vmm_sys_util::epoll::EventSet;
use vmm_sys_util::event::{
    EventConsumer, EventFlag, EventNotifier, new_event_consumer_and_notifier,
};

use crate::config::NicEntry;

use super::buffer::BufferPool;

/// Virtio net header size (without mergeable rx buffers)
const VIRTIO_NET_HDR_SIZE: usize = 12;

/// Queue indices
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

/// Virtio net features we support
const VIRTIO_NET_F_CSUM: u64 = 1 << 0; // Checksum offload
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_GUEST_TSO4: u64 = 1 << 7; // Guest can receive TSO IPv4
const VIRTIO_NET_F_GUEST_TSO6: u64 = 1 << 8; // Guest can receive TSO IPv6
const VIRTIO_NET_F_HOST_TSO4: u64 = 1 << 11; // Host can receive TSO IPv4
const VIRTIO_NET_F_HOST_TSO6: u64 = 1 << 12; // Host can receive TSO IPv6
const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15; // Mergeable RX buffers (for GRO)
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;
const VIRTIO_F_RING_INDIRECT_DESC: u64 = 1 << 28;
const VHOST_USER_F_PROTOCOL_FEATURES: u64 = 1 << 30;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// GSO types from virtio spec
pub const VIRTIO_NET_HDR_GSO_NONE: u8 = 0;
pub const VIRTIO_NET_HDR_GSO_TCPV4: u8 = 1;
pub const VIRTIO_NET_HDR_GSO_UDP: u8 = 3;
pub const VIRTIO_NET_HDR_GSO_TCPV6: u8 = 4;
pub const VIRTIO_NET_HDR_GSO_UDP_L4: u8 = 5;

/// Virtio-net header flags
pub const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1; // Guest requests checksum computation
pub const VIRTIO_NET_HDR_F_DATA_VALID: u8 = 2; // Checksum is valid (host validated)
pub const VIRTIO_NET_HDR_F_RSC_INFO: u8 = 4; // RSC info in csum fields

/// Virtio net header
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: Le16,
    pub gso_size: Le16,
    pub csum_start: Le16,
    pub csum_offset: Le16,
    pub num_buffers: Le16,
}

// SAFETY: VirtioNetHdr contains only POD types
unsafe impl ByteValued for VirtioNetHdr {}

/// Packet handler callback type
pub type PacketHandler = Box<dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync>;

/// vhost-user backend for a single vNIC
pub struct VhostNetBackend {
    /// NIC configuration
    nic_config: NicEntry,

    /// Guest memory (interior mutability for thread safety)
    mem: RwLock<GuestMemoryAtomic<GuestMemoryMmap>>,

    /// Event index enabled
    event_idx: RwLock<bool>,

    /// Shutdown flag
    #[allow(dead_code)]
    shutdown: Arc<AtomicBool>,

    /// Packet handler (processes TX packets, returns response packets)
    packet_handler: Mutex<Option<PacketHandler>>,

    /// Pending RX packets to inject into guest
    rx_queue: Mutex<Vec<Vec<u8>>>,

    /// Stored vrings for external RX injection (set on first handle_event)
    vrings: RwLock<Option<Vec<VringRwLock>>>,

    /// Exit event for signaling worker threads to terminate (consumer, notifier)
    exit_event: (EventConsumer, EventNotifier),

    /// Buffer pool for zero-copy TX processing
    pool: Arc<BufferPool>,
}

impl VhostNetBackend {
    /// Create a new vhost-user net backend
    pub fn new(
        nic_config: NicEntry,
        shutdown: Arc<AtomicBool>,
        pool: Arc<BufferPool>,
    ) -> io::Result<Self> {
        debug!(nic_id = %nic_config.id, "Creating VhostNetBackend");
        let exit_event = new_event_consumer_and_notifier(EventFlag::NONBLOCK)?;
        Ok(Self {
            nic_config,
            mem: RwLock::new(GuestMemoryAtomic::new(GuestMemoryMmap::new())),
            event_idx: RwLock::new(false),
            shutdown,
            packet_handler: Mutex::new(None),
            rx_queue: Mutex::new(Vec::new()),
            vrings: RwLock::new(None),
            exit_event,
            pool,
        })
    }

    /// Set the packet handler
    pub fn set_packet_handler(&self, handler: PacketHandler) {
        let mut ph = self.packet_handler.lock().unwrap();
        *ph = Some(handler);
    }

    /// Inject a packet into the guest's RX queue
    pub fn inject_packet(&self, packet: Vec<u8>) {
        debug!(
            packet_len = packet.len(),
            "inject_packet: queuing packet for RX"
        );
        let mut rx = self.rx_queue.lock().unwrap();
        rx.push(packet);
    }

    /// Inject a packet from a slice and immediately deliver it to the guest
    /// This is used for routed packets from other vNICs (zero-copy friendly)
    pub fn inject_and_deliver_slice(&self, packet: &[u8]) {
        trace!(packet_len = packet.len(), "Injecting packet to RX queue");
        // Add to queue (copy is needed since rx_queue stores Vec<u8>)
        {
            let mut rx = self.rx_queue.lock().unwrap();
            rx.push(packet.to_vec());
        }

        // Try to deliver immediately if vrings are available
        let vrings_guard = self.vrings.read().unwrap();
        if let Some(ref vrings) = *vrings_guard
            && let Some(rx_vring) = vrings.get(RX_QUEUE as usize)
        {
            // Process RX queue
            let _ = self.process_rx(rx_vring);
            // Always signal for externally injected packets - EVENT_IDX suppression
            // can cause missed notifications when guest is idle waiting for packets
            let _ = rx_vring.signal_used_queue();
        }
    }

    /// Inject a packet and immediately deliver it to the guest
    /// This is used for routed packets from other vNICs
    pub fn inject_and_deliver(&self, packet: Vec<u8>) {
        // Add to queue
        {
            let mut rx = self.rx_queue.lock().unwrap();
            rx.push(packet);
        }

        // Try to deliver immediately if vrings are available
        let vrings_guard = self.vrings.read().unwrap();
        if let Some(ref vrings) = *vrings_guard
            && let Some(rx_vring) = vrings.get(RX_QUEUE as usize)
        {
            let _ = self.process_rx(rx_vring);
            // Always signal - EVENT_IDX suppression can miss notifications
            let _ = rx_vring.signal_used_queue();
        }
    }

    /// Get the MAC address as bytes
    fn mac_bytes(&self) -> [u8; 6] {
        parse_mac(&self.nic_config.mac_address).unwrap_or([0x52, 0x54, 0x00, 0x00, 0x00, 0x00])
    }

    /// Process TX queue (packets from guest)
    /// Returns whether the guest needs to be notified (for EVENT_IDX)
    fn process_tx(&self, vring: &VringRwLock) -> io::Result<bool> {
        let mem_guard = self.mem.read().unwrap();
        let mem = mem_guard.memory();
        let mut processed_count = 0u32;

        loop {
            let mut vring_state = vring.get_mut();
            let queue = vring_state.get_queue_mut();

            let avail_desc = match queue.pop_descriptor_chain(mem.clone()) {
                Some(desc) => desc,
                None => break,
            };
            processed_count += 1;

            // Allocate buffer from pool for zero-copy packet processing
            let Some(mut buffer) = self.pool.alloc() else {
                // Pool exhausted - fall back to Vec allocation
                trace!("Buffer pool exhausted in TX, using Vec fallback");
                let mut packet = Vec::new();
                for desc in avail_desc.clone() {
                    if !desc.is_write_only() {
                        let len = desc.len() as usize;
                        let mut buf = vec![0u8; len];
                        mem.read(&mut buf, desc.addr())
                            .map_err(|e| io::Error::other(format!("Failed to read desc: {e}")))?;
                        packet.extend_from_slice(&buf);
                    }
                }
                if packet.len() > VIRTIO_NET_HDR_SIZE {
                    // Parse virtio-net header to check for checksum offload
                    let flags = packet[0];
                    let csum_start = u16::from_le_bytes([packet[6], packet[7]]);
                    let csum_offset = u16::from_le_bytes([packet[8], packet[9]]);

                    // Finalize checksum if guest requested it
                    if flags & VIRTIO_NET_HDR_F_NEEDS_CSUM != 0 {
                        finalize_checksum(
                            &mut packet[VIRTIO_NET_HDR_SIZE..],
                            csum_start,
                            csum_offset,
                        );
                    }

                    let eth_frame = &packet[VIRTIO_NET_HDR_SIZE..];
                    let handler_guard = self.packet_handler.lock().unwrap();
                    if let Some(ref handler) = *handler_guard
                        && let Some(response) = handler(eth_frame)
                    {
                        self.inject_packet(response);
                    }
                }
                // Mark descriptor as used and continue
                let desc_idx = avail_desc.head_index();
                queue
                    .add_used(&*mem, desc_idx, 0)
                    .map_err(|e| io::Error::other(format!("Failed to add used: {e}")))?;
                let more_work = queue
                    .enable_notification(&*mem)
                    .map_err(|e| io::Error::other(format!("Failed to enable notification: {e}")))?;
                if !more_work {
                    break;
                }
                continue;
            };

            // Read descriptors directly into pool buffer (zero-copy from guest memory)
            {
                let write_area = buffer.write_area();
                let mut offset = 0;
                for desc in avail_desc.clone() {
                    if !desc.is_write_only() {
                        let len = desc.len() as usize;
                        if offset + len > write_area.len() {
                            // Packet too large for buffer, truncate
                            break;
                        }
                        mem.read(&mut write_area[offset..offset + len], desc.addr())
                            .map_err(|e| io::Error::other(format!("Failed to read desc: {e}")))?;
                        offset += len;
                    }
                }
                buffer.len = offset;
            }

            // Process virtio-net header and packet
            if buffer.len > VIRTIO_NET_HDR_SIZE {
                // Parse virtio-net header to check for checksum offload
                let hdr_data = buffer.data();
                let flags = hdr_data[0];
                let csum_start = u16::from_le_bytes([hdr_data[6], hdr_data[7]]);
                let csum_offset = u16::from_le_bytes([hdr_data[8], hdr_data[9]]);

                // Adjust buffer to point past virtio header (zero-copy!)
                buffer.start += VIRTIO_NET_HDR_SIZE;
                buffer.len -= VIRTIO_NET_HDR_SIZE;

                // Finalize checksum if guest requested it
                if flags & VIRTIO_NET_HDR_F_NEEDS_CSUM != 0 {
                    finalize_checksum(buffer.data_mut(), csum_start, csum_offset);
                }

                let handler_guard = self.packet_handler.lock().unwrap();
                if let Some(ref handler) = *handler_guard
                    && let Some(response) = handler(buffer.data())
                {
                    self.inject_packet(response);
                }
            }

            // Mark descriptor as used
            let desc_idx = avail_desc.head_index();
            queue
                .add_used(&*mem, desc_idx, 0)
                .map_err(|e| io::Error::other(format!("Failed to add used: {e}")))?;

            // With EVENT_IDX: check if we should continue processing
            // enable_notification updates avail_event and returns true if more work is available
            let more_work = queue
                .enable_notification(&*mem)
                .map_err(|e| io::Error::other(format!("Failed to enable notification: {e}")))?;

            if !more_work {
                break;
            }
        }

        // Determine if we need to notify the guest
        // With EVENT_IDX this checks the used_event value set by the driver
        if processed_count == 0 {
            return Ok(false);
        }

        let mut vring_state = vring.get_mut();
        let needs_notification = vring_state
            .get_queue_mut()
            .needs_notification(&*mem)
            .map_err(|e| io::Error::other(format!("Failed to check needs_notification: {e}")))?;

        Ok(needs_notification)
    }

    /// Process RX queue (inject packets to guest)
    /// Returns whether the guest needs to be notified (for EVENT_IDX)
    fn process_rx(&self, vring: &VringRwLock) -> io::Result<bool> {
        let mut rx_queue = self.rx_queue.lock().unwrap();
        if rx_queue.is_empty() {
            return Ok(false);
        }

        let mem_guard = self.mem.read().unwrap();
        let mem = mem_guard.memory();
        let mut processed_count = 0u32;

        while !rx_queue.is_empty() {
            let mut vring_state = vring.get_mut();
            let queue = vring_state.get_queue_mut();

            let avail_desc = match queue.pop_descriptor_chain(mem.clone()) {
                Some(desc) => desc,
                None => {
                    // Guest hasn't provided enough RX buffers
                    break;
                }
            };

            let packet = rx_queue.remove(0);

            // Build virtio-net header + packet
            // IMPORTANT: With MRG_RXBUF, num_buffers must be set to 1 for single-buffer packets
            let hdr = VirtioNetHdr {
                num_buffers: Le16::from(1),
                ..Default::default()
            };
            let hdr_bytes = hdr.as_slice();
            let total_len = hdr_bytes.len() + packet.len();

            // Write virtio header + packet to descriptor chain
            let mut written = 0;
            for desc in avail_desc.clone() {
                if desc.is_write_only() && written < total_len {
                    let to_write = std::cmp::min(desc.len() as usize, total_len - written);

                    if written < hdr_bytes.len() {
                        // Write header first
                        let hdr_end = std::cmp::min(hdr_bytes.len() - written, to_write);
                        mem.write(&hdr_bytes[written..written + hdr_end], desc.addr())
                            .map_err(|e| io::Error::other(format!("Failed to write hdr: {e}")))?;

                        // Write packet data if space remains in this descriptor
                        if hdr_end < to_write {
                            let pkt_end = to_write - hdr_end;
                            mem.write(
                                &packet[..pkt_end],
                                desc.addr().unchecked_add(hdr_end as u64),
                            )
                            .map_err(|e| io::Error::other(format!("Failed to write pkt: {e}")))?;
                        }
                    } else {
                        // Write only packet data (header already written)
                        let pkt_offset = written - hdr_bytes.len();
                        mem.write(&packet[pkt_offset..pkt_offset + to_write], desc.addr())
                            .map_err(|e| io::Error::other(format!("Failed to write pkt: {e}")))?;
                    }

                    written += to_write;
                }
            }

            let desc_idx = avail_desc.head_index();
            queue
                .add_used(&*mem, desc_idx, written as u32)
                .map_err(|e| io::Error::other(format!("Failed to add used: {e}")))?;

            processed_count += 1;
            trace!(
                packet_len = packet.len(),
                written_bytes = written,
                desc_idx = desc_idx,
                "process_rx: delivered packet to guest"
            );

            // With EVENT_IDX: check if we should continue processing
            let more_work = queue
                .enable_notification(&*mem)
                .map_err(|e| io::Error::other(format!("Failed to enable notification: {e}")))?;

            if !more_work {
                break;
            }
        }

        // Determine if we need to notify the guest
        if processed_count == 0 {
            return Ok(false);
        }

        trace!(
            processed_count = processed_count,
            remaining = rx_queue.len(),
            "Delivered RX packets"
        );

        let mut vring_state = vring.get_mut();
        let needs_notification = vring_state
            .get_queue_mut()
            .needs_notification(&*mem)
            .map_err(|e| io::Error::other(format!("Failed to check needs_notification: {e}")))?;

        Ok(needs_notification)
    }
}

impl VhostUserBackend for VhostNetBackend {
    type Bitmap = ();
    type Vring = VringRwLock;

    fn num_queues(&self) -> usize {
        2 // RX and TX queues
    }

    fn max_queue_size(&self) -> usize {
        256
    }

    fn features(&self) -> u64 {
        // VHOST_USER_F_PROTOCOL_FEATURES is required by cloud-hypervisor
        // When set, vrings must be enabled via SET_VRING_ENABLE messages
        let f = VIRTIO_F_VERSION_1
            | VIRTIO_NET_F_CSUM
            | VIRTIO_NET_F_MAC
            | VIRTIO_NET_F_GUEST_TSO4
            | VIRTIO_NET_F_GUEST_TSO6
            | VIRTIO_NET_F_HOST_TSO4
            | VIRTIO_NET_F_HOST_TSO6
            | VIRTIO_NET_F_MRG_RXBUF
            | VIRTIO_NET_F_STATUS
            | VIRTIO_RING_F_EVENT_IDX
            | VIRTIO_F_RING_INDIRECT_DESC
            | VHOST_USER_F_PROTOCOL_FEATURES;
        debug!(features = format!("{:#x}", f), "Returning virtio features");
        f
    }

    fn protocol_features(&self) -> VhostUserProtocolFeatures {
        let pf = VhostUserProtocolFeatures::CONFIG
            | VhostUserProtocolFeatures::MQ
            | VhostUserProtocolFeatures::REPLY_ACK;
        debug!(?pf, "Returning protocol features");
        pf
    }

    fn set_event_idx(&self, enabled: bool) {
        debug!(enabled, "Setting event_idx");
        *self.event_idx.write().unwrap() = enabled;
    }

    fn update_memory(&self, mem: GuestMemoryAtomic<GuestMemoryMmap>) -> io::Result<()> {
        debug!("Updating guest memory mapping");
        *self.mem.write().unwrap() = mem;
        Ok(())
    }

    fn handle_event(
        &self,
        device_event: u16,
        evset: EventSet,
        vrings: &[Self::Vring],
        _thread_id: usize,
    ) -> io::Result<()> {
        trace!(device_event, ?evset, "Handling vring event");

        // Store vrings on first call for external RX injection
        {
            let mut stored = self.vrings.write().unwrap();
            if stored.is_none() {
                debug!("Storing vrings for external RX injection");
                *stored = Some(vrings.to_vec());
            }
        }

        if evset != EventSet::IN {
            return Ok(());
        }

        match device_event {
            RX_QUEUE => {
                // RX queue kick - process pending RX packets
                if self.process_rx(&vrings[RX_QUEUE as usize])? {
                    vrings[RX_QUEUE as usize]
                        .signal_used_queue()
                        .map_err(|e| io::Error::other(format!("Failed to signal: {e}")))?;
                }
            }
            TX_QUEUE => {
                // TX queue kick - process outgoing packets
                let tx_needs_signal = self.process_tx(&vrings[TX_QUEUE as usize])?;
                if tx_needs_signal {
                    vrings[TX_QUEUE as usize]
                        .signal_used_queue()
                        .map_err(|e| io::Error::other(format!("Failed to signal: {e}")))?;
                }

                // Always try to process any generated RX packets (e.g., ARP/DHCP responses)
                // This must happen even if TX doesn't need notification
                if self.process_rx(&vrings[RX_QUEUE as usize])? {
                    vrings[RX_QUEUE as usize]
                        .signal_used_queue()
                        .map_err(|e| io::Error::other(format!("Failed to signal: {e}")))?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn get_config(&self, offset: u32, size: u32) -> Vec<u8> {
        // Virtio net config: 6 bytes MAC + 2 bytes status
        let mac = self.mac_bytes();
        debug!(
            offset,
            size,
            mac = format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            ),
            "Returning device config"
        );
        let mut config = [0u8; 8];
        config[..6].copy_from_slice(&mac);
        config[6..8].copy_from_slice(&[1, 0]); // VIRTIO_NET_S_LINK_UP

        let start = offset as usize;
        let end = std::cmp::min(start + size as usize, config.len());
        if start < config.len() {
            config[start..end].to_vec()
        } else {
            vec![]
        }
    }

    fn exit_event(&self, _thread_index: usize) -> Option<(EventConsumer, EventNotifier)> {
        // Clone the exit event pair for this thread
        Some((
            self.exit_event
                .0
                .try_clone()
                .expect("Failed to clone EventConsumer"),
            self.exit_event
                .1
                .try_clone()
                .expect("Failed to clone EventNotifier"),
        ))
    }
}

/// Finalize checksum for a packet with VIRTIO_NET_HDR_F_NEEDS_CSUM set
///
/// The guest has computed a partial checksum (pseudo-header) and stored it
/// at csum_start + csum_offset. We need to:
/// 1. Read the partial checksum
/// 2. Compute the one's complement sum over the data from csum_start to end
/// 3. Fold and finalize the checksum
/// 4. Write the result back
///
/// # Arguments
/// * `packet` - The full packet (Ethernet frame) as mutable slice
/// * `csum_start` - Offset from start of Ethernet frame where checksum data begins
/// * `csum_offset` - Offset from csum_start where the checksum field is located
fn finalize_checksum(packet: &mut [u8], csum_start: u16, csum_offset: u16) {
    let start = csum_start as usize;
    let offset = csum_offset as usize;

    // Validate offsets
    if start + offset + 2 > packet.len() {
        return;
    }

    // Compute one's complement sum over the checksummed region
    let data = &packet[start..];
    let mut sum: u32 = 0;

    // Sum 16-bit words
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }

    // Handle odd byte
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // Fold 32-bit sum to 16 bits
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // One's complement
    let checksum = !(sum as u16);

    // Write the final checksum
    let csum_pos = start + offset;
    packet[csum_pos..csum_pos + 2].copy_from_slice(&checksum.to_be_bytes());
}

/// Parse MAC address string to bytes
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mac() {
        assert_eq!(
            parse_mac("52:54:00:12:34:56"),
            Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
        );
        assert_eq!(parse_mac("invalid"), None);
        assert_eq!(parse_mac("52:54:00:12:34"), None);
        assert_eq!(parse_mac("52:54:00:12:34:ZZ"), None);
    }

    #[test]
    fn test_virtio_net_hdr_size() {
        assert_eq!(std::mem::size_of::<VirtioNetHdr>(), VIRTIO_NET_HDR_SIZE);
    }
}
