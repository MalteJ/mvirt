//! Reactor registry for managing multiple reactor instances.
//!
//! The registry provides a central lookup for reactor information,
//! enabling cross-reactor communication via channels and eventfd signaling.

use crate::inter_reactor::{CompletionNotify, PacketRef, ReactorId};
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::RwLock;
use std::sync::mpsc::Sender;
use uuid::Uuid;

/// Type of interface a reactor handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceType {
    /// TUN interface reactor.
    Tun {
        /// Interface index from the kernel.
        if_index: u32,
    },
    /// Vhost-user device reactor.
    Vhost {
        /// UUID identifying the vhost device.
        device_id: Uuid,
    },
}

impl InterfaceType {
    /// Get the TUN interface index if this is a TUN interface.
    pub fn tun_if_index(&self) -> Option<u32> {
        match self {
            InterfaceType::Tun { if_index } => Some(*if_index),
            InterfaceType::Vhost { .. } => None,
        }
    }

    /// Get the vhost device ID if this is a vhost interface.
    pub fn vhost_device_id(&self) -> Option<Uuid> {
        match self {
            InterfaceType::Tun { .. } => None,
            InterfaceType::Vhost { device_id } => Some(*device_id),
        }
    }
}

/// Information about a registered reactor.
#[derive(Debug)]
pub struct ReactorInfo {
    /// Unique reactor identifier.
    pub id: ReactorId,
    /// Eventfd for waking the reactor.
    pub eventfd: RawFd,
    /// Channel for sending packets to this reactor.
    pub packet_tx: Sender<PacketRef>,
    /// Channel for sending completion notifications to this reactor.
    pub completion_tx: Sender<CompletionNotify>,
    /// Type of interface this reactor handles.
    pub interface_type: InterfaceType,
    /// MAC address for vhost interfaces (used for Ethernet header construction).
    pub mac_address: Option<[u8; 6]>,
}

impl ReactorInfo {
    /// Create new reactor info.
    pub fn new(
        id: ReactorId,
        eventfd: RawFd,
        packet_tx: Sender<PacketRef>,
        completion_tx: Sender<CompletionNotify>,
        interface_type: InterfaceType,
    ) -> Self {
        ReactorInfo {
            id,
            eventfd,
            packet_tx,
            completion_tx,
            interface_type,
            mac_address: None,
        }
    }

    /// Create new reactor info with a MAC address.
    pub fn with_mac(
        id: ReactorId,
        eventfd: RawFd,
        packet_tx: Sender<PacketRef>,
        completion_tx: Sender<CompletionNotify>,
        interface_type: InterfaceType,
        mac_address: [u8; 6],
    ) -> Self {
        ReactorInfo {
            id,
            eventfd,
            packet_tx,
            completion_tx,
            interface_type,
            mac_address: Some(mac_address),
        }
    }

    /// Signal this reactor via eventfd.
    pub fn signal(&self) {
        let buf: u64 = 1;
        unsafe {
            nix::libc::write(
                self.eventfd,
                &buf as *const u64 as *const nix::libc::c_void,
                8,
            );
        }
    }

    /// Send a packet to this reactor and signal it.
    #[allow(clippy::result_large_err)] // PacketRef uses fixed-size array to avoid heap allocation in hot path
    pub fn send_packet(&self, packet: PacketRef) -> Result<(), PacketRef> {
        self.packet_tx.send(packet).map_err(|e| e.0)?;
        self.signal();
        Ok(())
    }

    /// Send a packet to this reactor without signaling (for batching).
    #[allow(clippy::result_large_err)] // PacketRef uses fixed-size array to avoid heap allocation in hot path
    pub fn send_packet_no_signal(&self, packet: PacketRef) -> Result<(), PacketRef> {
        self.packet_tx.send(packet).map_err(|e| e.0)
    }

    /// Send a completion notification to this reactor and signal it.
    pub fn send_completion(&self, completion: CompletionNotify) -> Result<(), CompletionNotify> {
        self.completion_tx.send(completion).map_err(|e| e.0)?;
        self.signal();
        Ok(())
    }
}

