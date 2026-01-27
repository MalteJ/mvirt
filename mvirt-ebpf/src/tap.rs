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

    #[error("Failed to add route for {0}: {1}")]
    AddRoute(String, io::Error),

    #[error("Failed to remove route for {0}: {1}")]
    RemoveRoute(String, io::Error),
}

pub type Result<T> = std::result::Result<T, TapError>;

// ioctl constants for TUN/TAP
const TUNSETIFF: libc::c_ulong = 0x400454ca;
const TUNSETPERSIST: libc::c_ulong = 0x400454cb;
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

/// Create a persistent TAP device.
///
/// The TAP device is created with TUNSETPERSIST, so it remains even after
/// the fd is closed. This allows cloud-hypervisor to open it later.
/// Returns the interface index.
pub fn create_persistent_tap(name: &str) -> Result<u32> {
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

    // TUNSETIFF ioctl - create the TAP device
    let ret = unsafe { libc::ioctl(tun_fd, TUNSETIFF as libc::Ioctl, &mut ifreq) };
    if ret < 0 {
        let err = io::Error::last_os_error();
        let _ = close(tun_fd);
        return Err(TapError::CreateDevice(name.to_string(), err));
    }

    // TUNSETPERSIST - make it persistent (survives fd close)
    let ret = unsafe { libc::ioctl(tun_fd, TUNSETPERSIST as libc::Ioctl, 1i32) };
    if ret < 0 {
        let err = io::Error::last_os_error();
        let _ = close(tun_fd);
        return Err(TapError::CreateDevice(
            name.to_string(),
            io::Error::new(err.kind(), format!("TUNSETPERSIST failed: {}", err)),
        ));
    }

    // Get interface index before closing fd
    let if_index = get_if_index_by_name(name)?;

    // Close fd - TAP persists due to TUNSETPERSIST
    let _ = close(tun_fd);

    Ok(if_index)
}

/// Set the MAC address of an interface.
pub fn set_interface_mac(name: &str, mac: [u8; 6]) -> Result<()> {
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        return Err(TapError::SetUp(io::Error::last_os_error()));
    }

    let mut ifreq: libc::ifreq = unsafe { std::mem::zeroed() };
    let name_bytes = name.as_bytes();
    if name_bytes.len() > 15 {
        unsafe { libc::close(sock) };
        return Err(TapError::NameTooLong(name.to_string()));
    }
    ifreq.ifr_name[..name_bytes.len()].copy_from_slice(unsafe {
        std::slice::from_raw_parts(name_bytes.as_ptr() as *const i8, name_bytes.len())
    });

    // Set MAC address in ifr_hwaddr
    unsafe {
        ifreq.ifr_ifru.ifru_hwaddr.sa_family = libc::ARPHRD_ETHER;
        ifreq.ifr_ifru.ifru_hwaddr.sa_data[..6]
            .copy_from_slice(std::slice::from_raw_parts(mac.as_ptr() as *const i8, 6));
    }

    let ret = unsafe { libc::ioctl(sock, libc::SIOCSIFHWADDR as libc::Ioctl, &ifreq) };
    unsafe { libc::close(sock) };

    if ret < 0 {
        return Err(TapError::SetUp(io::Error::last_os_error()));
    }

    Ok(())
}

/// Set an interface up by name.
pub fn set_interface_up(name: &str) -> Result<()> {
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        return Err(TapError::SetUp(io::Error::last_os_error()));
    }

    let mut ifreq: libc::ifreq = unsafe { std::mem::zeroed() };
    let name_bytes = name.as_bytes();
    if name_bytes.len() > 15 {
        unsafe { libc::close(sock) };
        return Err(TapError::NameTooLong(name.to_string()));
    }
    ifreq.ifr_name[..name_bytes.len()].copy_from_slice(unsafe {
        std::slice::from_raw_parts(name_bytes.as_ptr() as *const i8, name_bytes.len())
    });

    // Get current flags
    let ret = unsafe { libc::ioctl(sock, libc::SIOCGIFFLAGS as libc::Ioctl, &mut ifreq) };
    if ret < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(sock) };
        return Err(TapError::SetUp(err));
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

