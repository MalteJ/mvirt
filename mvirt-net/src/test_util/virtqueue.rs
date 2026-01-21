//! Virtqueue driver implementation for test frontends
//!
//! This implements a realistic virtqueue driver (guest/driver side) based on
//! the Linux kernel's virtio_ring implementation, including:
//! - Free-list descriptor management
//! - Descriptor chaining (scatter-gather)
//! - Event index support (VIRTIO_F_RING_EVENT_IDX)

use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{Ordering, fence};

use vm_memory::{Bytes, GuestAddress, GuestMemory};
use vmm_sys_util::eventfd::EventFd;

/// Virtio descriptor flags
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Ring layout constants
const DESC_SIZE: u64 = 16;
const AVAIL_RING_HEADER: u64 = 4; // flags(2) + idx(2)
const AVAIL_RING_ELEM: u64 = 2;
const USED_RING_HEADER: u64 = 4; // flags(2) + idx(2)
const USED_RING_ELEM: u64 = 8; // id(4) + len(4)

/// State for each descriptor in the ring
#[derive(Clone, Default)]
struct DescState {
    /// Opaque token for callback data
    token: u64,
    /// Number of descriptors in the chain (for freeing)
    chain_len: u16,
}

/// Used buffer information returned by pop_used
#[derive(Debug)]
pub struct UsedBuffer {
    /// The token associated with this buffer
    pub token: u64,
    /// Number of bytes written by the device
    pub len: u32,
    /// The head descriptor index
    pub head: u16,
}

/// Virtqueue driver (guest/driver side)
///
/// Implements the driver side of a virtqueue, managing descriptors,
/// the available ring, and processing the used ring.
pub struct VirtqueueDriver {
    /// Queue size (number of descriptors)
    size: u16,
    /// Guest address of descriptor table
    desc_addr: u64,
    /// Guest address of available ring
    avail_addr: u64,
    /// Guest address of used ring
    used_addr: u64,

    /// Head of free descriptor list
    free_head: u16,
    /// Number of free descriptors
    num_free: u16,

    /// Shadow of avail->idx for tracking what we've published
    avail_idx_shadow: u16,
    /// Last seen used->idx
    last_used_idx: u16,

    /// Event index feature enabled
    event_idx: bool,

    /// Per-descriptor state for tokens and chain info
    desc_state: Vec<DescState>,

    /// EventFd to kick the backend
    kick: EventFd,
    /// EventFd signaled by backend on completion
    call: EventFd,
}

impl VirtqueueDriver {
    /// Create a new virtqueue driver
    ///
    /// # Arguments
    /// * `size` - Number of descriptors in the queue
    /// * `base_addr` - Guest physical address for the queue structures
    /// * `event_idx` - Whether to enable VIRTIO_F_RING_EVENT_IDX
    pub fn new(size: u16, base_addr: u64, event_idx: bool) -> io::Result<Self> {
        let desc_addr = base_addr;
        // Available ring follows descriptor table
        let avail_addr = desc_addr + (size as u64 * DESC_SIZE);
        // Used ring follows available ring (with alignment to 4 bytes)
        // avail ring: flags(2) + idx(2) + ring[size](2*size) + used_event(2)
        let avail_size = AVAIL_RING_HEADER + (size as u64 * AVAIL_RING_ELEM) + 2; // +2 for used_event
        let used_addr = (avail_addr + avail_size + 3) & !3; // Align to 4 bytes

        Ok(VirtqueueDriver {
            size,
            desc_addr,
            avail_addr,
            used_addr,
            free_head: 0,
            num_free: size,
            avail_idx_shadow: 0,
            last_used_idx: 0,
            event_idx,
            desc_state: vec![DescState::default(); size as usize],
            kick: EventFd::new(0).map_err(io::Error::other)?,
            call: EventFd::new(0).map_err(io::Error::other)?,
        })
    }

