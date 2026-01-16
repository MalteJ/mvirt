//! Zero-copy buffer pool for high-performance packet processing
//!
//! Pre-allocates memory using hugepages (if available) and provides
//! lock-free buffer allocation for the data plane.

use std::io;
use std::ptr::NonNull;
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use nix::sys::mman::{MapFlags, ProtFlags, mmap_anonymous, munmap};

/// Ethernet header size (headroom for prepending)
pub const ETH_HEADROOM: usize = 14;

/// Virtio-net header size (headroom)
pub const VIRTIO_HDR_SIZE: usize = 12;

/// Total headroom (Ethernet + Virtio header)
pub const HEADROOM: usize = ETH_HEADROOM + VIRTIO_HDR_SIZE;

/// Buffer size (64KB for GRO/GSO aggregated packets)
pub const BUFFER_SIZE: usize = 65536;

/// Maximum payload per buffer
pub const MAX_PACKET: usize = BUFFER_SIZE - HEADROOM;

/// Number of buffers in pool (512 × 64KB = 32MB)
pub const POOL_SIZE: usize = 512;

/// Memory-mapped buffer pool using hugepages
///
/// Provides lock-free allocation and deallocation of fixed-size buffers
/// for zero-copy packet processing.
pub struct BufferPool {
    /// Base pointer to mmap'd region
    base: NonNull<u8>,
    /// Total size of mapped region
    size: usize,
    /// Free list (lock-free stack of buffer indices)
    free: ArrayQueue<u32>,
    /// Whether hugepages are being used
    #[allow(dead_code)]
    using_hugepages: bool,
}

// SAFETY: BufferPool uses mmap'd memory that is process-global
// and ArrayQueue is thread-safe
unsafe impl Send for BufferPool {}
unsafe impl Sync for BufferPool {}

impl BufferPool {
    /// Create a new buffer pool
    ///
    /// Attempts to use 2MB hugepages for better TLB performance.
    /// Falls back to regular pages if hugepages are not available.
    pub fn new() -> io::Result<Self> {
        let size = BUFFER_SIZE * POOL_SIZE;

        // Try hugepages first (2MB pages)
        let (ptr, using_hugepages) =
            Self::try_mmap_hugepages(size).or_else(|_| Self::mmap_regular(size))?;

        let base =
            NonNull::new(ptr).ok_or_else(|| io::Error::other("mmap returned null pointer"))?;

        // Initialize free list with all buffer indices
        let free = ArrayQueue::new(POOL_SIZE);
        for i in 0..POOL_SIZE {
            // ArrayQueue::push only fails if full, which can't happen here
            let _ = free.push(i as u32);
        }

        if using_hugepages {
            tracing::info!(
                pool_size_mb = size / (1024 * 1024),
                buffer_count = POOL_SIZE,
                buffer_size_kb = BUFFER_SIZE / 1024,
                "Buffer pool created with hugepages"
            );
        } else {
            tracing::info!(
                pool_size_mb = size / (1024 * 1024),
                buffer_count = POOL_SIZE,
                buffer_size_kb = BUFFER_SIZE / 1024,
                "Buffer pool created with regular pages (hugepages unavailable)"
            );
        }

        Ok(Self {
            base,
            size,
            free,
            using_hugepages,
        })
    }

    /// Try to allocate memory using hugepages
    fn try_mmap_hugepages(size: usize) -> io::Result<(*mut u8, bool)> {
        // MAP_HUGETLB requests 2MB hugepages
        let flags = MapFlags::MAP_PRIVATE | MapFlags::MAP_ANONYMOUS | MapFlags::MAP_HUGETLB;

        let ptr = unsafe {
            mmap_anonymous(
                None,
                size.try_into()
                    .map_err(|_| io::Error::other("size overflow"))?,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                flags,
            )?
        };

        Ok((ptr.as_ptr().cast(), true))
    }

    /// Allocate memory using regular pages
    fn mmap_regular(size: usize) -> io::Result<(*mut u8, bool)> {
        let flags = MapFlags::MAP_PRIVATE | MapFlags::MAP_ANONYMOUS;

        let ptr = unsafe {
            mmap_anonymous(
                None,
                size.try_into()
                    .map_err(|_| io::Error::other("size overflow"))?,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                flags,
            )?
        };

        Ok((ptr.as_ptr().cast(), false))
    }

    /// Allocate a buffer from the pool
    ///
    /// Returns `None` if the pool is exhausted.
    /// Requires an Arc reference to allow the buffer to be sent across threads.
    #[inline]
    pub fn alloc(self: &Arc<Self>) -> Option<PoolBuffer> {
        self.free.pop().map(|idx| PoolBuffer {
            pool: Arc::clone(self),
            index: idx,
            start: HEADROOM,
            len: 0,
        })
    }