/// Central registry for all reactor instances.
///
/// Provides thread-safe lookup of reactors by ID, interface index, or device ID.
/// Used for routing packets between reactors.
pub struct ReactorRegistry {
    /// Reactors indexed by their unique ID.
    reactors: RwLock<HashMap<ReactorId, ReactorInfo>>,
    /// Index from TUN if_index to ReactorId for fast lookup.
    tun_index: RwLock<HashMap<u32, ReactorId>>,
    /// Index from vhost device UUID to ReactorId for fast lookup.
    vhost_index: RwLock<HashMap<Uuid, ReactorId>>,
}

impl ReactorRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        ReactorRegistry {
            reactors: RwLock::new(HashMap::new()),
            tun_index: RwLock::new(HashMap::new()),
            vhost_index: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new reactor.
    ///
    /// Returns the previous reactor info if one existed with the same ID.
    pub fn register(&self, info: ReactorInfo) -> Option<ReactorInfo> {
        let id = info.id;

        // Update secondary indexes
        match &info.interface_type {
            InterfaceType::Tun { if_index } => {
                self.tun_index.write().unwrap().insert(*if_index, id);
            }
            InterfaceType::Vhost { device_id } => {
                self.vhost_index.write().unwrap().insert(*device_id, id);
            }
        }

        self.reactors.write().unwrap().insert(id, info)
    }

    /// Unregister a reactor by ID.
    ///
    /// Returns the removed reactor info if it existed.
    pub fn unregister(&self, id: &ReactorId) -> Option<ReactorInfo> {
        let info = self.reactors.write().unwrap().remove(id)?;

        // Clean up secondary indexes
        match &info.interface_type {
            InterfaceType::Tun { if_index } => {
                self.tun_index.write().unwrap().remove(if_index);
            }
            InterfaceType::Vhost { device_id } => {
                self.vhost_index.write().unwrap().remove(device_id);
            }
        }

        Some(info)
    }

    /// Get reactor ID by TUN interface index.
    pub fn get_by_tun_index(&self, if_index: u32) -> Option<ReactorId> {
        self.tun_index.read().unwrap().get(&if_index).copied()
    }

    /// Get reactor ID by vhost device UUID.
    pub fn get_by_vhost_id(&self, device_id: &Uuid) -> Option<ReactorId> {
        self.vhost_index.read().unwrap().get(device_id).copied()
    }

    /// Send a packet to a reactor by ID.
    ///
    /// Returns true if the packet was sent, false if reactor not found.
    pub fn send_packet_to(&self, reactor_id: &ReactorId, packet: PacketRef) -> bool {
        let reactors = self.reactors.read().unwrap();
        if let Some(info) = reactors.get(reactor_id) {
            info.send_packet(packet).is_ok()
        } else {
            false
        }
    }

    /// Send a packet to a reactor by ID without signaling (for batching).
    ///
    /// Returns true if the packet was sent, false if reactor not found.
    /// Caller must call `signal_reactor` after sending all packets.
    pub fn send_packet_to_no_signal(&self, reactor_id: &ReactorId, packet: PacketRef) -> bool {
        let reactors = self.reactors.read().unwrap();
        if let Some(info) = reactors.get(reactor_id) {
            info.send_packet_no_signal(packet).is_ok()
        } else {
            false
        }
    }

    /// Send a packet to a TUN reactor by interface index.
    ///
    /// Returns true if the packet was sent, false if reactor not found.
    pub fn send_packet_to_tun(&self, if_index: u32, packet: PacketRef) -> bool {
        let reactor_id = match self.get_by_tun_index(if_index) {
            Some(id) => id,
            None => return false,
        };
        self.send_packet_to(&reactor_id, packet)
    }

    /// Send a packet to a vhost reactor by device UUID.
    ///
    /// Returns true if the packet was sent, false if reactor not found.
    pub fn send_packet_to_vhost(&self, device_id: &Uuid, packet: PacketRef) -> bool {
        let reactor_id = match self.get_by_vhost_id(device_id) {
            Some(id) => id,
            None => return false,
        };
        self.send_packet_to(&reactor_id, packet)
    }

