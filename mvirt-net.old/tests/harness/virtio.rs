//! Virtio queue management for tests
//!
//! Implements the frontend side of virtio queues for sending/receiving packets.

use std::sync::atomic::{AtomicU16, Ordering};

use vm_memory::{Address, ByteValued, Bytes, GuestAddress, GuestMemoryMmap, Le16, Le32, Le64};

/// Virtio queue size
pub const QUEUE_SIZE: u16 = 256;

/// Virtio descriptor flags
pub const VRING_DESC_F_NEXT: u16 = 1;
pub const VRING_DESC_F_WRITE: u16 = 2;

/// Virtio net header size
pub const VIRTIO_NET_HDR_SIZE: usize = 12;

/// Virtio descriptor
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VringDesc {
    pub addr: Le64,
    pub len: Le32,
    pub flags: Le16,
    pub next: Le16,
}

// SAFETY: VringDesc is POD
unsafe impl ByteValued for VringDesc {}

/// Virtio available ring
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VringAvail {
    pub flags: Le16,
    pub idx: Le16,
    // ring[QUEUE_SIZE] follows
}

// SAFETY: VringAvail is POD
unsafe impl ByteValued for VringAvail {}

/// Virtio used ring element
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VringUsedElem {
    pub id: Le32,
    pub len: Le32,
}

// SAFETY: VringUsedElem is POD
unsafe impl ByteValued for VringUsedElem {}

/// Virtio used ring
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VringUsed {
    pub flags: Le16,
    pub idx: Le16,
    // ring[QUEUE_SIZE] follows
}

// SAFETY: VringUsed is POD
unsafe impl ByteValued for VringUsed {}

/// Virtio net header (without mergeable buffers)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: Le16,
    pub gso_size: Le16,
    pub csum_start: Le16,
    pub csum_offset: Le16,
    pub num_buffers: Le16,
}

// SAFETY: VirtioNetHdr is POD
unsafe impl ByteValued for VirtioNetHdr {}

/// Manages a single virtio queue from the frontend perspective
pub struct VirtioQueue {
    /// Queue index (0 = RX, 1 = TX)
    pub queue_idx: u16,
    /// Descriptor table guest address
    pub desc_table: GuestAddress,
    /// Available ring guest address
    pub avail_ring: GuestAddress,
    /// Used ring guest address
    pub used_ring: GuestAddress,
    /// Next descriptor index to use
    next_desc: AtomicU16,
    /// Next available ring index
    next_avail: AtomicU16,
    /// Last seen used ring index
    last_used: AtomicU16,
}

impl VirtioQueue {
    /// Create a new virtio queue
    pub fn new(
        queue_idx: u16,
        desc_table: GuestAddress,
        avail_ring: GuestAddress,
        used_ring: GuestAddress,
    ) -> Self {
        Self {
            queue_idx,
            desc_table,
            avail_ring,
            used_ring,
            next_desc: AtomicU16::new(0),
            next_avail: AtomicU16::new(0),
            last_used: AtomicU16::new(0),
        }
    }

    /// Initialize the queue structures in guest memory
    pub fn init(&self, mem: &GuestMemoryMmap) -> std::io::Result<()> {
        // Zero out descriptor table
        let zeros = vec![0u8; std::mem::size_of::<VringDesc>() * QUEUE_SIZE as usize];
        mem.write(&zeros, self.desc_table)
            .map_err(|e| std::io::Error::other(format!("desc table init: {e}")))?;

        // Zero out avail ring (includes used_event field for EVENT_IDX)
        // Layout: flags(2) + idx(2) + ring[queue_size](2*queue_size) + used_event(2)
        let avail_size = std::mem::size_of::<VringAvail>()
            + std::mem::size_of::<u16>() * QUEUE_SIZE as usize
            + 2; // used_event for EVENT_IDX
        let zeros = vec![0u8; avail_size];
        mem.write(&zeros, self.avail_ring)
            .map_err(|e| std::io::Error::other(format!("avail ring init: {e}")))?;

        // Zero out used ring (includes avail_event field for EVENT_IDX)
        // Layout: flags(2) + idx(2) + ring[queue_size](8*queue_size) + avail_event(2)
        let used_size = std::mem::size_of::<VringUsed>()
            + std::mem::size_of::<VringUsedElem>() * QUEUE_SIZE as usize
            + 2; // avail_event for EVENT_IDX
        let zeros = vec![0u8; used_size];
        mem.write(&zeros, self.used_ring)
            .map_err(|e| std::io::Error::other(format!("used ring init: {e}")))?;

        Ok(())
    }

