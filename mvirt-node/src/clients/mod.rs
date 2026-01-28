//! Clients for local mvirt daemons.
//!
//! These clients connect to local services on the hypervisor:
//! - mvirt-vmm: VM management (create, start, stop)
//! - mvirt-zfs: Storage management (volumes, snapshots, templates)
//! - mvirt-net: Network management (networks, NICs, security groups)

pub mod net;
pub mod vmm;
pub mod zfs;

pub use net::NetClient;
pub use vmm::VmmClient;
pub use zfs::ZfsClient;