/// Get interface index for an existing interface by name.
///
/// Returns the interface index if the interface exists, or an error if not found.
pub fn get_if_index_by_name(name: &str) -> Result<u32> {
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
        get_if_index_by_name(name)
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

/// Add a host route for a VM's IP address through the TAP device.
///
/// This adds a /32 route for IPv4 (or /128 for IPv6) so the kernel knows
/// how to route return traffic back to the VM.
pub async fn add_host_route_v4(addr: std::net::Ipv4Addr, if_index: u32) -> Result<()> {
    // Get interface name from index
    let if_name =
        if_name_from_index(if_index).map_err(|e| TapError::AddRoute(addr.to_string(), e))?;

    let output = std::process::Command::new("ip")
        .args(["route", "add", &format!("{}/32", addr), "dev", &if_name])
        .output()
        .map_err(|e| TapError::AddRoute(addr.to_string(), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "File exists" error (route already present)
        if !stderr.contains("File exists") {
            return Err(TapError::AddRoute(
                addr.to_string(),
                io::Error::other(stderr.to_string()),
            ));
        }
    }

    Ok(())
}

/// Remove a host route for a VM's IP address.
pub async fn remove_host_route_v4(addr: std::net::Ipv4Addr, if_index: u32) -> Result<()> {
    let if_name =
        if_name_from_index(if_index).map_err(|e| TapError::RemoveRoute(addr.to_string(), e))?;

    let output = std::process::Command::new("ip")
        .args(["route", "del", &format!("{}/32", addr), "dev", &if_name])
        .output()
        .map_err(|e| TapError::RemoveRoute(addr.to_string(), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "No such process" error (route doesn't exist)
        if !stderr.contains("No such process") && !stderr.contains("Cannot find device") {
            return Err(TapError::RemoveRoute(
                addr.to_string(),
                io::Error::other(stderr.to_string()),
            ));
        }
    }

    Ok(())
}

/// Add a host route for a VM's IPv6 address through the TAP device.
pub async fn add_host_route_v6(addr: std::net::Ipv6Addr, if_index: u32) -> Result<()> {
    let if_name =
        if_name_from_index(if_index).map_err(|e| TapError::AddRoute(addr.to_string(), e))?;

    let output = std::process::Command::new("ip")
        .args([
            "-6",
            "route",
            "add",
            &format!("{}/128", addr),
            "dev",
            &if_name,
        ])
        .output()
        .map_err(|e| TapError::AddRoute(addr.to_string(), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("File exists") {
            return Err(TapError::AddRoute(
                addr.to_string(),
                io::Error::other(stderr.to_string()),
            ));
        }
    }

    Ok(())
}

/// Remove a host route for a VM's IPv6 address.
pub async fn remove_host_route_v6(addr: std::net::Ipv6Addr, if_index: u32) -> Result<()> {
    let if_name =
        if_name_from_index(if_index).map_err(|e| TapError::RemoveRoute(addr.to_string(), e))?;

    let output = std::process::Command::new("ip")
        .args([
            "-6",
            "route",
            "del",
            &format!("{}/128", addr),
            "dev",
            &if_name,
        ])
        .output()
        .map_err(|e| TapError::RemoveRoute(addr.to_string(), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("No such process") && !stderr.contains("Cannot find device") {
            return Err(TapError::RemoveRoute(
                addr.to_string(),
                io::Error::other(stderr.to_string()),
            ));
        }
    }

    Ok(())
}

/// Get interface name from index.
fn if_name_from_index(if_index: u32) -> io::Result<String> {
    let mut name = [0u8; 16];
    let ptr = name.as_mut_ptr() as *mut libc::c_char;
    let result = unsafe { libc::if_indextoname(if_index, ptr) };
    if result.is_null() {
        return Err(io::Error::last_os_error());
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Ok(String::from_utf8_lossy(&name[..len]).to_string())
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