    /// Get the total size needed for this queue's rings in memory
    pub fn total_size(size: u16) -> u64 {
        let desc_size = size as u64 * DESC_SIZE;
        // avail: flags(2) + idx(2) + ring[size] + used_event(2)
        let avail_size = AVAIL_RING_HEADER + (size as u64 * AVAIL_RING_ELEM) + 2;
        // used: flags(2) + idx(2) + ring[size] + avail_event(2)
        let used_size = USED_RING_HEADER + (size as u64 * USED_RING_ELEM) + 2;
        // Include alignment
        desc_size + ((avail_size + 3) & !3) + ((used_size + 3) & !3)
    }

    /// Initialize the queue structures in guest memory
    ///
    /// This zeros the memory and sets up the free descriptor list
    /// (each descriptor points to the next as a linked list).
    pub fn init<M: GuestMemory>(&mut self, mem: &M) -> io::Result<()> {
        // Zero out the queue area
        let total = Self::total_size(self.size);
        let zeros = vec![0u8; total as usize];
        mem.write_slice(&zeros, GuestAddress(self.desc_addr))
            .map_err(|e| io::Error::other(format!("write failed: {:?}", e)))?;

        // Initialize free list: desc[i].next = i + 1
        // The kernel does this to have a linked list of free descriptors
        for i in 0..self.size {
            let next = if i == self.size - 1 { 0 } else { i + 1 };
            let desc_offset = self.desc_addr + (i as u64 * DESC_SIZE) + 14; // next field
            mem.write_obj(next, GuestAddress(desc_offset))
                .map_err(|e| io::Error::other(format!("init next: {:?}", e)))?;
        }

        self.free_head = 0;
        self.num_free = self.size;
        self.avail_idx_shadow = 0;
        self.last_used_idx = 0;

        Ok(())
    }

    /// Get the descriptor table address
    pub fn desc_addr(&self) -> u64 {
        self.desc_addr
    }

    /// Get the available ring address
    pub fn avail_addr(&self) -> u64 {
        self.avail_addr
    }

    /// Get the used ring address
    pub fn used_addr(&self) -> u64 {
        self.used_addr
    }

    /// Get the queue size
    pub fn size(&self) -> u16 {
        self.size
    }

    /// Get a reference to the kick eventfd
    pub fn kick_fd(&self) -> &EventFd {
        &self.kick
    }

    /// Get a reference to the call eventfd
    pub fn call_fd(&self) -> &EventFd {
        &self.call
    }

    /// Get number of free descriptors
    pub fn num_free(&self) -> u16 {
        self.num_free
    }

    /// Add an output buffer chain to the queue (for TX)
    ///
    /// The buffers are chained using VIRTQ_DESC_F_NEXT flags.
    /// This is like Linux kernel's virtqueue_add_outbuf().
    ///
    /// # Arguments
    /// * `mem` - Guest memory
    /// * `buffers` - Slice of (address, length) pairs to chain
    /// * `token` - Opaque value returned when buffer is used
    ///
    /// # Returns
    /// The head descriptor index
    pub fn add_outbuf<M: GuestMemory>(
        &mut self,
        mem: &M,
        buffers: &[(u64, u32)],
        token: u64,
    ) -> io::Result<u16> {
        self.add_buf_internal(mem, buffers, &[], token)
    }

    /// Add an input buffer to the queue (for RX)
    ///
    /// This is like Linux kernel's virtqueue_add_inbuf().
    pub fn add_inbuf<M: GuestMemory>(
        &mut self,
        mem: &M,
        buf_addr: u64,
        len: u32,
        token: u64,
    ) -> io::Result<u16> {
        self.add_buf_internal(mem, &[], &[(buf_addr, len)], token)
    }

