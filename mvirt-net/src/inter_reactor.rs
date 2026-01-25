//! Inter-reactor communication types for multi-threaded packet forwarding.
//!
//! This module defines the message types used to pass packets between reactors
//! (TUN and vhost) without copying packet data. Instead, references (iovecs/HVAs)
//! are passed via mpsc channels, with eventfd for wakeup signaling.

use nix::libc;
use std::any::Any;
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Maximum iovecs per packet.
/// Descriptor chains rarely exceed 4 segments; 8 provides headroom.
/// Using a fixed-size array avoids heap allocation in the hot path.
pub const MAX_PACKET_IOVECS: usize = 8;

/// Unique identifier for a reactor instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReactorId(pub Uuid);

impl ReactorId {
    /// Create a new random ReactorId.
    pub fn new() -> Self {
        ReactorId(Uuid::new_v4())
    }

    /// Create a ReactorId from an existing UUID.
    pub fn from_uuid(id: Uuid) -> Self {
        ReactorId(id)
    }

    /// Get the underlying UUID.
    pub fn uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for ReactorId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ReactorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Reactor({})", self.0)
    }
}

/// Unique identifier for a packet in flight.
///
/// Used to correlate completion notifications with their source packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PacketId(pub u64);

impl PacketId {
    /// Create a new PacketId from a raw value.
    pub fn new(id: u64) -> Self {
        PacketId(id)
    }

    /// Get the raw value.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for PacketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pkt({})", self.0)
    }
}

/// Source of a packet being forwarded between reactors.
///
/// This tracks where the packet came from so the destination reactor
/// can send completion notifications back to the source.
#[derive(Debug, Clone)]
pub enum PacketSource {
    /// Packet from vhost TX queue (guest -> network).
    VhostTx {
        /// Descriptor chain head index for returning to guest.
        head_index: u16,
        /// Total length of the packet data.
        total_len: u32,
        /// Reactor that owns this packet.
        source_reactor: ReactorId,
    },
    /// Packet from TUN RX (network -> guest).
    TunRx {
        /// Chain ID for returning buffer to pool.
        chain_id: u64,
        /// Length of received data.
        len: u32,
        /// Reactor that owns this packet.
        source_reactor: ReactorId,
        /// Destination MAC address for Ethernet header injection.
        dst_mac: [u8; 6],
        /// EtherType for Ethernet header injection (0x0800 = IPv4, 0x86DD = IPv6).
        ethertype: u16,
    },
    /// Packet from vhost TX destined for another vhost (VM-to-VM).
    VhostToVhost {
        /// Descriptor chain head index for returning to guest.
        head_index: u16,
        /// Total length of the packet data.
        total_len: u32,
        /// Reactor that owns this packet.
        source_reactor: ReactorId,
        /// Destination MAC address (target VM's MAC).
        dst_mac: [u8; 6],
        /// Source MAC address (router's MAC).
        src_mac: [u8; 6],
    },
}

impl PacketSource {
    /// Get the source reactor ID.
    pub fn source_reactor(&self) -> ReactorId {
        match self {
            PacketSource::VhostTx { source_reactor, .. } => *source_reactor,
            PacketSource::TunRx { source_reactor, .. } => *source_reactor,
            PacketSource::VhostToVhost { source_reactor, .. } => *source_reactor,
        }
    }
}

/// A packet reference for zero-copy forwarding between reactors.
///
/// Contains iovecs pointing to packet data in guest memory (vhost) or
/// registered buffers (TUN). The receiving reactor performs I/O using
/// these iovecs and sends a CompletionNotify back.
///
/// Uses a fixed-size array to avoid heap allocation in the hot path.
#[derive(Clone)]
pub struct PacketRef {
    /// Unique identifier for this packet.
    pub id: PacketId,
    /// Scatter-gather list pointing to packet data (fixed-size to avoid heap allocation).
    /// For vhost: HVAs from guest memory mapping.
    /// For TUN: pointers to registered buffer pool.
    iovecs: [libc::iovec; MAX_PACKET_IOVECS],
    /// Number of valid iovecs in the array.
    iovecs_len: usize,
    /// Source information for completion handling.
    pub source: PacketSource,
    /// Keep-alive reference to prevent underlying memory from being unmapped.
    /// Holds a reference to guest memory mapping while packets are in flight.
    pub keep_alive: Option<Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for PacketRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PacketRef")
            .field("id", &self.id)
            .field("iovecs_len", &self.iovecs_len)
            .field("source", &self.source)
            .field("keep_alive", &self.keep_alive.is_some())
            .finish()
    }
}

impl PacketRef {
    /// Create a new PacketRef with a fixed-size iovec array.
    pub fn new(
        id: PacketId,
        iovecs: [libc::iovec; MAX_PACKET_IOVECS],
        iovecs_len: usize,
        source: PacketSource,
        keep_alive: Option<Arc<dyn Any + Send + Sync>>,
    ) -> Self {
        debug_assert!(iovecs_len <= MAX_PACKET_IOVECS);
        PacketRef {
            id,
            iovecs,
            iovecs_len,
            source,
            keep_alive,
        }
    }

    /// Get the valid iovecs as a slice.
    #[inline]
    pub fn iovecs(&self) -> &[libc::iovec] {
        &self.iovecs[..self.iovecs_len]
    }

    /// Get the number of valid iovecs.
    #[inline]
    pub fn iovecs_len(&self) -> usize {
        self.iovecs_len
    }

    /// Get the total length of all iovecs.
    #[inline]
    pub fn total_len(&self) -> usize {
        self.iovecs().iter().map(|iov| iov.iov_len).sum()
    }
}

