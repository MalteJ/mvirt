//! TUN device management for public network internet access
//!
//! Creates and manages the global "mvirt-net" TUN device that provides
//! internet access for public networks via the Linux kernel.

use nix::libc;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};

/// Name of the global TUN device
pub const TUN_NAME: &str = "mvirt-net";

/// TUN device flags from linux/if_tun.h
const IFF_TUN: libc::c_short = 0x0001;
const IFF_NO_PI: libc::c_short = 0x1000;
const IFF_MULTI_QUEUE: libc::c_short = 0x0100; // Multi-queue TUN support
const IFF_VNET_HDR: libc::c_short = 0x4000; // Prepend virtio_net_hdr to packets

/// ioctl request code for TUNSETIFF
const TUNSETIFF: libc::c_ulong = 0x400454ca;

/// ioctl request code for TUNSETOFFLOAD (enable TSO/checksum offload)
const TUNSETOFFLOAD: libc::c_ulong = 0x400454d0;

/// ioctl request code for TUNSETVNETHDRSZ (set virtio header size)
const TUNSETVNETHDRSZ: libc::c_ulong = 0x400454d8;

/// Size of virtio_net_hdr with num_buffers field (12 bytes)
const VNET_HDR_SIZE: libc::c_int = 12;

/// TUN offload flags (from linux/if_tun.h)
const TUN_F_CSUM: libc::c_uint = 0x01; // Checksum offload
// TSO flags intentionally omitted - would cause packet truncation

/// ifreq structure for TUN device configuration
#[repr(C)]
struct IfReq {
    ifr_name: [libc::c_char; libc::IFNAMSIZ],
    ifr_flags: libc::c_short,
    _pad: [u8; 22], // padding to match kernel struct size
}

/// Global TUN device "mvirt-net" for routing packets to/from the internet
pub struct TunDevice {
    name: String,
    file: File,
}

impl TunDevice {
    /// Create a single-queue TUN device "mvirt-net"
    ///
    /// This creates a Layer 3 TUN device (raw IP packets, no Ethernet header).
    /// For multi-queue support, use `new_multiqueue()` instead.
    pub fn new() -> io::Result<Self> {
        Self::create_queue(false)
    }

