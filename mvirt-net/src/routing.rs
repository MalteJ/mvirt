//! LPM (Longest Prefix Match) routing tables for packet forwarding.
//!
//! This module provides:
//! - `RouteTarget`: Where to send packets (reactor, blackhole, etc.)
//! - `LpmTable`: A single routing table with IPv4/IPv6 LPM lookup
//! - `RoutingTables`: Collection of tables (for per-reactor local copy)
//! - `RouteUpdate`: Messages for broadcasting routing changes
//! - `RoutingManager`: Control plane that broadcasts updates to reactors

use crate::inter_reactor::ReactorId;
use ipnet::{Ipv4Net, Ipv6Net};
use prefix_trie::PrefixMap;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::mpsc::Sender;
use tracing::{debug, warn};
use uuid::Uuid;

/// Target for a routing table entry.
#[derive(Debug, Clone)]
pub enum RouteTarget {
    /// Route to a specific reactor (TUN or vhost).
    Reactor {
        /// Reactor ID to forward packets to.
        id: ReactorId,
    },
    /// Route to a vhost-user device (legacy, for backwards compatibility).
    Vhost {
        /// UUID identifying the vhost device.
        id: Uuid,
    },
    /// Route to a TUN device (legacy, for backwards compatibility).
    Tun {
        /// Interface index (from TunDevice.if_index).
        if_index: u32,
    },
    /// Extensible target for future use cases.
    Custom {
        /// Type identifier for the custom target.
        target_type: u32,
        /// Opaque data for the handler.
        data: Vec<u8>,
    },
    /// Drop the packet (blackhole route).
    Drop,
}

impl RouteTarget {
    /// Create a target for a specific reactor.
    pub fn reactor(id: ReactorId) -> Self {
        RouteTarget::Reactor { id }
    }

    /// Create a target that drops packets.
    pub fn drop() -> Self {
        RouteTarget::Drop
    }
}

/// Routing decision for a packet.
///
/// This enum represents the result of looking up a packet's destination
/// in the routing tables and deciding where to send it.
#[derive(Debug, Clone)]
pub enum RoutingDecision {
    /// Route to a TUN interface (VM → external network).
    ToTun {
        /// Interface index of the TUN device.
        if_index: u32,
    },
    /// Route to a vhost reactor (VM → VM or TUN → VM).
    ToVhost {
        /// Target reactor ID.
        reactor_id: ReactorId,
    },
    /// Handle locally (ICMP echo, etc.).
    Local,
    /// Drop the packet (blackhole route or no route found).
    Drop,
}

/// IP prefix (IPv4 or IPv6) for routing commands.
#[derive(Debug, Clone)]
pub enum IpPrefix {
    V4(Ipv4Net),
    V6(Ipv6Net),
}

/// A single LPM routing table supporting both IPv4 and IPv6.
#[derive(Clone)]
pub struct LpmTable {
    /// Table identifier.
    pub id: Uuid,
    /// Human-readable name (for debugging).
    pub name: String,
    /// IPv4 routing entries.
    ipv4: PrefixMap<Ipv4Net, RouteTarget>,
    /// IPv6 routing entries.
    ipv6: PrefixMap<Ipv6Net, RouteTarget>,
}

impl LpmTable {
    /// Create a new empty LPM table.
    pub fn new(id: Uuid, name: impl Into<String>) -> Self {
        LpmTable {
            id,
            name: name.into(),
            ipv4: PrefixMap::new(),
            ipv6: PrefixMap::new(),
        }
    }

    /// Lookup IPv4 address, returns longest matching prefix target.
    pub fn lookup_v4(&self, addr: Ipv4Addr) -> Option<&RouteTarget> {
        let prefix = Ipv4Net::new(addr, 32).ok()?;
        self.ipv4.get_lpm(&prefix).map(|(_, target)| target)
    }

    /// Lookup IPv6 address, returns longest matching prefix target.
    pub fn lookup_v6(&self, addr: Ipv6Addr) -> Option<&RouteTarget> {
        let prefix = Ipv6Net::new(addr, 128).ok()?;
        self.ipv6.get_lpm(&prefix).map(|(_, target)| target)
    }

    /// Insert IPv4 route.
    pub fn insert_v4(&mut self, prefix: Ipv4Net, target: RouteTarget) {
        self.ipv4.insert(prefix, target);
    }

    /// Insert IPv6 route.
    pub fn insert_v6(&mut self, prefix: Ipv6Net, target: RouteTarget) {
        self.ipv6.insert(prefix, target);
    }

