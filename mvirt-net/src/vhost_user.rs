//! vhost-user net backend implementation using rust-vmm libraries

#![allow(dead_code)]

use std::io;
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};
use vhost::vhost_user::Listener;
use vhost::vhost_user::message::{VhostUserProtocolFeatures, VhostUserVirtioFeatures};
use vhost_user_backend::{VhostUserBackendMut, VhostUserDaemon, VringMutex, VringT};
use vm_memory::{ByteValued, GuestMemoryAtomic, GuestMemoryMmap, Le16};
use vmm_sys_util::epoll::EventSet;
use vmm_sys_util::event::{
    EventConsumer, EventFlag, EventNotifier, new_event_consumer_and_notifier,
};

/// Virtio feature flags
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_F_RING_EVENT_IDX: u64 = 1 << 29;

// Virtio-net feature flags
const VIRTIO_NET_F_CSUM: u64 = 1 << 0;
const VIRTIO_NET_F_GUEST_CSUM: u64 = 1 << 1;
const VIRTIO_NET_F_GUEST_TSO4: u64 = 1 << 7;
const VIRTIO_NET_F_GUEST_TSO6: u64 = 1 << 8;
const VIRTIO_NET_F_GUEST_ECN: u64 = 1 << 9;
const VIRTIO_NET_F_GUEST_UFO: u64 = 1 << 10;
const VIRTIO_NET_F_HOST_TSO4: u64 = 1 << 11;
const VIRTIO_NET_F_HOST_TSO6: u64 = 1 << 12;
const VIRTIO_NET_F_HOST_ECN: u64 = 1 << 13;
const VIRTIO_NET_F_HOST_UFO: u64 = 1 << 14;
const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15;

/// Number of virtqueues (RX, TX)
const NUM_QUEUES: usize = 2;
const QUEUE_SIZE: usize = 256;

/// Queue indices
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

/// Virtio net config (MAC address + status)
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct VirtioNetConfig {
    pub mac: [u8; 6],
    pub status: Le16,
}

// SAFETY: VirtioNetConfig contains only plain data types
unsafe impl ByteValued for VirtioNetConfig {}

/// Type alias for guest memory
pub type GuestMemoryMmapAtomic = GuestMemoryAtomic<GuestMemoryMmap>;
/// Type alias for vring with mutex
pub type VringType = VringMutex<GuestMemoryMmapAtomic>;

/// Handshake data sent from VhostUserDaemon to Reactor (once)
pub struct VhostHandshake {
    pub mem: GuestMemoryMmapAtomic,
    pub vrings: Vec<VringType>,
}

/// The vhost-user net backend
pub struct VhostUserNetBackend {
    event_idx: bool,
    mem: Option<GuestMemoryMmapAtomic>,
    config: VirtioNetConfig,
    exit_event: Option<(EventConsumer, EventNotifier)>,
    /// One-shot handshake channel to reactor
    handshake_tx: Option<SyncSender<VhostHandshake>>,
    /// Fd to signal reactor for queue processing
    reactor_notify: Option<OwnedFd>,
    /// Whether handshake has been completed
    handshake_done: bool,
    /// Store vrings for set_event_idx propagation
    vrings: Option<Vec<VringType>>,
}

impl VhostUserNetBackend {
    pub fn new(
        mac: [u8; 6],
        handshake_tx: Option<SyncSender<VhostHandshake>>,
        reactor_notify: Option<OwnedFd>,
    ) -> io::Result<Self> {
        let exit_event = new_event_consumer_and_notifier(EventFlag::CLOEXEC).ok();

        Ok(VhostUserNetBackend {
            event_idx: false,
            mem: None,
            config: VirtioNetConfig {
                mac,
                status: Le16::default(),
            },
            exit_event,
            handshake_tx,
            reactor_notify,
            handshake_done: false,
            vrings: None,
        })
    }