    /// Set the used_event field (driver tells device when to interrupt)
    /// With EVENT_IDX, device should only interrupt when used_idx reaches this value
    pub fn set_used_event(&self, mem: &GuestMemoryMmap, event: u16) -> std::io::Result<()> {
        // used_event is at the end of avail ring
        let offset = self.avail_ring.raw_value()
            + std::mem::size_of::<VringAvail>() as u64
            + (QUEUE_SIZE as u64 * 2); // after ring array
        mem.write(&event.to_le_bytes(), GuestAddress(offset))
            .map_err(|e| std::io::Error::other(format!("write used_event: {e}")))?;
        Ok(())
    }

    /// Get the avail_event field (device tells driver when to kick)
    /// With EVENT_IDX, driver should only kick when avail_idx reaches this value
    #[allow(dead_code)]
    pub fn get_avail_event(&self, mem: &GuestMemoryMmap) -> std::io::Result<u16> {
        // avail_event is at the end of used ring
        let offset = self.used_ring.raw_value()
            + std::mem::size_of::<VringUsed>() as u64
            + (QUEUE_SIZE as u64 * std::mem::size_of::<VringUsedElem>() as u64);
        let mut bytes = [0u8; 2];
        mem.read(&mut bytes, GuestAddress(offset))
            .map_err(|e| std::io::Error::other(format!("read avail_event: {e}")))?;
        Ok(u16::from_le_bytes(bytes))
    }

    /// Add a TX buffer (readable by device) with ethernet frame data
    pub fn add_tx_buffer(
        &self,
        mem: &GuestMemoryMmap,
        data_addr: GuestAddress,
        data: &[u8],
    ) -> std::io::Result<u16> {
        // Write virtio-net header + data
        let hdr = VirtioNetHdr::default();
        let mut buffer = Vec::with_capacity(VIRTIO_NET_HDR_SIZE + data.len());
        buffer.extend_from_slice(hdr.as_slice());
        buffer.extend_from_slice(data);

        mem.write(&buffer, data_addr)
            .map_err(|e| std::io::Error::other(format!("write data: {e}")))?;

        // Get next descriptor index
        let desc_idx = self.next_desc.fetch_add(1, Ordering::SeqCst) % QUEUE_SIZE;

        // Write descriptor
        let desc = VringDesc {
            addr: Le64::from(data_addr.raw_value()),
            len: Le32::from(buffer.len() as u32),
            flags: Le16::from(0), // No WRITE flag = readable by device
            next: Le16::from(0),
        };

        let desc_offset = self.desc_table.raw_value()
            + (desc_idx as u64) * std::mem::size_of::<VringDesc>() as u64;
        mem.write(desc.as_slice(), GuestAddress(desc_offset))
            .map_err(|e| std::io::Error::other(format!("write desc: {e}")))?;

        // Add to available ring
        let avail_idx = self.next_avail.fetch_add(1, Ordering::SeqCst);
        let ring_offset = self.avail_ring.raw_value()
            + std::mem::size_of::<VringAvail>() as u64
            + ((avail_idx % QUEUE_SIZE) as u64) * 2;
        mem.write(&desc_idx.to_le_bytes(), GuestAddress(ring_offset))
            .map_err(|e| std::io::Error::other(format!("write avail ring: {e}")))?;

        // Update avail idx
        let idx_offset = self.avail_ring.raw_value() + 2; // offset of idx field
        mem.write(
            &(avail_idx.wrapping_add(1)).to_le_bytes(),
            GuestAddress(idx_offset),
        )
        .map_err(|e| std::io::Error::other(format!("write avail idx: {e}")))?;

        Ok(desc_idx)
    }