    /// Remove IPv4 route.
    pub fn remove_v4(&mut self, prefix: &Ipv4Net) -> Option<RouteTarget> {
        self.ipv4.remove(prefix)
    }

    /// Remove IPv6 route.
    pub fn remove_v6(&mut self, prefix: &Ipv6Net) -> Option<RouteTarget> {
        self.ipv6.remove(prefix)
    }
}

/// Manages multiple LPM tables indexed by UUID.
#[derive(Clone)]
pub struct RoutingTables {
    /// Tables indexed by UUID.
    tables: HashMap<Uuid, LpmTable>,
    /// Default table UUID (if any).
    default_table: Option<Uuid>,
}

impl RoutingTables {
    /// Create a new empty routing table manager.
    pub fn new() -> Self {
        RoutingTables {
            tables: HashMap::new(),
            default_table: None,
        }
    }

    /// Add a table. First table becomes default if none set.
    pub fn add_table(&mut self, table: LpmTable) {
        let id = table.id;
        self.tables.insert(id, table);
        if self.default_table.is_none() {
            self.default_table = Some(id);
        }
    }

    /// Get a table by ID.
    pub fn get_table(&self, id: &Uuid) -> Option<&LpmTable> {
        self.tables.get(id)
    }

    /// Get a mutable table by ID.
    pub fn get_table_mut(&mut self, id: &Uuid) -> Option<&mut LpmTable> {
        self.tables.get_mut(id)
    }

    /// Get the default table.
    pub fn get_default(&self) -> Option<&LpmTable> {
        self.default_table.and_then(|id| self.tables.get(&id))
    }

    /// Set the default table.
    pub fn set_default(&mut self, id: Uuid) {
        if self.tables.contains_key(&id) {
            self.default_table = Some(id);
        }
    }

    /// Remove a table by ID.
    pub fn remove_table(&mut self, id: &Uuid) -> Option<LpmTable> {
        let table = self.tables.remove(id);
        if self.default_table == Some(*id) {
            self.default_table = self.tables.keys().next().copied();
        }
        table
    }
}

impl Default for RoutingTables {
    fn default() -> Self {
        Self::new()
    }
}

/// A routing update message broadcast to all reactors.
///
/// Reactors receive these messages and apply them to their local
/// RoutingTables copy, enabling lock-free routing lookups in the
/// data plane.
#[derive(Debug, Clone)]
pub enum RouteUpdate {
    /// Create a new routing table.
    CreateTable {
        /// Table UUID.
        id: Uuid,
        /// Human-readable name.
        name: String,
    },
    /// Delete a routing table.
    DeleteTable {
        /// Table UUID to delete.
        id: Uuid,
    },
    /// Add a route to a table.
    AddRoute {
        /// Table UUID.
        table_id: Uuid,
        /// IP prefix to match.
        prefix: IpPrefix,
        /// Target for matching packets.
        target: RouteTarget,
    },
    /// Remove a route from a table.
    RemoveRoute {
        /// Table UUID.
        table_id: Uuid,
        /// IP prefix to remove.
        prefix: IpPrefix,
    },
    /// Set the default routing table.
    SetDefaultTable {
        /// Table UUID to set as default.
        id: Uuid,
    },
}

impl RouteUpdate {
    /// Apply this update to a RoutingTables instance.
    pub fn apply(&self, tables: &mut RoutingTables) {
        match self {
            RouteUpdate::CreateTable { id, name } => {
                tables.add_table(LpmTable::new(*id, name.clone()));
            }
            RouteUpdate::DeleteTable { id } => {
                tables.remove_table(id);
            }
            RouteUpdate::AddRoute {
                table_id,
                prefix,
                target,
            } => {
                if let Some(table) = tables.get_table_mut(table_id) {
                    match prefix {
                        IpPrefix::V4(p) => table.insert_v4(*p, target.clone()),
                        IpPrefix::V6(p) => table.insert_v6(*p, target.clone()),
                    }
                }
            }
            RouteUpdate::RemoveRoute { table_id, prefix } => {
                if let Some(table) = tables.get_table_mut(table_id) {
                    match prefix {
                        IpPrefix::V4(p) => {
                            table.remove_v4(p);
                        }
                        IpPrefix::V6(p) => {
                            table.remove_v6(p);
                        }
                    }
                }
            }
            RouteUpdate::SetDefaultTable { id } => {
                tables.set_default(*id);
            }
        }
    }
}