    /// Check if handshake should be performed and send vrings+memory to reactor (once)
    fn check_handshake(&mut self, vrings: &[VringType]) {
        // Only do handshake once, and only if we have the channel and memory
        if self.handshake_done {
            return;
        }

        let Some(ref handshake_tx) = self.handshake_tx else {
            return;
        };

        let Some(ref mem) = self.mem else {
            return;
        };

        // Clone vrings for the reactor
        let vrings_clone: Vec<VringType> = vrings.to_vec();

        // Store vrings for later set_event_idx calls
        self.vrings = Some(vrings_clone.clone());

        // Apply event_idx to vrings (may have been set before handshake)
        for vring in &vrings_clone {
            vring.set_queue_event_idx(self.event_idx);
        }

        let handshake = VhostHandshake {
            mem: mem.clone(),
            vrings: vrings_clone,
        };

        match handshake_tx.try_send(handshake) {
            Ok(()) => {
                info!("Handshake sent to reactor");
                self.handshake_done = true;
            }
            Err(e) => {
                warn!(?e, "Failed to send handshake to reactor");
            }
        }
    }

    /// Signal the reactor to process vhost queues
    fn signal_reactor(&self) {
        if let Some(ref notify) = self.reactor_notify {
            let buf: u64 = 1;
            unsafe {
                nix::libc::write(
                    notify.as_raw_fd(),
                    &buf as *const u64 as *const nix::libc::c_void,
                    8,
                );
            }
            debug!("Signaled reactor for vhost queue processing");
        }
    }
}

impl VhostUserBackendMut for VhostUserNetBackend {
    type Bitmap = ();
    type Vring = VringType;

    fn num_queues(&self) -> usize {
        NUM_QUEUES
    }

    fn max_queue_size(&self) -> usize {
        QUEUE_SIZE
    }

    fn features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_F_RING_EVENT_IDX
            | VIRTIO_NET_F_CSUM
            | VIRTIO_NET_F_GUEST_CSUM
            | VIRTIO_NET_F_GUEST_TSO4
            | VIRTIO_NET_F_GUEST_TSO6
            | VIRTIO_NET_F_GUEST_ECN
        //    | VIRTIO_NET_F_GUEST_UFO
            | VIRTIO_NET_F_HOST_TSO4
            | VIRTIO_NET_F_HOST_TSO6
            | VIRTIO_NET_F_HOST_ECN
        //    | VIRTIO_NET_F_HOST_UFO
            | VIRTIO_NET_F_MRG_RXBUF
            | VhostUserVirtioFeatures::PROTOCOL_FEATURES.bits()
    }

    fn protocol_features(&self) -> VhostUserProtocolFeatures {
        VhostUserProtocolFeatures::CONFIG
    }

    fn set_event_idx(&mut self, enabled: bool) {
        info!(enabled, "set_event_idx called");
        self.event_idx = enabled;

        // Propagate to vrings if available
        if let Some(ref vrings) = self.vrings {
            for vring in vrings {
                vring.set_queue_event_idx(enabled);
            }
            info!(
                enabled,
                num_vrings = vrings.len(),
                "Propagated event_idx to vrings"
            );
        }
    }

    fn update_memory(&mut self, mem: GuestMemoryMmapAtomic) -> io::Result<()> {
        info!("update_memory called");
        self.mem = Some(mem);
        Ok(())
    }

    fn handle_event(
        &mut self,
        device_event: u16,
        evset: EventSet,
        vrings: &[Self::Vring],
        _thread_id: usize,
    ) -> io::Result<()> {
        debug!(device_event, ?evset, "handle_event called");

        if evset != EventSet::IN {
            return Err(io::Error::other("unexpected event set"));
        }

        // Try to complete handshake (sends vrings+memory to reactor once)
        self.check_handshake(vrings);

        match device_event {
            RX_QUEUE => {
                // Guest made RX buffers available
                debug!("RX queue event - signaling reactor");
                self.signal_reactor();
            }
            TX_QUEUE => {
                // Guest has packets to send
                debug!("TX queue event - signaling reactor");
                self.signal_reactor();
            }
            _ => {
                warn!(device_event, "unexpected device event");
            }
        }

        Ok(())
    }

    fn get_config(&self, offset: u32, size: u32) -> Vec<u8> {
        let config_bytes = self.config.as_slice();
        let offset = offset as usize;
        let size = size as usize;

        if offset >= config_bytes.len() {
            return vec![];
        }

        let end = std::cmp::min(offset + size, config_bytes.len());
        config_bytes[offset..end].to_vec()
    }

    fn set_config(&mut self, _offset: u32, _buf: &[u8]) -> io::Result<()> {
        // Config is read-only for virtio-net
        Ok(())
    }

    fn exit_event(&self, _thread_index: usize) -> Option<(EventConsumer, EventNotifier)> {
        self.exit_event.as_ref().and_then(|(consumer, notifier)| {
            Some((consumer.try_clone().ok()?, notifier.try_clone().ok()?))
        })
    }

    fn queues_per_thread(&self) -> Vec<u64> {
        // All queues handled by one thread
        vec![0b11] // Bitmask: queue 0 and queue 1
    }
}

