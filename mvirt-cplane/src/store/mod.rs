//! DataStore abstraction for the control plane.
//!
//! This module provides a clean abstraction over the Raft-replicated state,
//! allowing handlers to work with domain objects instead of raw commands.
//!
//! # Architecture
//!
//! ```text
//! handlers.rs → Arc<dyn DataStore>
//!                     ↓
//!               store.list_networks()              // Reads (µs)
//!               store.create_network(req).await    // Writes (ms, via Raft)
//! ```
//!
//! Handlers only know about domain DTOs (Network, Nic), not Raft details.
//!
//! # Usage
//!
//! ```ignore
//! use mvirt_cp::store::{DataStore, RaftStore, CreateNetworkRequest};
//!
//! // Create a RaftStore wrapping the RaftNode
//! let (event_tx, _) = broadcast::channel(256);
//! let store = Arc::new(RaftStore::new(node, event_tx, node_id));
//!
//! // Use the store in handlers
//! let networks = store.list_networks().await?;
//! let network = store.create_network(CreateNetworkRequest {
//!     name: "my-network".to_string(),
//!     ipv4_enabled: true,
//!     ..Default::default()
//! }).await?;
//! ```

mod error;
mod event;
mod raft_store;
mod traits;

pub use error::{Result, StoreError};
pub use event::Event;
pub use raft_store::RaftStore;
pub use traits::*;
