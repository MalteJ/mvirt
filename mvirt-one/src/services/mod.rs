//! Service modules for one.
//!
//! Following the Actor pattern from FeOS:
//! - API handlers receive requests and send commands to dispatchers
//! - Dispatchers consume commands and coordinate workers
//! - Workers perform the actual operations

pub mod image;
pub mod pod;
pub mod task;
