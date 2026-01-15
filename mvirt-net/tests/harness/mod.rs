//! Test harness for vhost-user integration tests
//!
//! Provides utilities for simulating the VM side of vhost-user connections.

#![allow(dead_code)]

pub mod backend;
pub mod client;
pub mod memory;
pub mod packets;
pub mod virtio;

pub use backend::{TestBackend, GATEWAY_IP, GATEWAY_MAC};
pub use client::VhostTestClient;