    /// Send a completion notification to a reactor by ID.
    ///
    /// Returns true if the notification was sent, false if reactor not found.
    pub fn send_completion_to(&self, reactor_id: &ReactorId, completion: CompletionNotify) -> bool {
        let reactors = self.reactors.read().unwrap();
        if let Some(info) = reactors.get(reactor_id) {
            info.send_completion(completion).is_ok()
        } else {
            false
        }
    }

    /// Signal a reactor by ID (for use with batched packet sending).
    pub fn signal_reactor(&self, reactor_id: &ReactorId) {
        let reactors = self.reactors.read().unwrap();
        if let Some(info) = reactors.get(reactor_id) {
            info.signal();
        }
    }

    /// Get the MAC address for a reactor by ID.
    ///
    /// Returns the MAC address if the reactor exists and has one configured.
    /// Used for Ethernet header construction when forwarding from TUN to vhost.
    pub fn get_mac_for_reactor(&self, reactor_id: &ReactorId) -> Option<[u8; 6]> {
        let reactors = self.reactors.read().unwrap();
        reactors.get(reactor_id).and_then(|info| info.mac_address)
    }

    /// Get the number of registered reactors.
    pub fn len(&self) -> usize {
        self.reactors.read().unwrap().len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all registered reactor IDs.
    pub fn reactor_ids(&self) -> Vec<ReactorId> {
        self.reactors.read().unwrap().keys().copied().collect()
    }
}

impl Default for ReactorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn create_test_reactor_info(id: ReactorId, iface_type: InterfaceType) -> ReactorInfo {
        let (packet_tx, _packet_rx) = mpsc::channel();
        let (completion_tx, _completion_rx) = mpsc::channel();
        // Use a dummy fd for testing - don't actually use it
        let dummy_fd = -1;
        ReactorInfo::new(id, dummy_fd, packet_tx, completion_tx, iface_type)
    }

    #[test]
    fn test_registry_register_unregister() {
        let registry = ReactorRegistry::new();
        let id = ReactorId::new();

        let info = create_test_reactor_info(id, InterfaceType::Tun { if_index: 42 });

        assert!(registry.is_empty());

        // Register
        assert!(registry.register(info).is_none());
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        // Lookup by TUN index
        assert_eq!(registry.get_by_tun_index(42), Some(id));
        assert_eq!(registry.get_by_tun_index(99), None);

        // Unregister
        let removed = registry.unregister(&id);
        assert!(removed.is_some());
        assert!(registry.is_empty());
        assert_eq!(registry.get_by_tun_index(42), None);
    }

    #[test]
    fn test_registry_vhost_lookup() {
        let registry = ReactorRegistry::new();
        let id = ReactorId::new();
        let device_id = Uuid::new_v4();

        let info = create_test_reactor_info(id, InterfaceType::Vhost { device_id });

        registry.register(info);

        // Lookup by vhost device ID
        assert_eq!(registry.get_by_vhost_id(&device_id), Some(id));
        assert_eq!(registry.get_by_vhost_id(&Uuid::new_v4()), None);
    }

    #[test]
    fn test_registry_multiple_reactors() {
        let registry = ReactorRegistry::new();

        let id1 = ReactorId::new();
        let id2 = ReactorId::new();
        let device_id = Uuid::new_v4();

        let info1 = create_test_reactor_info(id1, InterfaceType::Tun { if_index: 10 });
        let info2 = create_test_reactor_info(id2, InterfaceType::Vhost { device_id });

        registry.register(info1);
        registry.register(info2);

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.get_by_tun_index(10), Some(id1));
        assert_eq!(registry.get_by_vhost_id(&device_id), Some(id2));

        let ids = registry.reactor_ids();
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn test_interface_type_accessors() {
        let tun = InterfaceType::Tun { if_index: 5 };
        assert_eq!(tun.tun_if_index(), Some(5));
        assert_eq!(tun.vhost_device_id(), None);

        let device_id = Uuid::new_v4();
        let vhost = InterfaceType::Vhost { device_id };
        assert_eq!(vhost.tun_if_index(), None);
        assert_eq!(vhost.vhost_device_id(), Some(device_id));
    }
}