/// Central routing manager that broadcasts updates to all reactors.
///
/// The RoutingManager owns the authoritative routing tables and broadcasts
/// any changes to registered reactors. Each reactor maintains its own
/// local copy of the tables for lock-free lookups.
pub struct RoutingManager {
    /// The authoritative routing tables.
    tables: RoutingTables,
    /// Channels to send updates to each reactor.
    update_senders: Vec<Sender<RouteUpdate>>,
}

impl RoutingManager {
    /// Create a new RoutingManager.
    pub fn new() -> Self {
        RoutingManager {
            tables: RoutingTables::new(),
            update_senders: Vec::new(),
        }
    }

    /// Register a reactor to receive routing updates.
    ///
    /// Returns the current routing tables for the reactor to initialize with.
    pub fn register_reactor(&mut self, sender: Sender<RouteUpdate>) -> RoutingTables {
        self.update_senders.push(sender);
        // Clone current tables for the new reactor
        self.tables.clone()
    }

    /// Unregister a reactor (by removing closed channels on next broadcast).
    /// Channels are cleaned up lazily during broadcast.
    pub fn cleanup_closed_channels(&mut self) {
        // We'll clean up on the next broadcast - closed channels will fail to send
    }

    /// Broadcast an update to all registered reactors.
    fn broadcast(&mut self, update: &RouteUpdate) {
        // Remove senders that fail (channel closed)
        self.update_senders
            .retain(|sender| match sender.send(update.clone()) {
                Ok(()) => true,
                Err(_) => {
                    debug!("Removing closed routing update channel");
                    false
                }
            });
    }

    /// Create a new routing table and broadcast to all reactors.
    pub fn create_table(&mut self, id: Uuid, name: impl Into<String>) {
        let name = name.into();
        debug!(%id, %name, "Creating routing table");

        let update = RouteUpdate::CreateTable {
            id,
            name: name.clone(),
        };

        // Apply locally first
        update.apply(&mut self.tables);

        // Broadcast to reactors
        self.broadcast(&update);
    }

    /// Delete a routing table and broadcast to all reactors.
    pub fn delete_table(&mut self, id: Uuid) {
        debug!(%id, "Deleting routing table");

        let update = RouteUpdate::DeleteTable { id };
        update.apply(&mut self.tables);
        self.broadcast(&update);
    }

    /// Add a route to a table and broadcast to all reactors.
    pub fn add_route(&mut self, table_id: Uuid, prefix: IpPrefix, target: RouteTarget) {
        if self.tables.get_table(&table_id).is_none() {
            warn!(%table_id, "Cannot add route: table not found");
            return;
        }

        debug!(%table_id, ?prefix, "Adding route");

        let update = RouteUpdate::AddRoute {
            table_id,
            prefix,
            target,
        };
        update.apply(&mut self.tables);
        self.broadcast(&update);
    }

    /// Remove a route from a table and broadcast to all reactors.
    pub fn remove_route(&mut self, table_id: Uuid, prefix: IpPrefix) {
        debug!(%table_id, ?prefix, "Removing route");

        let update = RouteUpdate::RemoveRoute { table_id, prefix };
        update.apply(&mut self.tables);
        self.broadcast(&update);
    }

    /// Set the default routing table and broadcast to all reactors.
    pub fn set_default_table(&mut self, id: Uuid) {
        debug!(%id, "Setting default routing table");

        let update = RouteUpdate::SetDefaultTable { id };
        update.apply(&mut self.tables);
        self.broadcast(&update);
    }

    /// Get a reference to the authoritative routing tables.
    pub fn tables(&self) -> &RoutingTables {
        &self.tables
    }

    /// Get the number of registered reactors.
    pub fn reactor_count(&self) -> usize {
        self.update_senders.len()
    }
}

impl Default for RoutingManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lpm_lookup_v4() {
        let mut table = LpmTable::new(Uuid::new_v4(), "test");

        // Insert 10.0.0.0/8 -> Tun
        table.insert_v4(
            "10.0.0.0/8".parse().unwrap(),
            RouteTarget::Tun { if_index: 1 },
        );

        // Insert more specific 10.1.0.0/16 -> Vhost
        table.insert_v4(
            "10.1.0.0/16".parse().unwrap(),
            RouteTarget::Vhost { id: Uuid::new_v4() },
        );

        // Lookup 10.1.2.3 -> should match 10.1.0.0/16 (more specific)
        let target = table.lookup_v4("10.1.2.3".parse().unwrap());
        assert!(matches!(target, Some(RouteTarget::Vhost { .. })));