    /// Add an RX buffer (writable by device)
    pub fn add_rx_buffer(
        &self,
        mem: &GuestMemoryMmap,
        data_addr: GuestAddress,
        size: usize,
    ) -> std::io::Result<u16> {
        // Get next descriptor index
        let desc_idx = self.next_desc.fetch_add(1, Ordering::SeqCst) % QUEUE_SIZE;

        // Write descriptor with WRITE flag
        let desc = VringDesc {
            addr: Le64::from(data_addr.raw_value()),
            len: Le32::from(size as u32),
            flags: Le16::from(VRING_DESC_F_WRITE),
            next: Le16::from(0),
        };

        let desc_offset = self.desc_table.raw_value()
            + (desc_idx as u64) * std::mem::size_of::<VringDesc>() as u64;
        mem.write(desc.as_slice(), GuestAddress(desc_offset))
            .map_err(|e| std::io::Error::other(format!("write desc: {e}")))?;

        // Add to available ring
        let avail_idx = self.next_avail.fetch_add(1, Ordering::SeqCst);
        let ring_offset = self.avail_ring.raw_value()
            + std::mem::size_of::<VringAvail>() as u64
            + ((avail_idx % QUEUE_SIZE) as u64) * 2;
        mem.write(&desc_idx.to_le_bytes(), GuestAddress(ring_offset))
            .map_err(|e| std::io::Error::other(format!("write avail ring: {e}")))?;

        // Update avail idx
        let idx_offset = self.avail_ring.raw_value() + 2;
        mem.write(
            &(avail_idx.wrapping_add(1)).to_le_bytes(),
            GuestAddress(idx_offset),
        )
        .map_err(|e| std::io::Error::other(format!("write avail idx: {e}")))?;

        Ok(desc_idx)
    }

    /// Check if there are used descriptors available
    pub fn has_used(&self, mem: &GuestMemoryMmap) -> std::io::Result<bool> {
        let idx_offset = self.used_ring.raw_value() + 2;
        let mut idx_bytes = [0u8; 2];
        mem.read(&mut idx_bytes, GuestAddress(idx_offset))
            .map_err(|e| std::io::Error::other(format!("read used idx: {e}")))?;
        let used_idx = u16::from_le_bytes(idx_bytes);
        Ok(used_idx != self.last_used.load(Ordering::SeqCst))
    }

    /// Pop a used descriptor
    pub fn pop_used(&self, mem: &GuestMemoryMmap) -> std::io::Result<Option<(u16, u32)>> {
        let idx_offset = self.used_ring.raw_value() + 2;
        let mut idx_bytes = [0u8; 2];
        mem.read(&mut idx_bytes, GuestAddress(idx_offset))
            .map_err(|e| std::io::Error::other(format!("read used idx: {e}")))?;
        let used_idx = u16::from_le_bytes(idx_bytes);

        let last = self.last_used.load(Ordering::SeqCst);
        if used_idx == last {
            return Ok(None);
        }

        // Read used element
        let elem_offset = self.used_ring.raw_value()
            + std::mem::size_of::<VringUsed>() as u64
            + ((last % QUEUE_SIZE) as u64) * std::mem::size_of::<VringUsedElem>() as u64;

        let mut elem = VringUsedElem::default();
        mem.read(elem.as_mut_slice(), GuestAddress(elem_offset))
            .map_err(|e| std::io::Error::other(format!("read used elem: {e}")))?;

        self.last_used.fetch_add(1, Ordering::SeqCst);

        Ok(Some((u32::from(elem.id) as u16, u32::from(elem.len))))
    }

    /// Read data from an RX buffer after it's been used
    pub fn read_rx_data(
        &self,
        mem: &GuestMemoryMmap,
        data_addr: GuestAddress,
        len: u32,
    ) -> std::io::Result<Vec<u8>> {
        let mut buffer = vec![0u8; len as usize];
        mem.read(&mut buffer, data_addr)
            .map_err(|e| std::io::Error::other(format!("read rx data: {e}")))?;

        // Skip virtio-net header
        if buffer.len() > VIRTIO_NET_HDR_SIZE {
            Ok(buffer[VIRTIO_NET_HDR_SIZE..].to_vec())
        } else {
            Ok(Vec::new())
        }
    }
}
