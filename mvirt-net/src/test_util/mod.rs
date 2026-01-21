//! Test utilities for vhost-user frontend simulation
//!
//! This module provides a realistic virtio-net driver implementation
//! for integration tests, based on the Linux kernel driver.

pub mod frontend_device;
pub mod virtqueue;

pub use frontend_device::VhostUserFrontendDevice;
pub use virtqueue::{UsedBuffer, VirtqueueDriver};