        // Lookup 10.2.3.4 -> should match 10.0.0.0/8
        let target = table.lookup_v4("10.2.3.4".parse().unwrap());
        assert!(matches!(target, Some(RouteTarget::Tun { if_index: 1 })));

        // Lookup 192.168.1.1 -> no match
        let target = table.lookup_v4("192.168.1.1".parse().unwrap());
        assert!(target.is_none());
    }

    #[test]
    fn test_routing_tables_default() {
        let mut tables = RoutingTables::new();

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        tables.add_table(LpmTable::new(id1, "table1"));
        tables.add_table(LpmTable::new(id2, "table2"));

        // First table should be default
        assert_eq!(tables.get_default().map(|t| t.id), Some(id1));

        // Change default
        tables.set_default(id2);
        assert_eq!(tables.get_default().map(|t| t.id), Some(id2));

        // Remove default, should fall back
        tables.remove_table(&id2);
        assert_eq!(tables.get_default().map(|t| t.id), Some(id1));
    }

    #[test]
    fn test_route_update_apply() {
        let mut tables = RoutingTables::new();
        let table_id = Uuid::new_v4();

        // Create table via update
        let create = RouteUpdate::CreateTable {
            id: table_id,
            name: "test".to_string(),
        };
        create.apply(&mut tables);
        assert!(tables.get_table(&table_id).is_some());

        // Add route via update
        let add = RouteUpdate::AddRoute {
            table_id,
            prefix: IpPrefix::V4("10.0.0.0/8".parse().unwrap()),
            target: RouteTarget::Tun { if_index: 42 },
        };
        add.apply(&mut tables);
        let table = tables.get_table(&table_id).unwrap();
        assert!(matches!(
            table.lookup_v4("10.0.0.1".parse().unwrap()),
            Some(RouteTarget::Tun { if_index: 42 })
        ));

        // Remove route via update
        let remove = RouteUpdate::RemoveRoute {
            table_id,
            prefix: IpPrefix::V4("10.0.0.0/8".parse().unwrap()),
        };
        remove.apply(&mut tables);
        let table = tables.get_table(&table_id).unwrap();
        assert!(table.lookup_v4("10.0.0.1".parse().unwrap()).is_none());

        // Delete table via update
        let delete = RouteUpdate::DeleteTable { id: table_id };
        delete.apply(&mut tables);
        assert!(tables.get_table(&table_id).is_none());
    }

    #[test]
    fn test_routing_manager_broadcast() {
        use std::sync::mpsc;

        let mut manager = RoutingManager::new();

        // Register two receivers
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();

        let _tables1 = manager.register_reactor(tx1);
        let _tables2 = manager.register_reactor(tx2);

        assert_eq!(manager.reactor_count(), 2);

        // Create a table - should broadcast to both
        let table_id = Uuid::new_v4();
        manager.create_table(table_id, "broadcast_test");

        // Both should receive the update
        let update1 = rx1.try_recv().unwrap();
        let update2 = rx2.try_recv().unwrap();

        assert!(matches!(
            update1,
            RouteUpdate::CreateTable { id, .. } if id == table_id
        ));
        assert!(matches!(
            update2,
            RouteUpdate::CreateTable { id, .. } if id == table_id
        ));

        // Manager's local tables should also be updated
        assert!(manager.tables().get_table(&table_id).is_some());
    }

    #[test]
    fn test_routing_manager_cleanup_closed() {
        use std::sync::mpsc;

        let mut manager = RoutingManager::new();

        let (tx1, rx1) = mpsc::channel();
        let (tx2, _rx2) = mpsc::channel(); // rx2 dropped immediately

        manager.register_reactor(tx1);
        manager.register_reactor(tx2);

        assert_eq!(manager.reactor_count(), 2);

        // Drop rx2's receiver (already dropped)
        drop(_rx2);

        // Broadcast - should clean up the closed channel
        let table_id = Uuid::new_v4();
        manager.create_table(table_id, "cleanup_test");

        // tx1 should still work
        assert!(rx1.try_recv().is_ok());

        // Manager should have cleaned up the dead channel
        assert_eq!(manager.reactor_count(), 1);
    }

    #[test]
    fn test_route_target_reactor() {
        let reactor_id = ReactorId::new();
        let target = RouteTarget::reactor(reactor_id);
        assert!(matches!(target, RouteTarget::Reactor { id } if id == reactor_id));

        let drop_target = RouteTarget::drop();
        assert!(matches!(drop_target, RouteTarget::Drop));
    }
}
