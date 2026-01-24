use futures::TryStreamExt;
use nix::libc::{self, IFF_NO_PI, IFF_TUN, IFF_VNET_HDR, IFNAMSIZ, c_char, c_short, c_uint};
use rtnetlink::Handle;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::ManuallyDrop;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use tracing::{debug, info, warn};

const TUNSETIFF: nix::libc::Ioctl = 0x400454ca as nix::libc::Ioctl;
const TUNSETVNETHDRSZ: nix::libc::Ioctl = 0x400454d8 as nix::libc::Ioctl;

/// Size of the virtio_net_hdr structure (12 bytes for v1)
pub const VNET_HDR_SIZE: usize = 12;

/// virtio_net_hdr structure prepended to each packet when IFF_VNET_HDR is set
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

#[repr(C)]
struct IfReq {
    ifr_name: [c_char; IFNAMSIZ],
    ifr_flags: c_short,
    _padding: [u8; 22],
}

pub struct TunDevice {
    pub name: String,
    file: ManuallyDrop<File>,
    pub if_index: u32,
}

impl TunDevice {
    pub async fn create(name: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/net/tun")?;

        let mut ifr = IfReq {
            ifr_name: [0; IFNAMSIZ],
            ifr_flags: (IFF_TUN | IFF_NO_PI | IFF_VNET_HDR) as c_short,
            _padding: [0; 22],
        };

        for (i, byte) in name.bytes().enumerate() {
            if i >= IFNAMSIZ - 1 {
                break;
            }
            ifr.ifr_name[i] = byte as c_char;
        }

        let result = unsafe { nix::libc::ioctl(file.as_raw_fd(), TUNSETIFF, &ifr) };

        if result < 0 {
            return Err(io::Error::last_os_error());
        }

        // Set the vnet header size
        let vnet_hdr_sz: c_uint = VNET_HDR_SIZE as c_uint;
        let result = unsafe { nix::libc::ioctl(file.as_raw_fd(), TUNSETVNETHDRSZ, &vnet_hdr_sz) };

        if result < 0 {
            return Err(io::Error::last_os_error());
        }

        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        let if_index = Self::get_interface_index(&handle, name)
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Interface not found"))?;

        info!(name, if_index, "TUN device created");

        Ok(TunDevice {
            name: name.to_string(),
            file: ManuallyDrop::new(file),
            if_index,
        })
    }

    async fn get_interface_index(handle: &Handle, name: &str) -> Option<u32> {
        let mut links = handle.link().get().match_name(name.to_string()).execute();
        if let Ok(Some(link)) = links.try_next().await {
            return Some(link.header.index);
        }
        None
    }

    pub async fn set_up(&self) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        handle
            .link()
            .set(self.if_index)
            .up()
            .execute()
            .await
            .map_err(io::Error::other)?;
        info!(name = %self.name, "Interface set UP");
        Ok(())
    }

    pub async fn add_address(&self, addr: Ipv4Addr, prefix_len: u8) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        handle
            .address()
            .add(self.if_index, std::net::IpAddr::V4(addr), prefix_len)
            .execute()
            .await
            .map_err(io::Error::other)?;

        info!(addr = %addr, prefix_len, name = %self.name, "Address added");
        Ok(())
    }

    /// Add a kernel route via this TUN device.
    ///
    /// This adds a route in the kernel's routing table so that traffic for the
    /// given prefix is delivered to this TUN device.
    pub async fn add_route_v4(&self, prefix: Ipv4Addr, prefix_len: u8) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        match handle
            .route()
            .add()
            .v4()
            .destination_prefix(prefix, prefix_len)
            .output_interface(self.if_index)
            .execute()
            .await
        {
            Ok(()) => {
                info!(prefix = %prefix, prefix_len, name = %self.name, "Kernel route added");
                Ok(())
            }
            Err(rtnetlink::Error::NetlinkError(e)) if e.raw_code() == -libc::EEXIST => {
                // Route already exists, that's fine
                warn!(prefix = %prefix, prefix_len, name = %self.name, "Kernel route already exists");
                Ok(())
            }
            Err(e) => Err(io::Error::other(e)),
        }
    }

    /// Add an IPv6 kernel route via this TUN device.
    pub async fn add_route_v6(&self, prefix: Ipv6Addr, prefix_len: u8) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        match handle
            .route()
            .add()
            .v6()
            .destination_prefix(prefix, prefix_len)
            .output_interface(self.if_index)
            .execute()
            .await
        {
            Ok(()) => {
                info!(prefix = %prefix, prefix_len, name = %self.name, "IPv6 kernel route added");
                Ok(())
            }
            Err(rtnetlink::Error::NetlinkError(e)) if e.raw_code() == -libc::EEXIST => {
                // Route already exists, that's fine
                warn!(prefix = %prefix, prefix_len, name = %self.name, "IPv6 kernel route already exists");
                Ok(())
            }
            Err(e) => Err(io::Error::other(e)),
        }
    }
}

impl TunDevice {
    pub fn into_file(mut self) -> File {
        let file = unsafe { ManuallyDrop::take(&mut self.file) };
        std::mem::forget(self);
        file
    }

    pub async fn delete(name: &str) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        let if_index = Self::get_interface_index(&handle, name)
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Interface not found"))?;

        handle
            .link()
            .del(if_index)
            .execute()
            .await
            .map_err(io::Error::other)?;

        info!(name, "TUN device deleted");
        Ok(())
    }
}

impl Drop for TunDevice {
    fn drop(&mut self) {
        debug!(name = %self.name, "TUN device closed");
    }
}
