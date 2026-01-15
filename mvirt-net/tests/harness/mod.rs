//! Test harness for vhost-user integration tests
//!
//! Provides utilities for simulating the VM side of vhost-user connections.

#![allow(dead_code)]

pub mod backend;
pub mod client;
pub mod memory;
pub mod packets;
pub mod virtio;

pub use backend::{GATEWAY_IP, TestBackend};
pub use client::VhostTestClient;
pub use mvirt_net::dataplane::GATEWAY_MAC;
