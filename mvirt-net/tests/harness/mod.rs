//! Test harness for vhost-user integration tests
//!
//! Provides utilities for simulating the VM side of vhost-user connections.

#![allow(dead_code)]
// Re-exports are used by integration tests in separate test binaries
#![allow(unused_imports)]

pub mod backend;
pub mod client;
pub mod memory;
pub mod packets;
pub mod virtio;

pub use backend::{GATEWAY_IP, RoutingNicConfig, RoutingTestBackend, TestBackend};
pub use client::VhostTestClient;
pub use mvirt_net::dataplane::GATEWAY_MAC;
