//! Events emitted by state machine changes.

use crate::command::{NetworkData, NicData, NodeData, VmData, VolumeData};

/// Events emitted when state changes occur.
///
/// These events are dispatched via broadcast channels to subscribers.
/// They are only emitted on the leader node after commands are applied.
#[derive(Debug, Clone)]
pub enum Event {
    // Node events
    /// A new node was registered.
    NodeRegistered(NodeData),
    /// A node was updated (status, resources).
    NodeUpdated {
        id: String,
        old: NodeData,
        new: NodeData,
    },
    /// A node was deregistered.
    NodeDeregistered { id: String },

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

    // VM events
    /// A new VM was created.
    VmCreated(VmData),
    /// A VM spec was updated.
    VmUpdated {
        id: String,
        old: VmData,
        new: VmData,
    },
    /// A VM status was updated (from node).
    VmStatusUpdated {
        id: String,
        old: VmData,
        new: VmData,
    },
    /// A VM was deleted.
    VmDeleted { id: String },

    // Volume events
    /// A new volume was created.
    VolumeCreated(VolumeData),
    /// A volume was deleted.
    VolumeDeleted { id: String, node_id: String },

    // Security Group events
    /// A security group was created.
    SecurityGroupCreated { id: String },
    /// A security group was deleted.
    SecurityGroupDeleted { id: String },
}

impl Event {
    /// Get the resource type for this event.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Event::NodeRegistered(_)
            | Event::NodeUpdated { .. }
            | Event::NodeDeregistered { .. } => "node",
            Event::NetworkCreated(_)
            | Event::NetworkUpdated { .. }
            | Event::NetworkDeleted { .. } => "network",
            Event::NicCreated(_) | Event::NicUpdated { .. } | Event::NicDeleted { .. } => "nic",
            Event::VmCreated(_)
            | Event::VmUpdated { .. }
            | Event::VmStatusUpdated { .. }
            | Event::VmDeleted { .. } => "vm",
            Event::VolumeCreated(_) | Event::VolumeDeleted { .. } => "volume",
            Event::SecurityGroupCreated { .. } | Event::SecurityGroupDeleted { .. } => {
                "security_group"
            }
        }
    }

    /// Get the resource ID for this event.
    pub fn resource_id(&self) -> &str {
        match self {
            Event::NodeRegistered(n) => &n.id,
            Event::NodeUpdated { id, .. } => id,
            Event::NodeDeregistered { id } => id,
            Event::NetworkCreated(n) => &n.id,
            Event::NetworkUpdated { id, .. } => id,
            Event::NetworkDeleted { id } => id,
            Event::NicCreated(n) => &n.id,
            Event::NicUpdated { id, .. } => id,
            Event::NicDeleted { id, .. } => id,
            Event::VmCreated(v) => &v.id,
            Event::VmUpdated { id, .. } => id,
            Event::VmStatusUpdated { id, .. } => id,
            Event::VmDeleted { id } => id,
            Event::VolumeCreated(v) => &v.id,
            Event::VolumeDeleted { id, .. } => id,
            Event::SecurityGroupCreated { id } | Event::SecurityGroupDeleted { id } => id,
        }
    }
}
