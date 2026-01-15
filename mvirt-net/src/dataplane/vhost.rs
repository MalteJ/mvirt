//! vhost-user backend implementation for virtio-net
//!
//! This module implements the VhostUserBackend trait for handling
//! virtio-net devices over the vhost-user protocol.

use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use vhost::vhost_user::message::VhostUserProtocolFeatures;
use vhost_user_backend::{VhostUserBackend, VringRwLock, VringT};
use virtio_queue::QueueT;
use vm_memory::{
    Address, ByteValued, Bytes, GuestAddressSpace, GuestMemoryAtomic, GuestMemoryMmap, Le16,
};
use vmm_sys_util::epoll::EventSet;
use vmm_sys_util::eventfd::EventFd;

use crate::config::NicEntry;

/// Virtio net header size (without mergeable rx buffers)
const VIRTIO_NET_HDR_SIZE: usize = 12;

/// Queue indices
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

/// Virtio net features we support
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;
const VIRTIO_F_RING_INDIRECT_DESC: u64 = 1 << 28;
const VHOST_USER_F_PROTOCOL_FEATURES: u64 = 1 << 30;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

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
}

impl VhostNetBackend {
    /// Create a new vhost-user net backend
    pub fn new(nic_config: NicEntry, shutdown: Arc<AtomicBool>) -> io::Result<Self> {
        eprintln!(
            "[VHOST] VhostNetBackend::new() called for NIC {}",
            nic_config.id
        );
        Ok(Self {
            nic_config,
            mem: RwLock::new(GuestMemoryAtomic::new(GuestMemoryMmap::new())),
            event_idx: RwLock::new(false),
            shutdown,
            packet_handler: Mutex::new(None),
            rx_queue: Mutex::new(Vec::new()),
        })
    }

    /// Set the packet handler
    pub fn set_packet_handler(&self, handler: PacketHandler) {
        let mut ph = self.packet_handler.lock().unwrap();
        *ph = Some(handler);
    }

    /// Inject a packet into the guest's RX queue
    pub fn inject_packet(&self, packet: Vec<u8>) {
        let mut rx = self.rx_queue.lock().unwrap();
        rx.push(packet);
    }

    /// Get the MAC address as bytes
    fn mac_bytes(&self) -> [u8; 6] {
        parse_mac(&self.nic_config.mac_address).unwrap_or([0x52, 0x54, 0x00, 0x00, 0x00, 0x00])
    }

    /// Process TX queue (packets from guest)
    fn process_tx(&self, vring: &VringRwLock) -> io::Result<bool> {
        let mut used_descs = false;
        let mem_guard = self.mem.read().unwrap();
        let mem = mem_guard.memory();

        loop {
            let mut vring_state = vring.get_mut();

            let avail_desc = match vring_state
                .get_queue_mut()
                .pop_descriptor_chain(mem.clone())
            {
                Some(desc) => desc,
                None => break,
            };

            // Collect packet data from descriptor chain
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

            // Skip virtio-net header and process packet
            if packet.len() > VIRTIO_NET_HDR_SIZE {
                let eth_frame = &packet[VIRTIO_NET_HDR_SIZE..];
                let handler_guard = self.packet_handler.lock().unwrap();
                if let Some(ref handler) = *handler_guard
                    && let Some(response) = handler(eth_frame)
                {
                    self.inject_packet(response);
                }
            }

            // Mark descriptor as used
            let desc_idx = avail_desc.head_index();
            vring_state
                .get_queue_mut()
                .add_used(&*mem, desc_idx, 0)
                .map_err(|e| io::Error::other(format!("Failed to add used: {e}")))?;

            used_descs = true;
        }

        Ok(used_descs)
    }