    /// Internal: add buffers with both out and in descriptors
    fn add_buf_internal<M: GuestMemory>(
        &mut self,
        mem: &M,
        out_bufs: &[(u64, u32)],
        in_bufs: &[(u64, u32)],
        token: u64,
    ) -> io::Result<u16> {
        let total = out_bufs.len() + in_bufs.len();
        if total == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty buffer list",
            ));
        }
        if total > self.num_free as usize {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("need {} descriptors, only {} free", total, self.num_free),
            ));
        }

        let head = self.free_head;
        let mut desc_idx = head;
        let mut prev_idx = head;

        // Process output buffers (device reads)
        for (i, &(addr, len)) in out_bufs.iter().enumerate() {
            let is_last = i == out_bufs.len() - 1 && in_bufs.is_empty();
            self.write_desc(mem, desc_idx, addr, len, false, !is_last)?;

            prev_idx = desc_idx;
            if !is_last {
                desc_idx = self.read_desc_next(mem, desc_idx)?;
            }
        }

        // Process input buffers (device writes)
        for (i, &(addr, len)) in in_bufs.iter().enumerate() {
            let is_last = i == in_bufs.len() - 1;
            self.write_desc(mem, desc_idx, addr, len, true, !is_last)?;

            prev_idx = desc_idx;
            if !is_last {
                desc_idx = self.read_desc_next(mem, desc_idx)?;
            }
        }

        // Update free list head
        self.free_head = self.read_desc_next(mem, prev_idx)?;
        self.num_free -= total as u16;

        // Save state for this chain
        self.desc_state[head as usize] = DescState {
            token,
            chain_len: total as u16,
        };

        // Add to available ring
        let avail_idx = self.avail_idx_shadow % self.size;
        let ring_offset =
            self.avail_addr + AVAIL_RING_HEADER + (avail_idx as u64 * AVAIL_RING_ELEM);
        mem.write_obj(head, GuestAddress(ring_offset))
            .map_err(|e| io::Error::other(format!("write avail ring: {:?}", e)))?;

        // Memory barrier before updating avail->idx
        fence(Ordering::SeqCst);

        // Update avail->idx
        self.avail_idx_shadow = self.avail_idx_shadow.wrapping_add(1);
        mem.write_obj(self.avail_idx_shadow, GuestAddress(self.avail_addr + 2))
            .map_err(|e| io::Error::other(format!("write avail idx: {:?}", e)))?;

        Ok(head)
    }

    /// Write a descriptor
    fn write_desc<M: GuestMemory>(
        &self,
        mem: &M,
        idx: u16,
        addr: u64,
        len: u32,
        write: bool,
        has_next: bool,
    ) -> io::Result<()> {
        let desc_offset = self.desc_addr + (idx as u64 * DESC_SIZE);

        let mut flags: u16 = 0;
        if write {
            flags |= VIRTQ_DESC_F_WRITE;
        }
        if has_next {
            flags |= VIRTQ_DESC_F_NEXT;
        }

        // Descriptor layout: addr(8) + len(4) + flags(2) + next(2)
        mem.write_obj(addr, GuestAddress(desc_offset))
            .map_err(|e| io::Error::other(format!("write desc addr: {:?}", e)))?;
        mem.write_obj(len, GuestAddress(desc_offset + 8))
            .map_err(|e| io::Error::other(format!("write desc len: {:?}", e)))?;
        mem.write_obj(flags, GuestAddress(desc_offset + 12))
            .map_err(|e| io::Error::other(format!("write desc flags: {:?}", e)))?;
        // next field is already set from init or previous chain

        Ok(())
    }

    /// Read the next field from a descriptor
    fn read_desc_next<M: GuestMemory>(&self, mem: &M, idx: u16) -> io::Result<u16> {
        let desc_offset = self.desc_addr + (idx as u64 * DESC_SIZE) + 14;
        mem.read_obj(GuestAddress(desc_offset))
            .map_err(|e| io::Error::other(format!("read desc next: {:?}", e)))
    }

    /// Pop a used buffer from the used ring
    ///
    /// Returns None if no buffers have been used.
    /// This is like Linux kernel's virtqueue_get_buf().
    pub fn pop_used<M: GuestMemory>(&mut self, mem: &M) -> io::Result<Option<UsedBuffer>> {
        // Read used->idx
        let used_idx: u16 = mem
            .read_obj(GuestAddress(self.used_addr + 2))
            .map_err(|e| io::Error::other(format!("read used idx: {:?}", e)))?;

        if used_idx == self.last_used_idx {
            return Ok(None);
        }

        // Memory barrier before reading used ring entry
        fence(Ordering::SeqCst);

        // Read the used ring entry
        let ring_idx = self.last_used_idx % self.size;
        let elem_offset = self.used_addr + USED_RING_HEADER + (ring_idx as u64 * USED_RING_ELEM);

        let id: u32 = mem
            .read_obj(GuestAddress(elem_offset))
            .map_err(|e| io::Error::other(format!("read used id: {:?}", e)))?;
        let len: u32 = mem
            .read_obj(GuestAddress(elem_offset + 4))
            .map_err(|e| io::Error::other(format!("read used len: {:?}", e)))?;

        let head = id as u16;
        let state = &self.desc_state[head as usize];
        let token = state.token;
        let chain_len = state.chain_len;

        // Return descriptors to free list
        self.return_descriptors_to_free_list(mem, head, chain_len)?;

        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        // Update used_event if event_idx is enabled
        if self.event_idx {
            self.write_used_event(mem, self.last_used_idx)?;
        }

        Ok(Some(UsedBuffer { token, len, head }))
    }

    /// Return a chain of descriptors to the free list
    fn return_descriptors_to_free_list<M: GuestMemory>(
        &mut self,
        mem: &M,
        head: u16,
        count: u16,
    ) -> io::Result<()> {
        // Find the tail of the chain
        let mut tail = head;
        for _ in 0..count - 1 {
            tail = self.read_desc_next(mem, tail)?;
        }

        // Link tail to current free_head
        let tail_next_offset = self.desc_addr + (tail as u64 * DESC_SIZE) + 14;
        mem.write_obj(self.free_head, GuestAddress(tail_next_offset))
            .map_err(|e| io::Error::other(format!("write free link: {:?}", e)))?;

        // Update free_head and count
        self.free_head = head;
        self.num_free += count;

        Ok(())
    }

    /// Write used_event (at end of available ring)
    fn write_used_event<M: GuestMemory>(&self, mem: &M, event: u16) -> io::Result<()> {
        // used_event is at avail->ring[num], i.e., after the last ring entry
        let used_event_offset =
            self.avail_addr + AVAIL_RING_HEADER + (self.size as u64 * AVAIL_RING_ELEM);
        mem.write_obj(event, GuestAddress(used_event_offset))
            .map_err(|e| io::Error::other(format!("write used_event: {:?}", e)))
    }

    /// Read avail_event from used ring (set by device)
    fn read_avail_event<M: GuestMemory>(&self, mem: &M) -> io::Result<u16> {
        // avail_event is at used->ring[num], stored as u16 at offset after used ring entries
        let avail_event_offset =
            self.used_addr + USED_RING_HEADER + (self.size as u64 * USED_RING_ELEM);
        mem.read_obj(GuestAddress(avail_event_offset))
            .map_err(|e| io::Error::other(format!("read avail_event: {:?}", e)))
    }

    /// Read used ring flags
    fn read_used_flags<M: GuestMemory>(&self, mem: &M) -> io::Result<u16> {
        mem.read_obj(GuestAddress(self.used_addr))
            .map_err(|e| io::Error::other(format!("read used flags: {:?}", e)))
    }

    /// Check if we need to notify the device
    ///
    /// With event_idx: uses vring_need_event() algorithm
    /// Without event_idx: checks VRING_USED_F_NO_NOTIFY flag
    pub fn needs_kick<M: GuestMemory>(&self, mem: &M) -> io::Result<bool> {
        // Memory barrier
        fence(Ordering::SeqCst);

        if self.event_idx {
            let avail_event = self.read_avail_event(mem)?;
            // We need to notify if new >= event + 1
            // This is: (new - event - 1) < (new - old)
            // where old is the previous avail_idx value (before this batch)
            // For simplicity, we always kick after adding buffers
            // A more optimized version would track old_avail_idx
            Ok(vring_need_event(
                avail_event,
                self.avail_idx_shadow,
                self.avail_idx_shadow.wrapping_sub(1),
            ))
        } else {
            // Check VRING_USED_F_NO_NOTIFY (bit 0)
            let flags = self.read_used_flags(mem)?;
            Ok((flags & 1) == 0)
        }
    }

    /// Kick the backend to process the queue
    ///
    /// This signals the kick eventfd. For efficiency, check needs_kick()
    /// first, or use kick_if_needed().
    pub fn kick(&self) -> io::Result<()> {
        self.kick.write(1).map_err(io::Error::other)?;
        Ok(())
    }

    /// Kick the backend only if needed
    pub fn kick_if_needed<M: GuestMemory>(&self, mem: &M) -> io::Result<bool> {
        if self.needs_kick(mem)? {
            self.kick()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Enable callbacks (interrupts) from the device
    ///
    /// With event_idx: sets used_event to current last_used_idx
    /// so we get notified on the next completion.
    pub fn enable_cb<M: GuestMemory>(&mut self, mem: &M) -> io::Result<()> {
        if self.event_idx {
            self.write_used_event(mem, self.last_used_idx)?;
        }
        // Without event_idx, callbacks are always enabled
        // (controlled by device via used->flags)
        Ok(())
    }

    /// Disable callbacks (interrupts) from the device
    ///
    /// With event_idx: sets used_event far in the future
    pub fn disable_cb<M: GuestMemory>(&mut self, mem: &M) -> io::Result<()> {
        if self.event_idx {
            // Set used_event far ahead so we won't get notified
            self.write_used_event(mem, self.last_used_idx.wrapping_sub(1))?;
        }
        Ok(())
    }

    /// Check if there are used buffers available
    pub fn has_used<M: GuestMemory>(&self, mem: &M) -> io::Result<bool> {
        let used_idx: u16 = mem
            .read_obj(GuestAddress(self.used_addr + 2))
            .map_err(|e| io::Error::other(format!("read used idx: {:?}", e)))?;
        Ok(used_idx != self.last_used_idx)
    }
}

/// vring_need_event - check if notification is needed
///
/// From virtio_ring.h:
/// ```c
/// static inline int vring_need_event(__u16 event_idx, __u16 new_idx, __u16 old)
/// {
///     return (__u16)(new_idx - event_idx - 1) < (__u16)(new_idx - old);
/// }
/// ```
///
/// Returns true if event_idx is between old and new (exclusive of old, inclusive of new).
fn vring_need_event(event_idx: u16, new_idx: u16, old: u16) -> bool {
    new_idx.wrapping_sub(event_idx).wrapping_sub(1) < new_idx.wrapping_sub(old)
}

/// Wait for a call eventfd to be signaled (with timeout)
pub fn wait_for_call(call: &EventFd, timeout_ms: u64) -> io::Result<bool> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::unix::io::BorrowedFd;

    let borrowed = unsafe { BorrowedFd::borrow_raw(call.as_raw_fd()) };
    let poll_fd = PollFd::new(borrowed, PollFlags::POLLIN);
    let result = poll(&mut [poll_fd], PollTimeout::try_from(timeout_ms).unwrap())
        .map_err(io::Error::other)?;

    if result > 0 {
        let _ = call.read();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vm_memory::{GuestAddress, GuestMemoryMmap, GuestRegionMmap, MmapRegion};

    fn create_test_memory(size: usize) -> GuestMemoryMmap {
        let mmap_region = MmapRegion::new(size).expect("mmap region");
        let region = GuestRegionMmap::new(mmap_region, GuestAddress(0)).expect("guest region");
        GuestMemoryMmap::from_regions(vec![region]).expect("mmap")
    }

    #[test]
    fn test_free_list_init() {
        let mem = create_test_memory(64 * 1024);
        let mut vq = VirtqueueDriver::new(16, 0, false).expect("create vq");
        vq.init(&mem).expect("init");

        assert_eq!(vq.num_free(), 16);
        assert_eq!(vq.free_head, 0);

        // Check free list linkage
        for i in 0..15u16 {
            let next = vq.read_desc_next(&mem, i).expect("read next");
            assert_eq!(next, i + 1);
        }
    }

    #[test]
    fn test_add_single_buffer() {
        let mem = create_test_memory(64 * 1024);
        let mut vq = VirtqueueDriver::new(16, 0, false).expect("create vq");
        vq.init(&mem).expect("init");

        let head = vq
            .add_outbuf(&mem, &[(0x1000, 256)], 42)
            .expect("add outbuf");

        assert_eq!(head, 0);
        assert_eq!(vq.num_free(), 15);
        assert_eq!(vq.avail_idx_shadow, 1);

        // Check descriptor
        let addr: u64 = mem.read_obj(GuestAddress(0)).expect("read addr");
        let len: u32 = mem.read_obj(GuestAddress(8)).expect("read len");
        let flags: u16 = mem.read_obj(GuestAddress(12)).expect("read flags");

        assert_eq!(addr, 0x1000);
        assert_eq!(len, 256);
        assert_eq!(flags, 0); // No NEXT or WRITE flags

        // Check avail ring
        let avail_idx: u16 = mem
            .read_obj(GuestAddress(vq.avail_addr + 2))
            .expect("avail idx");
        let avail_entry: u16 = mem
            .read_obj(GuestAddress(vq.avail_addr + 4))
            .expect("avail entry");

        assert_eq!(avail_idx, 1);
        assert_eq!(avail_entry, 0);
    }

    #[test]
    fn test_add_chained_buffer() {
        let mem = create_test_memory(64 * 1024);
        let mut vq = VirtqueueDriver::new(16, 0, false).expect("create vq");
        vq.init(&mem).expect("init");

        // Add a chain of 3 buffers (like virtio-net header + data)
        let buffers = [(0x1000, 12), (0x2000, 1500), (0x3000, 100)];
        let head = vq.add_outbuf(&mem, &buffers, 42).expect("add outbuf");

        assert_eq!(head, 0);
        assert_eq!(vq.num_free(), 13); // Used 3 descriptors

        // Check first descriptor has NEXT flag
        let flags0: u16 = mem.read_obj(GuestAddress(12)).expect("flags0");
        assert_eq!(flags0, VIRTQ_DESC_F_NEXT);

        // Check second descriptor has NEXT flag
        let flags1: u16 = mem.read_obj(GuestAddress(16 + 12)).expect("flags1");
        assert_eq!(flags1, VIRTQ_DESC_F_NEXT);

        // Check third descriptor has no NEXT flag
        let flags2: u16 = mem.read_obj(GuestAddress(32 + 12)).expect("flags2");
        assert_eq!(flags2, 0);
    }

    #[test]
    fn test_add_inbuf_write_flag() {
        let mem = create_test_memory(64 * 1024);
        let mut vq = VirtqueueDriver::new(16, 0, false).expect("create vq");
        vq.init(&mem).expect("init");

        vq.add_inbuf(&mem, 0x1000, 4096, 42).expect("add inbuf");

        // Check WRITE flag is set
        let flags: u16 = mem.read_obj(GuestAddress(12)).expect("flags");
        assert_eq!(flags, VIRTQ_DESC_F_WRITE);
    }

    #[test]
    fn test_vring_need_event() {
        // Event at 10, new at 11, old at 10 -> need event (11 >= 10+1)
        assert!(vring_need_event(10, 11, 10));

        // Event at 10, new at 10, old at 9 -> no event (10 < 10+1)
        assert!(!vring_need_event(10, 10, 9));

        // Wraparound case
        assert!(vring_need_event(65535, 0, 65535));
    }
}
