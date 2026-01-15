//! Data plane: per-vNIC worker threads
//!
//! This module contains the vhost-user backend implementation, packet processing,
//! and routing between vNICs.
//!
//! Architecture:
//! - Each vNIC gets a dedicated worker thread (shared-nothing)
//! - Workers handle vhost-user protocol and packet processing
//! - Inter-vNIC routing via crossbeam channels

pub mod arp;
pub mod dhcpv4;
pub mod dhcpv6;
pub mod ndp;
pub mod packet;
pub mod router;
pub mod vhost;
pub mod worker;

pub use arp::ArpResponder;
pub use dhcpv4::Dhcpv4Server;
pub use dhcpv6::Dhcpv6Server;
pub use ndp::NdpResponder;
pub use packet::{GATEWAY_IPV4, GATEWAY_MAC};
pub use router::Router;
pub use vhost::VhostNetBackend;
pub use worker::{RoutedPacket, WorkerConfig, WorkerHandle, WorkerManager};
