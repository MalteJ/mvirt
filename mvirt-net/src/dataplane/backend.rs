//! Reactor backend trait for pluggable I/O
//!
//! This module defines the `ReactorBackend` trait that abstracts over
//! different packet I/O mechanisms (vhost-user, TUN device).

use std::io::{self, IoSlice};
use std::os::fd::{BorrowedFd, RawFd};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::uio::writev;
use vm_memory::ByteValued;

use super::buffer::PoolBuffer;
use super::tun::TunDevice;
use super::vhost::VirtioNetHdr;

/// Result of a receive operation
pub enum RecvResult {
    /// Successfully received a packet with the given length
    Packet(usize),
    /// No packet available (would block)
    WouldBlock,
    /// Backend is done (e.g., connection closed)
    Done,
}

/// Abstraction for different I/O backends
///
/// Each backend handles packet I/O for one endpoint:
/// - `VhostBackend`: vhost-user socket for VM vNICs
/// - `TunBackend`: TUN device for internet gateway
pub trait ReactorBackend: Send {
    /// Try to receive a packet (non-blocking)
    ///
    /// Reads into the buffer's write area and returns the number of bytes read.
    /// Returns `WouldBlock` if no packet is available.
    fn try_recv(&mut self, buf: &mut PoolBuffer) -> io::Result<RecvResult>;

    /// Send a packet (non-blocking)
    ///
    /// Takes ownership of the buffer for zero-copy delivery.
    /// The virtio header carries GSO/checksum offload info.
    fn send(&mut self, buf: PoolBuffer, virtio_hdr: VirtioNetHdr) -> io::Result<()>;

    /// File descriptor for polling (if available)
    ///
    /// Returns the fd that should be polled for read readiness.
    fn poll_fd(&self) -> Option<RawFd>;

    /// Process any pending completions or housekeeping
    ///
    /// Called each iteration of the event loop. Used by vhost-user
    /// to process TX completions and return buffers to the guest.
    fn process_completions(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Check if the backend is still connected/valid
    fn is_connected(&self) -> bool {
        true
    }
}

// ============================================================================
// TUN Backend Implementation
// ============================================================================

/// TUN device backend for internet gateway
///
/// Handles raw IP packets (no Ethernet header) with virtio_net_hdr prepended.
/// Used by the TUN reactor to bridge VM traffic to/from the internet.
pub struct TunBackend {
    tun: TunDevice,
    fd: RawFd,
}

impl TunBackend {
    /// Create a new TUN backend from an existing TunDevice
    ///
    /// Sets the TUN device to non-blocking mode.
    pub fn new(tun: TunDevice) -> io::Result<Self> {
        let fd = tun.as_raw_fd();

        // Set non-blocking mode using BorrowedFd
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        let flags = fcntl(borrowed_fd, FcntlArg::F_GETFL).map_err(io::Error::from)?;
        let new_flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
        fcntl(borrowed_fd, FcntlArg::F_SETFL(new_flags)).map_err(io::Error::from)?;

        Ok(Self { tun, fd })
    }

    /// Get reference to the underlying TUN device
    pub fn tun(&self) -> &TunDevice {
        &self.tun
    }
}

impl ReactorBackend for TunBackend {
    fn try_recv(&mut self, buf: &mut PoolBuffer) -> io::Result<RecvResult> {
        match self.tun.read_packet(buf.write_area()) {
            Ok(0) => Ok(RecvResult::Done),
            Ok(n) => Ok(RecvResult::Packet(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(RecvResult::WouldBlock),
            Err(e) => Err(e),
        }
    }

    fn send(&mut self, buf: PoolBuffer, _virtio_hdr: VirtioNetHdr) -> io::Result<()> {
        // TUN expects virtio_net_hdr + IP packet
        // The buffer contains Ethernet frame, so we need to:
        // 1. Create a virtio_net_hdr (all zeros = no offload)
        // 2. Skip Ethernet header and send IP payload

        let data = buf.data();
        if data.len() < 14 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Packet too small",
            ));
        }

        // Create zero virtio header (no GSO, no checksum offload)
        // We ignore the incoming virtio_hdr as TUN handles checksums itself
        let hdr = VirtioNetHdr::default();
        let hdr_bytes = hdr.as_slice();

        // Skip Ethernet header (14 bytes) to get IP packet
        let ip_packet = &data[14..];

        // Use scatter-gather I/O: virtio header + IP payload
        let iov = [IoSlice::new(hdr_bytes), IoSlice::new(ip_packet)];
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        writev(fd, &iov).map_err(io::Error::from)?;

        Ok(())
        // buf dropped here, returned to pool
    }

    fn poll_fd(&self) -> Option<RawFd> {
        Some(self.fd)
    }
}

// ============================================================================
// Vhost Backend Implementation
// ============================================================================

use crossbeam_channel::TryRecvError;

/// Sender for forwarding packets from VhostNetBackend to VhostBackend
pub type VhostPacketSender = crossbeam_channel::Sender<(PoolBuffer, VirtioNetHdr)>;

/// vhost-user backend wrapper for VM vNICs
///
/// This wraps `VhostNetBackend` to implement `ReactorBackend`.
/// Packets from the guest arrive via an internal channel (populated by packet_handler),
/// packets to the guest are injected via `inject_buffer_and_deliver`.
pub struct VhostBackend {
    backend: std::sync::Arc<super::vhost::VhostNetBackend>,
    /// Receiver for packets from guest (via VhostNetBackend's packet_handler)
    rx: crossbeam_channel::Receiver<(PoolBuffer, VirtioNetHdr)>,
}

impl VhostBackend {
    /// Create a new vhost backend wrapper
    ///
    /// Returns the backend and a sender that should be used in the packet_handler
    /// to forward packets from the guest to the Reactor.
    pub fn new(
        backend: std::sync::Arc<super::vhost::VhostNetBackend>,
    ) -> (Self, VhostPacketSender) {
        let (tx, rx) = crossbeam_channel::bounded(1024);
        (Self { backend, rx }, tx)
    }

    /// Get reference to the underlying VhostNetBackend
    pub fn backend(&self) -> &std::sync::Arc<super::vhost::VhostNetBackend> {
        &self.backend
    }
}

impl ReactorBackend for VhostBackend {
    fn try_recv(&mut self, buf: &mut PoolBuffer) -> io::Result<RecvResult> {
        // Receive packet from the channel (populated by VhostNetBackend's packet_handler)
        match self.rx.try_recv() {
            Ok((packet, _virtio_hdr)) => {
                // Copy packet data to the provided buffer
                // Note: This is a copy, but it's fast (memcpy) and keeps the trait simple
                let data = packet.data();
                let write_area = buf.write_area();
                let len = data.len().min(write_area.len());
                write_area[..len].copy_from_slice(&data[..len]);
                Ok(RecvResult::Packet(len))
            }
            Err(TryRecvError::Empty) => Ok(RecvResult::WouldBlock),
            Err(TryRecvError::Disconnected) => Ok(RecvResult::Done),
        }
    }

    fn send(&mut self, buf: PoolBuffer, virtio_hdr: VirtioNetHdr) -> io::Result<()> {
        // Inject packet to guest's RX queue (zero-copy)
        self.backend.inject_buffer_and_deliver(buf, virtio_hdr);
        Ok(())
    }

    fn poll_fd(&self) -> Option<RawFd> {
        // No fd to poll - packets come via channel from VhostUserDaemon thread
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recv_result_variants() {
        // Just ensure the enum variants exist
        let _ = RecvResult::Packet(100);
        let _ = RecvResult::WouldBlock;
        let _ = RecvResult::Done;
    }
}