// SAFETY: PacketRef contains raw pointers in iovecs, but these point to
// memory that remains valid for the lifetime of the packet (guest memory
// or registered buffers). The source reactor waits for completion before
// releasing the underlying memory.
unsafe impl Send for PacketRef {}

/// Completion notification sent back to the source reactor.
///
/// After the destination reactor completes I/O on a forwarded packet,
/// it sends this notification so the source can return descriptors
/// to the guest or buffer pool.
#[derive(Debug, Clone)]
pub enum CompletionNotify {
    /// Vhost TX packet was written to TUN.
    VhostTxComplete {
        /// The packet that was completed.
        packet_id: PacketId,
        /// Descriptor head index to return.
        head_index: u16,
        /// Total length for used ring.
        total_len: u32,
        /// I/O result (bytes written or negative errno).
        result: i32,
    },
    /// TUN RX packet was written to vhost RX queue.
    TunRxComplete {
        /// The packet that was completed.
        packet_id: PacketId,
        /// Chain ID to return to buffer pool.
        chain_id: u64,
        /// I/O result (bytes written or negative errno).
        result: i32,
    },
    /// Vhost TX packet was delivered to another vhost (VM-to-VM).
    VhostToVhostComplete {
        /// The packet that was completed.
        packet_id: PacketId,
        /// Descriptor head index to return.
        head_index: u16,
        /// Total length for used ring.
        total_len: u32,
        /// I/O result (bytes copied or negative errno).
        result: i32,
    },
}

impl CompletionNotify {
    /// Get the packet ID this completion is for.
    pub fn packet_id(&self) -> PacketId {
        match self {
            CompletionNotify::VhostTxComplete { packet_id, .. } => *packet_id,
            CompletionNotify::TunRxComplete { packet_id, .. } => *packet_id,
            CompletionNotify::VhostToVhostComplete { packet_id, .. } => *packet_id,
        }
    }

    /// Get the I/O result.
    pub fn result(&self) -> i32 {
        match self {
            CompletionNotify::VhostTxComplete { result, .. } => *result,
            CompletionNotify::TunRxComplete { result, .. } => *result,
            CompletionNotify::VhostToVhostComplete { result, .. } => *result,
        }
    }

    /// Check if the I/O completed successfully.
    pub fn is_success(&self) -> bool {
        self.result() >= 0
    }
}

/// Unified message type for inter-reactor communication.
///
/// Combines packet forwarding and completion notifications into a single
/// type, allowing use of a single channel (and eventfd) per reactor.
#[derive(Debug, Clone)]
pub enum ReactorMessage {
    /// Incoming packet to be processed (forwarded from another reactor).
    Packet(PacketRef),
    /// Completion notification for a previously sent packet.
    Completion(CompletionNotify),
}

impl ReactorMessage {
    /// Create a packet message.
    pub fn packet(packet: PacketRef) -> Self {
        ReactorMessage::Packet(packet)
    }

    /// Create a completion message.
    pub fn completion(completion: CompletionNotify) -> Self {
        ReactorMessage::Completion(completion)
    }

    /// Check if this is a packet message.
    pub fn is_packet(&self) -> bool {
        matches!(self, ReactorMessage::Packet(_))
    }

    /// Check if this is a completion message.
    pub fn is_completion(&self) -> bool {
        matches!(self, ReactorMessage::Completion(_))
    }

    /// Extract the packet if this is a packet message.
    pub fn into_packet(self) -> Option<PacketRef> {
        match self {
            ReactorMessage::Packet(p) => Some(p),
            _ => None,
        }
    }

    /// Extract the completion if this is a completion message.
    pub fn into_completion(self) -> Option<CompletionNotify> {
        match self {
            ReactorMessage::Completion(c) => Some(c),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reactor_id() {
        let id1 = ReactorId::new();
        let id2 = ReactorId::new();
        assert_ne!(id1, id2);

        let uuid = Uuid::new_v4();
        let id3 = ReactorId::from_uuid(uuid);
        assert_eq!(id3.uuid(), uuid);
    }

    #[test]
    fn test_packet_id() {
        let id = PacketId::new(42);
        assert_eq!(id.raw(), 42);
    }

    #[test]
    fn test_packet_source() {
        let reactor_id = ReactorId::new();
        let source = PacketSource::VhostTx {
            head_index: 5,
            total_len: 1500,
            source_reactor: reactor_id,
        };
        assert_eq!(source.source_reactor(), reactor_id);
    }

    #[test]
    fn test_packet_ref_total_len() {
        let mut iovecs = [libc::iovec {
            iov_base: std::ptr::null_mut(),
            iov_len: 0,
        }; MAX_PACKET_IOVECS];
        iovecs[0] = libc::iovec {
            iov_base: std::ptr::null_mut(),
            iov_len: 100,
        };
        iovecs[1] = libc::iovec {
            iov_base: std::ptr::null_mut(),
            iov_len: 200,
        };
        let packet = PacketRef::new(
            PacketId::new(1),
            iovecs,
            2,
            PacketSource::TunRx {
                chain_id: 0,
                len: 300,
                source_reactor: ReactorId::new(),
                dst_mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
                ethertype: 0x0800,
            },
            None,
        );
        assert_eq!(packet.total_len(), 300);
        assert_eq!(packet.iovecs_len(), 2);
    }

    #[test]
    fn test_completion_notify() {
        let completion = CompletionNotify::VhostTxComplete {
            packet_id: PacketId::new(42),
            head_index: 5,
            total_len: 1500,
            result: 1500,
        };
        assert_eq!(completion.packet_id(), PacketId::new(42));
        assert!(completion.is_success());

        let error_completion = CompletionNotify::TunRxComplete {
            packet_id: PacketId::new(43),
            chain_id: 10,
            result: -11, // EAGAIN
        };
        assert!(!error_completion.is_success());
    }
}
