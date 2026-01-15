//! Guest memory management for vhost-user tests
//!
//! Provides a test guest memory region that can be shared with the vhost-user backend.

use nix::libc;
use vm_memory::{
    Address, Bytes, GuestAddress, GuestMemory, GuestMemoryMmap, GuestMemoryRegion, GuestRegionMmap,
    MmapRegion,
};

/// Size of the test memory region (2MB)
pub const MEMORY_SIZE: u64 = 2 * 1024 * 1024;

/// Guest memory layout:
/// 0x0000_0000 - 0x0000_FFFF: Descriptor tables (64KB)
/// 0x0001_0000 - 0x0001_FFFF: Available rings (64KB)
/// 0x0002_0000 - 0x0002_FFFF: Used rings (64KB)
/// 0x0003_0000 - 0x001F_FFFF: Data buffers (~1.8MB)
pub const DESC_TABLE_OFFSET: u64 = 0x0000_0000;
pub const AVAIL_RING_OFFSET: u64 = 0x0001_0000;
pub const USED_RING_OFFSET: u64 = 0x0002_0000;
pub const DATA_BUFFER_OFFSET: u64 = 0x0003_0000;

/// Per-queue offsets (each queue gets 32KB of each region)
pub const QUEUE_REGION_SIZE: u64 = 0x8000; // 32KB

/// Test guest memory wrapper
pub struct TestGuestMemory {
    mem: GuestMemoryMmap,
    /// Next free offset in data buffer region
    next_data_offset: u64,
}

impl TestGuestMemory {
    /// Create a new test guest memory
    pub fn new() -> std::io::Result<Self> {
        // Create anonymous mmap region
        let region = MmapRegion::new(MEMORY_SIZE as usize)
            .map_err(|e| std::io::Error::other(format!("MmapRegion::new: {e}")))?;
        let guest_region = GuestRegionMmap::new(region, GuestAddress(0))
            .map_err(|e| std::io::Error::other(format!("GuestRegionMmap::new: {e}")))?;
        let mem = GuestMemoryMmap::from_regions(vec![guest_region])
            .map_err(|e| std::io::Error::other(format!("GuestMemoryMmap::from_regions: {e}")))?;

        Ok(Self {
            mem,
            next_data_offset: DATA_BUFFER_OFFSET,
        })
    }

    /// Get the underlying guest memory
    pub fn memory(&self) -> &GuestMemoryMmap {
        &self.mem
    }

    /// Allocate a data buffer and return its guest address
    pub fn alloc_buffer(&mut self, size: usize) -> GuestAddress {
        let addr = GuestAddress(self.next_data_offset);
        // Align to 16 bytes
        self.next_data_offset += ((size + 15) & !15) as u64;
        addr
    }

    /// Write data to a guest address
    pub fn write(&self, addr: GuestAddress, data: &[u8]) -> std::io::Result<()> {
        self.mem
            .write(data, addr)
            .map_err(|e| std::io::Error::other(format!("write failed: {e}")))
            .map(|_| ())
    }

    /// Read data from a guest address
    pub fn read(&self, addr: GuestAddress, buf: &mut [u8]) -> std::io::Result<()> {
        self.mem
            .read(buf, addr)
            .map_err(|e| std::io::Error::other(format!("read failed: {e}")))
            .map(|_| ())
    }

    /// Get the host pointer for a guest address (for mmap table setup)
    pub fn get_host_address(&self, addr: GuestAddress) -> Option<*mut u8> {
        self.mem.find_region(addr).map(|region| {
            let offset = addr.raw_value() - region.start_addr().raw_value();
            unsafe { region.as_ptr().add(offset as usize) }
        })
    }

    /// Get descriptor table address for a queue
    pub fn desc_table_addr(&self, queue_idx: u16) -> GuestAddress {
        GuestAddress(DESC_TABLE_OFFSET + (queue_idx as u64) * QUEUE_REGION_SIZE)
    }

    /// Get available ring address for a queue
    pub fn avail_ring_addr(&self, queue_idx: u16) -> GuestAddress {
        GuestAddress(AVAIL_RING_OFFSET + (queue_idx as u64) * QUEUE_REGION_SIZE)
    }

    /// Get used ring address for a queue
    pub fn used_ring_addr(&self, queue_idx: u16) -> GuestAddress {
        GuestAddress(USED_RING_OFFSET + (queue_idx as u64) * QUEUE_REGION_SIZE)
    }

    /// Get memory region info for vhost-user SET_MEM_TABLE
    pub fn get_region_info(&self) -> (u64, u64, u64, i32) {
        let region = self.mem.iter().next().unwrap();
        let guest_phys_addr = region.start_addr().raw_value();
        let size = region.len();
        let userspace_addr = region.as_ptr() as u64;

        // For tests, we use memfd to get a shareable fd
        // In real scenarios this would be the actual memory fd
        let fd = -1; // Will be handled by the test setup

        (guest_phys_addr, size, userspace_addr, fd)
    }
}

impl Default for TestGuestMemory {
    fn default() -> Self {
        Self::new().expect("Failed to create test guest memory")
    }
}

/// Create a memfd for sharing memory with the vhost-user backend
pub fn create_memfd(size: u64) -> std::io::Result<std::os::unix::io::RawFd> {
    use std::ffi::CString;

    let name = CString::new("vhost-test-mem").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Set size
    let ret = unsafe { libc::ftruncate(fd, size as libc::off_t) };
    if ret < 0 {
        unsafe { libc::close(fd) };
        return Err(std::io::Error::last_os_error());
    }

    Ok(fd)
}

/// Shared memory region that can be passed to vhost-user backend
pub struct SharedMemory {
    pub fd: std::os::unix::io::RawFd,
    pub size: u64,
    pub ptr: *mut u8,
}

impl SharedMemory {
    pub fn new(size: u64) -> std::io::Result<Self> {
        let fd = create_memfd(size)?;

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            unsafe { libc::close(fd) };
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            fd,
            size,
            ptr: ptr as *mut u8,
        })
    }

    /// Create GuestMemoryMmap from this shared memory
    pub fn to_guest_memory(&self) -> std::io::Result<GuestMemoryMmap> {
        // Create region from the existing mmap
        let region = unsafe {
            MmapRegion::build_raw(
                self.ptr,
                self.size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
            )
            .map_err(|e| std::io::Error::other(format!("build_raw: {e}")))?
        };

        let guest_region = GuestRegionMmap::new(region, GuestAddress(0))
            .map_err(|e| std::io::Error::other(format!("GuestRegionMmap: {e}")))?;

        GuestMemoryMmap::from_regions(vec![guest_region])
            .map_err(|e| std::io::Error::other(format!("from_regions: {e}")))
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.size as usize);
            libc::close(self.fd);
        }
    }
}