    /// Get the number of available buffers
    #[inline]
    pub fn available(&self) -> usize {
        self.free.len()
    }

    /// Get raw pointer to buffer at given index
    #[inline]
    fn buffer_ptr(&self, index: u32) -> *mut u8 {
        // SAFETY: index is always < POOL_SIZE (enforced by ArrayQueue capacity)
        unsafe { self.base.as_ptr().add(index as usize * BUFFER_SIZE) }
    }

    /// Return a buffer index to the pool (internal use)
    #[inline]
    fn return_buffer(&self, index: u32) {
        // ArrayQueue::push only fails if full, which shouldn't happen
        // if we're returning a buffer we previously allocated
        let _ = self.free.push(index);
    }
}

impl Drop for BufferPool {
    fn drop(&mut self) {
        // SAFETY: self.base was allocated by mmap in new()
        unsafe {
            let ptr = NonNull::new_unchecked(self.base.as_ptr().cast());
            let _ = munmap(ptr, self.size);
        }
    }
}

/// A buffer owned from the pool
///
/// Automatically returns to the pool when dropped.
/// Uses Arc to allow sending across threads.
pub struct PoolBuffer {
    pool: Arc<BufferPool>,
    index: u32,
    /// Start offset within buffer (for headroom management)
    pub start: usize,
    /// Length of valid data
    pub len: usize,
}

impl PoolBuffer {
    /// Get immutable slice to the data portion
    #[inline]
    pub fn data(&self) -> &[u8] {
        // SAFETY: start and len are always within buffer bounds
        unsafe {
            let ptr = self.pool.buffer_ptr(self.index).add(self.start);
            std::slice::from_raw_parts(ptr, self.len)
        }
    }

    /// Get mutable slice to the data portion
    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        // SAFETY: start and len are always within buffer bounds
        unsafe {
            let ptr = self.pool.buffer_ptr(self.index).add(self.start);
            std::slice::from_raw_parts_mut(ptr, self.len)
        }
    }

    /// Get mutable slice to the write area (from start to end of buffer)
    ///
    /// Use this for reading data into the buffer.
    #[inline]
    pub fn write_area(&mut self) -> &mut [u8] {
        // SAFETY: start is always within buffer bounds
        unsafe {
            let ptr = self.pool.buffer_ptr(self.index).add(self.start);
            let capacity = BUFFER_SIZE - self.start;
            std::slice::from_raw_parts_mut(ptr, capacity)
        }
    }

    /// Get the remaining capacity (bytes that can still be written)
    #[inline]
    pub fn remaining_capacity(&self) -> usize {
        BUFFER_SIZE - self.start - self.len
    }

    /// Prepend Ethernet header using headroom (no copy!)
    ///
    /// # Panics
    /// Panics if there's not enough headroom.
    #[inline]
    pub fn prepend_eth_header(&mut self, dst_mac: [u8; 6], src_mac: [u8; 6], ethertype: u16) {
        assert!(
            self.start >= ETH_HEADROOM,
            "Not enough headroom for Ethernet header"
        );
        self.start -= ETH_HEADROOM;
        self.len += ETH_HEADROOM;

        // SAFETY: we just checked headroom availability
        let header = unsafe {
            let ptr = self.pool.buffer_ptr(self.index).add(self.start);
            std::slice::from_raw_parts_mut(ptr, ETH_HEADROOM)
        };

        header[0..6].copy_from_slice(&dst_mac);
        header[6..12].copy_from_slice(&src_mac);
        header[12..14].copy_from_slice(&ethertype.to_be_bytes());
    }

    /// Prepend Virtio-net header using headroom (no copy!)
    ///
    /// Initializes header to zeros (no GSO, no checksum offload).
    ///
    /// # Panics
    /// Panics if there's not enough headroom.
    #[inline]
    pub fn prepend_virtio_hdr(&mut self) {
        assert!(
            self.start >= VIRTIO_HDR_SIZE,
            "Not enough headroom for Virtio header"
        );
        self.start -= VIRTIO_HDR_SIZE;
        self.len += VIRTIO_HDR_SIZE;

        // SAFETY: we just checked headroom availability
        let header = unsafe {
            let ptr = self.pool.buffer_ptr(self.index).add(self.start);
            std::slice::from_raw_parts_mut(ptr, VIRTIO_HDR_SIZE)
        };
        header.fill(0);
    }

    /// Strip Ethernet header (adjust offset, no copy!)
    ///
    /// # Panics
    /// Panics if buffer doesn't contain an Ethernet header.
    #[inline]
    pub fn strip_eth_header(&mut self) {
        assert!(
            self.len >= ETH_HEADROOM,
            "Buffer too small to strip Ethernet header"
        );
        self.start += ETH_HEADROOM;
        self.len -= ETH_HEADROOM;
    }

    /// Strip Virtio-net header (adjust offset, no copy!)
    ///
    /// # Panics
    /// Panics if buffer doesn't contain a Virtio header.
    #[inline]
    pub fn strip_virtio_hdr(&mut self) {
        assert!(
            self.len >= VIRTIO_HDR_SIZE,
            "Buffer too small to strip Virtio header"
        );
        self.start += VIRTIO_HDR_SIZE;
        self.len -= VIRTIO_HDR_SIZE;
    }

    /// Get IoSlice for writev (zero-copy send)
    #[inline]
    pub fn as_io_slice(&self) -> std::io::IoSlice<'_> {
        std::io::IoSlice::new(self.data())
    }

    /// Reset buffer to initial state (after headroom)
    ///
    /// Use this to reuse a buffer without returning it to the pool.
    #[inline]
    pub fn reset(&mut self) {
        self.start = HEADROOM;
        self.len = 0;
    }
}

