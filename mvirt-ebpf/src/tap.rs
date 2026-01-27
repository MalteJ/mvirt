//! TAP device management for eBPF-based networking.

use nix::fcntl::{OFlag, open};
use nix::sys::stat::Mode;
use nix::unistd::close;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use thiserror::Error;

/// TAP device errors.
#[derive(Debug, Error)]
pub enum TapError {
    #[error("Failed to open /dev/net/tun: {0}")]
    OpenTun(io::Error),

    #[error("Failed to create TAP device '{0}': {1}")]
    CreateDevice(String, io::Error),

    #[error("Failed to get interface index for '{0}': {1}")]
    GetIfIndex(String, io::Error),

    #[error("TAP name too long: {0} (max 15 chars)")]
    NameTooLong(String),

    #[error("Failed to set interface up: {0}")]
    SetUp(io::Error),

    #[error("Failed to delete interface '{0}': {1}")]
    DeleteInterface(String, io::Error),
}

pub type Result<T> = std::result::Result<T, TapError>;

// ioctl constants for TUN/TAP
const TUNSETIFF: libc::c_ulong = 0x400454ca;
const IFF_TAP: i16 = 0x0002;
const IFF_NO_PI: i16 = 0x1000;
const IFF_VNET_HDR: i16 = 0x4000;

#[repr(C)]
#[derive(Default)]
struct IfReq {
    ifr_name: [u8; 16],
    ifr_flags: i16,
    _padding: [u8; 22],
}

/// A managed TAP device.
pub struct TapDevice {
    /// TAP device name (e.g., "tap_abc123")
    pub name: String,
    /// File descriptor for the TAP device
    fd: OwnedFd,
    /// Interface index
    pub if_index: u32,
}

impl TapDevice {
    /// Create a new TAP device with the given name.
    ///
    /// The name must be at most 15 characters (IFNAMSIZ - 1).
    /// The TAP device is created with IFF_VNET_HDR for virtio compatibility.
    pub fn create(name: &str) -> Result<Self> {
        if name.len() > 15 {
            return Err(TapError::NameTooLong(name.to_string()));
        }

        // Open /dev/net/tun
        let tun_fd = open(c"/dev/net/tun", OFlag::O_RDWR, Mode::empty())
            .map_err(|e| TapError::OpenTun(io::Error::from_raw_os_error(e as i32)))?;

        // Create ifreq struct
        let mut ifreq = IfReq::default();
        let name_bytes = name.as_bytes();
        ifreq.ifr_name[..name_bytes.len()].copy_from_slice(name_bytes);
        ifreq.ifr_flags = IFF_TAP | IFF_NO_PI | IFF_VNET_HDR;

        // TUNSETIFF ioctl
        let ret = unsafe { libc::ioctl(tun_fd, TUNSETIFF as libc::Ioctl, &mut ifreq) };
        if ret < 0 {
            let _ = close(tun_fd);
            return Err(TapError::CreateDevice(
                name.to_string(),
                io::Error::last_os_error(),
            ));
        }

        let fd = unsafe { OwnedFd::from_raw_fd(tun_fd) };

        // Get interface index
        let if_index = Self::get_if_index(name)?;

        Ok(Self {
            name: name.to_string(),
            fd,
            if_index,
        })
    }

    /// Get interface index for a device by name.
    fn get_if_index(name: &str) -> Result<u32> {
        let c_name = CString::new(name).map_err(|_| {
            TapError::GetIfIndex(
                name.to_string(),
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid interface name"),
            )
        })?;

        let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
        if index == 0 {
            return Err(TapError::GetIfIndex(
                name.to_string(),
                io::Error::last_os_error(),
            ));
        }

        Ok(index)
    }

    /// Set the interface up.
    pub fn set_up(&self) -> Result<()> {
        let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if sock < 0 {
            return Err(TapError::SetUp(io::Error::last_os_error()));
        }

        let mut ifreq: libc::ifreq = unsafe { std::mem::zeroed() };
        let name_bytes = self.name.as_bytes();
        ifreq.ifr_name[..name_bytes.len()].copy_from_slice(unsafe {
            std::slice::from_raw_parts(name_bytes.as_ptr() as *const i8, name_bytes.len())
        });

        // Get current flags
        let ret = unsafe { libc::ioctl(sock, libc::SIOCGIFFLAGS as libc::Ioctl, &mut ifreq) };
        if ret < 0 {
            unsafe { libc::close(sock) };
            return Err(TapError::SetUp(io::Error::last_os_error()));
        }

        // Set IFF_UP flag
        unsafe {
            ifreq.ifr_ifru.ifru_flags |= libc::IFF_UP as i16;
        }

        let ret = unsafe { libc::ioctl(sock, libc::SIOCSIFFLAGS as libc::Ioctl, &ifreq) };
        unsafe { libc::close(sock) };

        if ret < 0 {
            return Err(TapError::SetUp(io::Error::last_os_error()));
        }

        Ok(())
    }

    /// Get the raw file descriptor.
    pub fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}

impl Drop for TapDevice {
    fn drop(&mut self) {
        // The TAP device is automatically deleted when the fd is closed
        // and no other process has it open.
        // The OwnedFd will close the fd automatically.
    }
}

/// Delete a TAP interface by name using netlink.
pub async fn delete_tap_interface(name: &str) -> Result<()> {
    use rtnetlink::new_connection;

    let (connection, handle, _) = new_connection()
        .map_err(|e| TapError::DeleteInterface(name.to_string(), io::Error::other(e)))?;

    tokio::spawn(connection);

    // Find the interface index
    let c_name = CString::new(name).map_err(|_| {
        TapError::DeleteInterface(
            name.to_string(),
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid interface name"),
        )
    })?;

    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    if index == 0 {
        // Interface doesn't exist, nothing to delete
        return Ok(());
    }

    // Delete the interface
    handle
        .link()
        .del(index)
        .execute()
        .await
        .map_err(|e| TapError::DeleteInterface(name.to_string(), io::Error::other(e)))?;

    Ok(())
}

/// Generate a TAP device name from a NIC ID.
///
/// Format: tap_<first 7 chars of UUID>
/// Total length: 11 chars (well under 15 char limit)
pub fn tap_name_from_nic_id(nic_id: &uuid::Uuid) -> String {
    let id_str = nic_id.to_string();
    format!("tap_{}", &id_str[..7])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tap_name_from_nic_id() {
        let id = uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();
        let name = tap_name_from_nic_id(&id);
        assert_eq!(name, "tap_1234567");
        assert!(name.len() <= 15);
    }

    #[test]
    fn test_name_too_long() {
        let result = TapDevice::create("this_name_is_way_too_long");
        assert!(matches!(result, Err(TapError::NameTooLong(_))));
    }
}
