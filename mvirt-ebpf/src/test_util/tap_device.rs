//! TAP device wrapper for integration tests.
//!
//! Provides a simple interface for creating TAP devices and
//! sending/receiving raw Ethernet packets.

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::time::Duration;

/// Flags for TAP device creation
const IFF_TAP: libc::c_short = 0x0002;
const IFF_NO_PI: libc::c_short = 0x1000;

/// ioctl request for TUNSETIFF
const TUNSETIFF: libc::c_ulong = 0x400454ca;

/// TAP device for integration testing.
///
/// Provides a simple interface to create a TAP device and send/receive
/// raw Ethernet packets. Unlike vhost-user, packets are sent directly
/// without virtio-net headers.
pub struct TapTestDevice {
    name: String,
    fd: OwnedFd,
    if_index: u32,
}

#[repr(C)]
struct IfReq {
    ifr_name: [libc::c_char; libc::IFNAMSIZ],
    ifr_flags: libc::c_short,
    _pad: [u8; 22],
}

impl TapTestDevice {
    /// Create a new TAP device for testing.
    ///
    /// The device name should be unique per test to avoid conflicts.
    /// Requires CAP_NET_ADMIN capability.
    pub fn create(name: &str) -> io::Result<Self> {
        // Open /dev/net/tun
        let tun_path = CString::new("/dev/net/tun").unwrap();
        let fd = unsafe { libc::open(tun_path.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        // Prepare ifreq structure
        let mut ifr = IfReq {
            ifr_name: [0; libc::IFNAMSIZ],
            ifr_flags: IFF_TAP | IFF_NO_PI,
            _pad: [0; 22],
        };

        // Copy device name
        let name_bytes = name.as_bytes();
        let copy_len = std::cmp::min(name_bytes.len(), libc::IFNAMSIZ - 1);
        for (i, &byte) in name_bytes[..copy_len].iter().enumerate() {
            ifr.ifr_name[i] = byte as libc::c_char;
        }

        // Create the TAP device
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), TUNSETIFF as libc::Ioctl, &ifr) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // Get interface index
        let if_index = Self::get_if_index(name)?;

        // Bring the interface up
        Self::set_interface_up(name)?;

        Ok(Self {
            name: name.to_string(),
            fd,
            if_index,
        })
    }

    /// Get the interface index for a given interface name
    fn get_if_index(name: &str) -> io::Result<u32> {
        let name_cstr = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name"))?;
        let index = unsafe { libc::if_nametoindex(name_cstr.as_ptr()) };
        if index == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(index)
    }

    /// Bring the interface up using ioctl
    fn set_interface_up(name: &str) -> io::Result<()> {
        let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if sock < 0 {
            return Err(io::Error::last_os_error());
        }
        let sock = unsafe { OwnedFd::from_raw_fd(sock) };

        #[repr(C)]
        struct IfReqFlags {
            ifr_name: [libc::c_char; libc::IFNAMSIZ],
            ifr_flags: libc::c_short,
            _pad: [u8; 22],
        }

        let mut ifr = IfReqFlags {
            ifr_name: [0; libc::IFNAMSIZ],
            ifr_flags: 0,
            _pad: [0; 22],
        };

        let name_bytes = name.as_bytes();
        let copy_len = std::cmp::min(name_bytes.len(), libc::IFNAMSIZ - 1);
        for (i, &byte) in name_bytes[..copy_len].iter().enumerate() {
            ifr.ifr_name[i] = byte as libc::c_char;
        }

        // Get current flags
        let ret = unsafe {
            libc::ioctl(
                sock.as_raw_fd(),
                libc::SIOCGIFFLAGS as libc::Ioctl,
                &mut ifr,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // Set IFF_UP flag
        ifr.ifr_flags |= libc::IFF_UP as libc::c_short;

        // Apply flags
        let ret = unsafe { libc::ioctl(sock.as_raw_fd(), libc::SIOCSIFFLAGS as libc::Ioctl, &ifr) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Get the interface name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the interface index
    pub fn if_index(&self) -> u32 {
        self.if_index
    }

    /// Get the raw file descriptor
    pub fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    /// Duplicate the file descriptor for use by another component (e.g., ProtocolHandler).
    ///
    /// The returned fd is independent and can be passed to other code that needs
    /// to read/write from the TAP device.
    pub fn dup_fd(&self) -> std::io::Result<OwnedFd> {
        let new_fd = unsafe { libc::dup(self.fd.as_raw_fd()) };
        if new_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(new_fd) })
    }

    /// Send a raw Ethernet packet.
    ///
    /// The packet should start with the Ethernet header (no virtio-net header).
    pub fn send_packet(&self, data: &[u8]) -> io::Result<usize> {
        let mut file = unsafe { std::fs::File::from_raw_fd(self.fd.as_raw_fd()) };
        let result = file.write(data);
        // Don't drop the file - it would close the fd
        std::mem::forget(file);
        result
    }

    /// Receive a packet with timeout.
    ///
    /// Returns None if no packet is available within the timeout.
    /// The returned packet starts with the Ethernet header (no virtio-net header).
    pub fn recv_packet(&self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        if !self.wait_readable(timeout)? {
            return Ok(None);
        }

        let mut buf = vec![0u8; 65535];
        let mut file = unsafe { std::fs::File::from_raw_fd(self.fd.as_raw_fd()) };
        let result = file.read(&mut buf);
        std::mem::forget(file);

        match result {
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Wait for packet availability with timeout.
    ///
    /// Returns true if data is available, false if timeout occurred.
    pub fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        let borrowed_fd: BorrowedFd = self.fd.as_fd();
        let mut poll_fds = [PollFd::new(borrowed_fd, PollFlags::POLLIN)];
        let timeout_ms = timeout.as_millis() as i32;
        let poll_timeout = if timeout_ms == 0 {
            PollTimeout::ZERO
        } else {
            PollTimeout::try_from(timeout_ms).unwrap_or(PollTimeout::MAX)
        };

        match poll(&mut poll_fds, poll_timeout) {
            Ok(n) => Ok(n > 0),
            Err(nix::errno::Errno::EINTR) => Ok(false),
            Err(e) => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }

    /// Drain all pending packets from the device.
    ///
    /// Useful for clearing any stale packets before starting a test.
    pub fn drain(&self) -> io::Result<usize> {
        let mut count = 0;
        while self.recv_packet(Duration::from_millis(10))?.is_some() {
            count += 1;
        }
        Ok(count)
    }
}

impl Drop for TapTestDevice {
    fn drop(&mut self) {
        // TAP device is automatically destroyed when fd is closed
        tracing::debug!(name = %self.name, "TAP test device dropped");
    }
}
