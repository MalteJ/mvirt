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

/// ioctl request code for TUNSETIFF
const TUNSETIFF: libc::c_ulong = 0x400454ca;

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
    /// Create the global TUN device "mvirt-net"
    ///
    /// This creates a Layer 3 TUN device (raw IP packets, no Ethernet header).
    /// The device is created with IFF_NO_PI, meaning no packet information
    /// header is prepended to packets.
    pub fn new() -> io::Result<Self> {
        // Open /dev/net/tun
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/net/tun")?;

        // Prepare ifreq struct
        let mut ifr = IfReq {
            ifr_name: [0; libc::IFNAMSIZ],
            ifr_flags: IFF_TUN | IFF_NO_PI,
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