impl Drop for PoolBuffer {
    #[inline]
    fn drop(&mut self) {
        self.pool.return_buffer(self.index);
    }
}

// PoolBuffer is Send because Arc<BufferPool> is Send + Sync
// and the other fields are plain data

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));
        assert_eq!(pool.available(), POOL_SIZE);
    }

    #[test]
    fn test_buffer_alloc_and_drop() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));

        let initial = pool.available();
        {
            let _buf = pool.alloc().expect("Failed to alloc");
            assert_eq!(pool.available(), initial - 1);
        }
        // Buffer returned on drop
        assert_eq!(pool.available(), initial);
    }

    #[test]
    fn test_buffer_write_and_read() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));
        let mut buf = pool.alloc().expect("Failed to alloc");

        let write_area = buf.write_area();
        write_area[0..5].copy_from_slice(b"hello");
        buf.len = 5;

        assert_eq!(buf.data(), b"hello");
    }

    #[test]
    fn test_eth_header_prepend() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));
        let mut buf = pool.alloc().expect("Failed to alloc");

        // Write some payload
        let write_area = buf.write_area();
        write_area[0..4].copy_from_slice(&[0x45, 0x00, 0x00, 0x28]); // IP header start
        buf.len = 4;

        // Prepend Ethernet header
        buf.prepend_eth_header(
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff], // dst MAC (broadcast)
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56], // src MAC
            0x0800,                               // IPv4
        );

        let data = buf.data();
        assert_eq!(data.len(), 4 + ETH_HEADROOM);
        assert_eq!(&data[0..6], &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // dst MAC
        assert_eq!(&data[6..12], &[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]); // src MAC
        assert_eq!(&data[12..14], &[0x08, 0x00]); // EtherType
        assert_eq!(&data[14..18], &[0x45, 0x00, 0x00, 0x28]); // IP header
    }

    #[test]
    fn test_strip_eth_header() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));
        let mut buf = pool.alloc().expect("Failed to alloc");

        // Write Ethernet frame
        let write_area = buf.write_area();
        write_area[0..18].copy_from_slice(&[
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // dst MAC
            0x52, 0x54, 0x00, 0x12, 0x34, 0x56, // src MAC
            0x08, 0x00, // EtherType
            0x45, 0x00, 0x00, 0x28, // IP header start
        ]);
        buf.len = 18;

        buf.strip_eth_header();

        assert_eq!(buf.data(), &[0x45, 0x00, 0x00, 0x28]);
    }

    #[test]
    fn test_pool_exhaustion() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));
        let mut buffers = Vec::new();

        // Allocate all buffers
        for _ in 0..POOL_SIZE {
            buffers.push(pool.alloc().expect("Should be able to alloc"));
        }

        // Pool should be empty now
        assert!(pool.alloc().is_none());
        assert_eq!(pool.available(), 0);

        // Return one buffer
        buffers.pop();
        assert_eq!(pool.available(), 1);
    }

    #[test]
    fn test_buffer_send_across_threads() {
        let pool = Arc::new(BufferPool::new().expect("Failed to create pool"));
        let mut buf = pool.alloc().expect("Failed to alloc");

        // Write data
        buf.write_area()[0..5].copy_from_slice(b"hello");
        buf.len = 5;

        // Send to another thread and receive back
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            tx.send(buf).unwrap();
        });

        let received = rx.recv().unwrap();
        assert_eq!(received.data(), b"hello");
    }
}
