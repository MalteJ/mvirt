//! Events emitted by state machine changes.

use crate::command::{NetworkData, NicData};

/// Events emitted when state changes occur.
///
/// These events are dispatched via broadcast channels to subscribers.
/// They are only emitted on the leader node after commands are applied.
#[derive(Debug, Clone)]
pub enum Event {
    // Network events
    /// A new network was created.
    NetworkCreated(NetworkData),
    /// A network was updated.
    NetworkUpdated {
        id: String,
        old: NetworkData,
        new: NetworkData,
    },
    /// A network was deleted.
    NetworkDeleted { id: String },

    // NIC events
    /// A new NIC was created.
    NicCreated(NicData),
    /// A NIC was updated.
    NicUpdated {
        id: String,
        old: NicData,
        new: NicData,
    },
    /// A NIC was deleted.
    NicDeleted { id: String, network_id: String },
}

impl Event {
    /// Get the resource type for this event.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Event::NetworkCreated(_)
            | Event::NetworkUpdated { .. }
            | Event::NetworkDeleted { .. } => "network",
            Event::NicCreated(_) | Event::NicUpdated { .. } | Event::NicDeleted { .. } => "nic",
        }
    }

    /// Get the resource ID for this event.
    pub fn resource_id(&self) -> &str {
        match self {
            Event::NetworkCreated(n) => &n.id,
            Event::NetworkUpdated { id, .. } => id,
            Event::NetworkDeleted { id } => id,
            Event::NicCreated(n) => &n.id,
            Event::NicUpdated { id, .. } => id,
            Event::NicDeleted { id, .. } => id,
        }
    }
}