/// A vhost-user net device that can be started as a daemon
pub struct VhostUserNetDevice {
    socket_path: String,
    mac: [u8; 6],
    handshake_tx: Option<SyncSender<VhostHandshake>>,
    reactor_notify: Option<OwnedFd>,
}

impl VhostUserNetDevice {
    pub fn new(socket_path: impl Into<String>, mac: [u8; 6]) -> Self {
        VhostUserNetDevice {
            socket_path: socket_path.into(),
            mac,
            handshake_tx: None,
            reactor_notify: None,
        }
    }

    /// Create with handshake channel and reactor notification
    pub fn with_reactor(
        socket_path: impl Into<String>,
        mac: [u8; 6],
        handshake_tx: SyncSender<VhostHandshake>,
        reactor_notify: OwnedFd,
    ) -> Self {
        VhostUserNetDevice {
            socket_path: socket_path.into(),
            mac,
            handshake_tx: Some(handshake_tx),
            reactor_notify: Some(reactor_notify),
        }
    }

    /// Start the vhost-user daemon (blocking, reconnection loop)
    pub fn run(self) -> io::Result<()> {
        info!(socket = %self.socket_path, "Creating vhost-user-net backend with reconnection support");

        // Create listener once - it will be reused for reconnections
        info!(socket = %self.socket_path, "Creating listener");
        let mut listener = Listener::new(&self.socket_path, true)
            .map_err(|e| io::Error::other(format!("listener failed: {:?}", e)))?;

        loop {
            // Create fresh backend for each connection
            info!("Creating new backend for connection");
            let backend = Arc::new(RwLock::new(VhostUserNetBackend::new(
                self.mac,
                self.handshake_tx.clone(),
                self.reactor_notify
                    .as_ref()
                    .map(|fd| fd.try_clone())
                    .transpose()?,
            )?));

            info!("Creating VhostUserDaemon");
            let mut daemon = VhostUserDaemon::new(
                String::from("vhost-user-net"),
                backend,
                GuestMemoryAtomic::new(GuestMemoryMmap::new()),
            )
            .map_err(|e| io::Error::other(format!("daemon creation failed: {:?}", e)))?;

            info!("Waiting for VM connection...");
            daemon
                .start(&mut listener)
                .map_err(|e| io::Error::other(format!("daemon start failed: {:?}", e)))?;

            info!("VM connected, calling daemon.wait()");
            match daemon.wait() {
                Ok(()) => {
                    info!("VM disconnected cleanly, waiting for reconnection...");
                }
                Err(e) => {
                    warn!(
                        ?e,
                        "VM connection ended with error, waiting for reconnection..."
                    );
                }
            }
        }
    }
}