    /// Create multiple TUN queues for parallel packet processing
    ///
    /// Each queue can be handled by a separate thread/reactor for better
    /// throughput. All queues share the same device name and IP configuration.
    ///
    /// # Arguments
    /// * `num_queues` - Number of queues to create (typically = number of CPUs)
    pub fn new_multiqueue(num_queues: usize) -> io::Result<Vec<Self>> {
        if num_queues == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "num_queues must be > 0",
            ));
        }

        let mut queues = Vec::with_capacity(num_queues);
        for _ in 0..num_queues {
            queues.push(Self::create_queue(true)?);
        }
        Ok(queues)
    }

    /// Internal: Create a TUN queue (single or multi-queue mode)
    fn create_queue(multiqueue: bool) -> io::Result<Self> {
        // Open /dev/net/tun
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/net/tun")?;

        // Prepare ifreq struct
        // IFF_VNET_HDR enables virtio_net_hdr for GSO/TSO support
        let mut flags = IFF_TUN | IFF_NO_PI | IFF_VNET_HDR;
        if multiqueue {
            flags |= IFF_MULTI_QUEUE;
        }

        let mut ifr = IfReq {
            ifr_name: [0; libc::IFNAMSIZ],
            ifr_flags: flags,
            _pad: [0; 22],
        };

        // Set device name
        let name_bytes = TUN_NAME.as_bytes();
        if name_bytes.len() >= libc::IFNAMSIZ {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "TUN device name too long",
            ));
        }
        for (i, &b) in name_bytes.iter().enumerate() {
            ifr.ifr_name[i] = b as libc::c_char;
        }

        // Create TUN device via ioctl
        let ret = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF as _, &ifr) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // Set virtio header size to 12 bytes (includes num_buffers field)
        // Default is 10 bytes, but we use the full 12-byte header
        let ret = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETVNETHDRSZ as _, &VNET_HDR_SIZE) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            name: TUN_NAME.to_string(),
            file,
        })
    }

    /// Get the device name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the raw file descriptor for polling
    pub fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Read a packet from the TUN device (blocks until data available)
    ///
    /// Returns raw IP packet (no Ethernet header)
    pub fn read_packet(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }

    /// Write a packet to the TUN device
    ///
    /// Expects raw IP packet (no Ethernet header)
    pub fn write_packet(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    /// Bring the TUN interface up
    ///
    /// This runs `ip link set mvirt-net up` via socket ioctl.
    /// Note: The TUN device gets no IP addresses - the host must configure
    /// routes and NAT separately.
    pub fn bring_up(&self) -> io::Result<()> {
        // Create a socket for ioctl
        let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if sock < 0 {
            return Err(io::Error::last_os_error());
        }

        // Ensure socket is closed on exit
        let sock_guard = SockGuard(sock);

        // Get current flags
        let mut ifr = IfReqFlags {
            ifr_name: [0; libc::IFNAMSIZ],
            ifr_flags: 0,
            _pad: [0; 22],
        };

        let name_bytes = self.name.as_bytes();
        for (i, &b) in name_bytes.iter().enumerate() {
            ifr.ifr_name[i] = b as libc::c_char;
        }

        // SIOCGIFFLAGS
        let ret = unsafe { libc::ioctl(sock_guard.0, libc::SIOCGIFFLAGS as _, &ifr) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // Set IFF_UP flag
        ifr.ifr_flags |= libc::IFF_UP as libc::c_short;

        // SIOCSIFFLAGS
        let ret = unsafe { libc::ioctl(sock_guard.0, libc::SIOCSIFFLAGS as _, &ifr) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Enable checksum offload on the TUN device
    ///
    /// Only checksum offload is enabled. TSO is disabled because process_rx
    /// cannot handle packets larger than a single descriptor chain (~MTU).
    /// With TSO disabled, the kernel segments packets to MTU size.
    pub fn enable_offload(&self) -> io::Result<()> {
        // Only CSUM, no TSO - prevents 64KB packets that would be truncated
        let offload = TUN_F_CSUM;

        // SAFETY: ioctl on valid fd with valid flags
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), TUNSETOFFLOAD as _, offload) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        tracing::info!("TUN device offload enabled (CSUM only, TSO disabled)");
        Ok(())
    }
}

/// ifreq structure for getting/setting interface flags
#[repr(C)]
struct IfReqFlags {
    ifr_name: [libc::c_char; libc::IFNAMSIZ],
    ifr_flags: libc::c_short,
    _pad: [u8; 22],
}

/// RAII guard for socket fd
struct SockGuard(RawFd);

impl Drop for SockGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Get all routes pointing to the TUN device
///
/// Returns a list of destination prefixes (CIDR notation)
pub fn get_routes() -> io::Result<Vec<String>> {
    use std::process::Command;

    let output = Command::new("ip")
        .args(["route", "show", "dev", TUN_NAME])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "ip route show failed: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut routes = Vec::new();

    for line in stdout.lines() {
        // Lines look like: "10.200.0.0/24 dev mvirt-net scope link"
        // We just need the first part (the destination)
        if let Some(dest) = line.split_whitespace().next() {
            routes.push(dest.to_string());
        }
    }

    // Also check IPv6 routes
    let output6 = Command::new("ip")
        .args(["-6", "route", "show", "dev", TUN_NAME])
        .output()?;

    if output6.status.success() {
        let stdout6 = String::from_utf8_lossy(&output6.stdout);
        for line in stdout6.lines() {
            if let Some(dest) = line.split_whitespace().next() {
                routes.push(dest.to_string());
            }
        }
    }

    Ok(routes)
}

/// Add a route for a subnet to the TUN device
///
/// This runs `ip route add <subnet> dev mvirt-net`
pub fn add_route(subnet: &str) -> io::Result<()> {
    use std::process::Command;

    let output = Command::new("ip")
        .args(["route", "add", subnet, "dev", TUN_NAME])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "File exists" error (route already exists)
        if !stderr.contains("File exists") {
            return Err(io::Error::other(format!(
                "ip route add failed: {}",
                stderr.trim()
            )));
        }
    }

    Ok(())
}

/// Remove a route for a subnet from the TUN device
///
/// This runs `ip route del <subnet> dev mvirt-net`
pub fn remove_route(subnet: &str) -> io::Result<()> {
    use std::process::Command;

    let output = Command::new("ip")
        .args(["route", "del", subnet, "dev", TUN_NAME])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "No such process" error (route doesn't exist)
        if !stderr.contains("No such process") {
            return Err(io::Error::other(format!(
                "ip route del failed: {}",
                stderr.trim()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tun_name_constant() {
        assert_eq!(TUN_NAME, "mvirt-net");
        assert!(TUN_NAME.len() < libc::IFNAMSIZ);
    }

    // Note: Actual TUN device creation requires CAP_NET_ADMIN
    // Integration tests should be run with appropriate privileges
}