    /// Process RX queue (inject packets to guest)
    fn process_rx(&self, vring: &VringRwLock) -> io::Result<bool> {
        let mut rx_queue = self.rx_queue.lock().unwrap();
        if rx_queue.is_empty() {
            return Ok(false);
        }

        let mut used_descs = false;
        let mem_guard = self.mem.read().unwrap();
        let mem = mem_guard.memory();

        while !rx_queue.is_empty() {
            let mut vring_state = vring.get_mut();

            let avail_desc = match vring_state
                .get_queue_mut()
                .pop_descriptor_chain(mem.clone())
            {
                Some(desc) => desc,
                None => break, // No available descriptors
            };

            let packet = rx_queue.remove(0);

            // Build virtio-net header + packet
            let hdr = VirtioNetHdr::default();
            let hdr_bytes = hdr.as_slice();
            let total_len = hdr_bytes.len() + packet.len();

            // Find a writable descriptor
            let mut written = 0;
            for desc in avail_desc.clone() {
                if desc.is_write_only() && written < total_len {
                    let to_write = std::cmp::min(desc.len() as usize, total_len - written);

                    if written < hdr_bytes.len() {
                        // Write header
                        let hdr_end = std::cmp::min(hdr_bytes.len() - written, to_write);
                        mem.write(&hdr_bytes[written..written + hdr_end], desc.addr())
                            .map_err(|e| io::Error::other(format!("Failed to write hdr: {e}")))?;

                        // Write packet data if space remains
                        if hdr_end < to_write {
                            let pkt_start = 0;
                            let pkt_end = to_write - hdr_end;
                            mem.write(
                                &packet[pkt_start..pkt_end],
                                desc.addr().unchecked_add(hdr_end as u64),
                            )
                            .map_err(|e| io::Error::other(format!("Failed to write pkt: {e}")))?;
                        }
                    } else {
                        // Write only packet data
                        let pkt_offset = written - hdr_bytes.len();
                        mem.write(&packet[pkt_offset..pkt_offset + to_write], desc.addr())
                            .map_err(|e| io::Error::other(format!("Failed to write pkt: {e}")))?;
                    }

                    written += to_write;
                }
            }

            let desc_idx = avail_desc.head_index();
            vring_state
                .get_queue_mut()
                .add_used(&*mem, desc_idx, written as u32)
                .map_err(|e| io::Error::other(format!("Failed to add used: {e}")))?;

            used_descs = true;
        }

        Ok(used_descs)
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
            | VIRTIO_NET_F_MAC
            | VIRTIO_NET_F_STATUS
            | VIRTIO_RING_F_EVENT_IDX
            | VIRTIO_F_RING_INDIRECT_DESC
            | VHOST_USER_F_PROTOCOL_FEATURES;
        eprintln!("[VHOST] features() called, returning {:#x}", f);
        tracing::debug!(features = f, "features() called");
        f
    }

    fn protocol_features(&self) -> VhostUserProtocolFeatures {
        let pf = VhostUserProtocolFeatures::CONFIG
            | VhostUserProtocolFeatures::MQ
            | VhostUserProtocolFeatures::REPLY_ACK;
        eprintln!("[VHOST] protocol_features() called, returning {:?}", pf);
        tracing::debug!(?pf, "protocol_features() called");
        pf
    }

    fn set_event_idx(&self, enabled: bool) {
        eprintln!("[VHOST] set_event_idx({})", enabled);
        *self.event_idx.write().unwrap() = enabled;
    }

    fn update_memory(&self, mem: GuestMemoryAtomic<GuestMemoryMmap>) -> io::Result<()> {
        eprintln!("[VHOST] update_memory() called");
        tracing::debug!("update_memory called");
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
        eprintln!(
            "[VHOST] handle_event called: device_event={}, evset={:?}",
            device_event, evset
        );
        tracing::debug!(device_event, ?evset, "handle_event called");
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
                if self.process_tx(&vrings[TX_QUEUE as usize])? {
                    vrings[TX_QUEUE as usize]
                        .signal_used_queue()
                        .map_err(|e| io::Error::other(format!("Failed to signal: {e}")))?;

                    // Also try to process any generated RX packets
                    if self.process_rx(&vrings[RX_QUEUE as usize])? {
                        vrings[RX_QUEUE as usize]
                            .signal_used_queue()
                            .map_err(|e| io::Error::other(format!("Failed to signal: {e}")))?;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn get_config(&self, offset: u32, size: u32) -> Vec<u8> {
        // Virtio net config: 6 bytes MAC + 2 bytes status
        let mac = self.mac_bytes();
        eprintln!(
            "[VHOST] get_config(offset={}, size={}) mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            offset, size, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
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

    fn exit_event(&self, _thread_index: usize) -> Option<EventFd> {
        None
    }
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
