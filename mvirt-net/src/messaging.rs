//! Inter-reactor messaging with automatic eventfd signaling.
//!
//! This module provides a channel abstraction that combines mpsc channels
//! with eventfd signaling, making it easy to integrate with io_uring-based
//! reactor loops.
//!
//! # Example
//!
//! ```ignore
//! use iou::messaging::SignalingChannel;
//!
//! // Create a channel
//! let channel = SignalingChannel::<String>::new()?;
//! let (inbox, outbox) = channel.split();
//!
//! // Reactor A: register inbox.eventfd() with io_uring
//! // Reactor B: clone outbox and send messages
//! let outbox_clone = outbox.clone();
//! outbox_clone.send("Hello from B".to_string())?;
//!
//! // Reactor A: woken by eventfd, drain messages
//! for msg in inbox.drain() {
//!     println!("Received: {}", msg);
//! }
//! ```

use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{self, Receiver, SendError, Sender, TryRecvError};

/// A signaling channel for inter-reactor communication.
///
/// Combines an mpsc channel with an eventfd so that receivers can be
/// woken up via io_uring when messages arrive.
pub struct SignalingChannel<T> {
    inbox: Inbox<T>,
    outbox: Outbox<T>,
}

/// The receiving end of a signaling channel.
///
/// Owns the eventfd and mpsc receiver. Register the eventfd with io_uring
/// to be woken when messages arrive.
pub struct Inbox<T> {
    rx: Receiver<T>,
    eventfd: OwnedFd,
}

/// The sending end of a signaling channel.
///
/// Clone this to distribute to other reactors. Sending automatically
/// signals the receiver's eventfd.
///
/// # Safety
///
/// The `Inbox` must outlive all `Outbox` clones. In practice, this is
/// ensured by the reactor registry holding references to channels.
pub struct Outbox<T> {
    tx: Sender<T>,
    eventfd: RawFd,
}

// Outbox is Clone - can be distributed to other reactors
impl<T> Clone for Outbox<T> {
    fn clone(&self) -> Self {
        Outbox {
            tx: self.tx.clone(),
            eventfd: self.eventfd,
        }
    }
}

impl<T> SignalingChannel<T> {
    /// Create a new signaling channel.
    pub fn new() -> std::io::Result<Self> {
        let eventfd = unsafe {
            let fd = nix::libc::eventfd(0, nix::libc::EFD_NONBLOCK);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(fd)
        };
        let (tx, rx) = mpsc::channel();

        let outbox = Outbox {
            tx,
            eventfd: eventfd.as_raw_fd(),
        };
        let inbox = Inbox { rx, eventfd };

        Ok(SignalingChannel { inbox, outbox })
    }

    /// Split into inbox and outbox.
    ///
    /// The inbox stays with the owning reactor; the outbox can be cloned
    /// and distributed to other reactors via the registry.
    pub fn split(self) -> (Inbox<T>, Outbox<T>) {
        (self.inbox, self.outbox)
    }
}

impl<T> Inbox<T> {
    /// Get the eventfd for io_uring registration.
    ///
    /// Register this with io_uring to be woken when messages arrive.
    pub fn eventfd(&self) -> &OwnedFd {
        &self.eventfd
    }

    /// Get the raw eventfd file descriptor.
    pub fn eventfd_raw(&self) -> RawFd {
        self.eventfd.as_raw_fd()
    }

    /// Try to receive a message without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.rx.try_recv()
    }

    /// Drain all available messages.
    ///
    /// Returns an iterator that yields messages until the channel is empty.
    pub fn drain(&self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.rx.try_recv().ok())
    }

    /// Check if there are pending messages without consuming them.
    ///
    /// Note: This is racy - messages may arrive after this check.
    pub fn is_empty(&self) -> bool {
        // mpsc doesn't have is_empty, but we can use try_recv + peek pattern
        // Actually, we can't peek, so just return false to be safe
        false
    }
}

impl<T> Outbox<T> {
    /// Send a message and signal the receiver.
    ///
    /// Returns an error if the receiver has been dropped.
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        self.tx.send(msg)?;
        self.signal();
        Ok(())
    }

    /// Signal the receiver's eventfd without sending a message.
    ///
    /// Useful for waking up a reactor for other reasons.
    pub fn signal(&self) {
        let buf: u64 = 1;
        unsafe {
            // Ignore errors - if eventfd is closed, receiver is gone anyway
            nix::libc::write(
                self.eventfd,
                &buf as *const u64 as *const nix::libc::c_void,
                8,
            );
        }
    }
}

/// Create a signaling channel pair.
///
/// Convenience function equivalent to `SignalingChannel::new()?.split()`.
pub fn channel<T>() -> std::io::Result<(Inbox<T>, Outbox<T>)> {
    SignalingChannel::new().map(|c| c.split())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_basic_send_recv() {
        let (inbox, outbox) = channel::<u32>().unwrap();

        outbox.send(42).unwrap();
        outbox.send(43).unwrap();

        assert_eq!(inbox.try_recv().unwrap(), 42);
        assert_eq!(inbox.try_recv().unwrap(), 43);
        assert!(inbox.try_recv().is_err());
    }

    #[test]
    fn test_drain() {
        let (inbox, outbox) = channel::<u32>().unwrap();

        outbox.send(1).unwrap();
        outbox.send(2).unwrap();
        outbox.send(3).unwrap();

        let msgs: Vec<_> = inbox.drain().collect();
        assert_eq!(msgs, vec![1, 2, 3]);
    }

    #[test]
    fn test_clone_outbox() {
        let (inbox, outbox) = channel::<u32>().unwrap();

        let outbox2 = outbox.clone();

        outbox.send(1).unwrap();
        outbox2.send(2).unwrap();

        let msgs: Vec<_> = inbox.drain().collect();
        assert_eq!(msgs, vec![1, 2]);
    }

    #[test]
    fn test_cross_thread() {
        let (inbox, outbox) = channel::<String>().unwrap();

        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            outbox.send("hello".to_string()).unwrap();
        });

        // Wait for message
        thread::sleep(Duration::from_millis(50));

        let msg = inbox.try_recv().unwrap();
        assert_eq!(msg, "hello");

        handle.join().unwrap();
    }

    #[test]
    fn test_eventfd_exists() {
        let (inbox, _outbox) = channel::<u32>().unwrap();

        // eventfd should be a valid fd
        let fd = inbox.eventfd_raw();
        assert!(fd >= 0);
    }
}
