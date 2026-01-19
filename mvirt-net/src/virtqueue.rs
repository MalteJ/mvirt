//! Virtqueue abstraction for guest memory buffer management

use crate::hugepage::HugePagePool;
use nix::libc;
use std::collections::VecDeque;
use std::fs::File;
use std::os::unix::io::{AsRawFd, RawFd};

/// A buffer descriptor from the virtqueue
#[derive(Debug, Clone, Copy)]
pub struct VirtqueueBuffer {
    /// Pointer to the buffer in guest RAM
    pub ptr: *mut u8,
    /// Length of the buffer
    pub len: u32,
    /// Index for io_uring fixed buffer registration
    pub buf_index: u16,
}

/// A chain of descriptors from the virtqueue
#[derive(Debug)]
pub struct DescriptorChain {
    /// Unique ID for this chain (used to return it to the guest)
    pub chain_id: u64,
    /// The buffer for this chain
    pub buffer: VirtqueueBuffer,
}

/// Trait for RX virtqueue (receiving packets)
#[allow(dead_code)]
pub trait RxVirtqueue: Send {
    /// Get the file descriptor for reading
    fn fd(&self) -> RawFd;

    /// Pop an available descriptor chain for receiving.
    fn pop_available(&mut self) -> Option<DescriptorChain>;

    /// Return a used descriptor chain after receiving.
    fn push_used(&mut self, chain_id: u64, len: u32);

    /// Get all RX buffers for io_uring registration (unused with regular Read).
    fn get_iovecs_for_registration(&self) -> Vec<libc::iovec>;

    /// Signal that RX buffers are available.
    fn notify(&self);
}

/// Trait for TX virtqueue (sending packets)
#[allow(dead_code)]
pub trait TxVirtqueue: Send {
    /// Get the file descriptor for writing
    fn fd(&self) -> RawFd;

    /// Pop an available descriptor chain for sending.
    fn pop_available(&mut self) -> Option<DescriptorChain>;

    /// Return a used descriptor chain after sending.
    fn push_used(&mut self, chain_id: u64);

    /// Get all TX buffers for io_uring registration (unused with regular Write).
    fn get_iovecs_for_registration(&self) -> Vec<libc::iovec>;

    /// Signal that TX buffers have been consumed.
    fn notify(&self);
}

/// Simple RX/TX virtqueue pair backed by HugePagePool
pub struct SimpleRxTxQueues {
    rx: SimpleRxQueue,
    tx: SimpleTxQueue,
}

impl SimpleRxTxQueues {
    pub fn new(
        file: File,
        buffers: HugePagePool,
        buffer_size: usize,
        rx_count: usize,
        tx_count: usize,
    ) -> Self {
        let total = rx_count + tx_count;
        assert!(
            total * buffer_size <= buffers.size(),
            "Buffer pool too small"
        );

        let fd = file.as_raw_fd();
        // Keep file alive
        std::mem::forget(file);

        let base_ptr = buffers.ptr();

        let rx = SimpleRxQueue {
            fd,
            base_ptr,
            buffer_size,
            buffer_count: rx_count,
            available: (0..rx_count as u64).collect(),
        };

        let tx = SimpleTxQueue {
            fd,
            base_ptr,
            buffer_size,
            rx_count, // TX buffers start after RX buffers
            buffer_count: tx_count,
            available: (rx_count as u64..(rx_count + tx_count) as u64).collect(),
        };

        // Keep the pool alive
        std::mem::forget(buffers);

        SimpleRxTxQueues { rx, tx }
    }

    pub fn split(self) -> (SimpleRxQueue, SimpleTxQueue) {
        (self.rx, self.tx)
    }
}

/// Simple RX queue implementation
pub struct SimpleRxQueue {
    fd: RawFd,
    base_ptr: *mut u8,
    buffer_size: usize,
    #[allow(dead_code)]
    buffer_count: usize,
    available: VecDeque<u64>,
}

unsafe impl Send for SimpleRxQueue {}

impl RxVirtqueue for SimpleRxQueue {
    fn fd(&self) -> RawFd {
        self.fd
    }

    fn pop_available(&mut self) -> Option<DescriptorChain> {
        let chain_id = self.available.pop_front()?;
        let idx = chain_id as usize;
        let ptr = unsafe { self.base_ptr.add(idx * self.buffer_size) };

        Some(DescriptorChain {
            chain_id,
            buffer: VirtqueueBuffer {
                ptr,
                len: self.buffer_size as u32,
                buf_index: idx as u16,
            },
        })
    }

    fn push_used(&mut self, chain_id: u64, _len: u32) {
        self.available.push_back(chain_id);
    }

    fn get_iovecs_for_registration(&self) -> Vec<libc::iovec> {
        (0..self.buffer_count)
            .map(|idx| libc::iovec {
                iov_base: unsafe { self.base_ptr.add(idx * self.buffer_size) } as *mut libc::c_void,
                iov_len: self.buffer_size,
            })
            .collect()
    }

    fn notify(&self) {}
}

/// Simple TX queue implementation
pub struct SimpleTxQueue {
    fd: RawFd,
    base_ptr: *mut u8,
    buffer_size: usize,
    #[allow(dead_code)]
    rx_count: usize, // Offset for TX buffer indices
    #[allow(dead_code)]
    buffer_count: usize,
    available: VecDeque<u64>,
}

unsafe impl Send for SimpleTxQueue {}

impl TxVirtqueue for SimpleTxQueue {
    fn fd(&self) -> RawFd {
        self.fd
    }

    fn pop_available(&mut self) -> Option<DescriptorChain> {
        let chain_id = self.available.pop_front()?;
        let idx = chain_id as usize;
        let ptr = unsafe { self.base_ptr.add(idx * self.buffer_size) };

        Some(DescriptorChain {
            chain_id,
            buffer: VirtqueueBuffer {
                ptr,
                len: self.buffer_size as u32,
                buf_index: idx as u16,
            },
        })
    }

    fn push_used(&mut self, chain_id: u64) {
        self.available.push_back(chain_id);
    }

    fn get_iovecs_for_registration(&self) -> Vec<libc::iovec> {
        (0..self.buffer_count)
            .map(|idx| {
                let global_idx = self.rx_count + idx;
                libc::iovec {
                    iov_base: unsafe { self.base_ptr.add(global_idx * self.buffer_size) }
                        as *mut libc::c_void,
                    iov_len: self.buffer_size,
                }
            })
            .collect()
    }

    fn notify(&self) {}
}

/// A pending TX packet ready to be sent
#[derive(Debug)]
pub struct TxPacket {
    /// The descriptor chain holding the packet data
    pub chain: DescriptorChain,
    /// Actual length of data to send (including virtio_net_hdr)
    pub len: u32,
}
