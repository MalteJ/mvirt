use anyhow::Result;
use chrono::Utc;
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

use crate::config::{NetworkEntry, NicEntry, NicState};

/// SQLite-backed metadata store for networks and NICs
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn new(metadata_dir: &str) -> Result<Self> {
        let db_path = format!("{}/metadata.db", metadata_dir);
        let db_url = format!("sqlite:{}?mode=rwc", db_path);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await?;

        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        // Networks table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS networks (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                ipv4_enabled INTEGER NOT NULL DEFAULT 0,
                ipv4_subnet TEXT,
                ipv6_enabled INTEGER NOT NULL DEFAULT 0,
                ipv6_prefix TEXT,
                dns_servers TEXT,
                ntp_servers TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // NICs table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS nics (
                id TEXT PRIMARY KEY,
                name TEXT,
                network_id TEXT NOT NULL,
                mac_address TEXT NOT NULL,
                ipv4_address TEXT,
                ipv6_address TEXT,
                routed_ipv4_prefixes TEXT,
                routed_ipv6_prefixes TEXT,
                socket_path TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'created',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (network_id) REFERENCES networks(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Address allocations table (for IP conflict detection)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS address_allocations (
                network_id TEXT NOT NULL,
                address TEXT NOT NULL,
                nic_id TEXT NOT NULL,
                PRIMARY KEY (network_id, address),
                FOREIGN KEY (network_id) REFERENCES networks(id) ON DELETE CASCADE,
                FOREIGN KEY (nic_id) REFERENCES nics(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Routed prefixes table (for routing table lookups)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS routed_prefixes (
                network_id TEXT NOT NULL,
                prefix TEXT NOT NULL,
                nic_id TEXT NOT NULL,
                PRIMARY KEY (network_id, prefix),
                FOREIGN KEY (network_id) REFERENCES networks(id) ON DELETE CASCADE,
                FOREIGN KEY (nic_id) REFERENCES nics(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // === Network operations ===

    pub async fn create_network(&self, entry: &NetworkEntry) -> Result<()> {
        let dns_json = serde_json::to_string(&entry.dns_servers)?;
        let ntp_json = serde_json::to_string(&entry.ntp_servers)?;

        sqlx::query(
            r#"
            INSERT INTO networks (id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix,
                                  dns_servers, ntp_servers, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.name)
        .bind(entry.ipv4_enabled)
        .bind(&entry.ipv4_subnet)
        .bind(entry.ipv6_enabled)
        .bind(&entry.ipv6_prefix)
        .bind(&dns_json)
        .bind(&ntp_json)
        .bind(&entry.created_at)
        .bind(&entry.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_network(&self, id: &str) -> Result<Option<NetworkEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix,
                   dns_servers, ntp_servers, created_at, updated_at
            FROM networks WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| self.row_to_network(&r)))
    }

    pub async fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix,
                   dns_servers, ntp_servers, created_at, updated_at
            FROM networks WHERE name = ?
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| self.row_to_network(&r)))
    }

    pub async fn list_networks(&self) -> Result<Vec<NetworkEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix,
                   dns_servers, ntp_servers, created_at, updated_at
            FROM networks ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(|r| self.row_to_network(r)).collect())
    }

    pub async fn update_network(
        &self,
        id: &str,
        dns_servers: &[String],
        ntp_servers: &[String],
    ) -> Result<bool> {
        let dns_json = serde_json::to_string(dns_servers)?;
        let ntp_json = serde_json::to_string(ntp_servers)?;
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            r#"
            UPDATE networks SET dns_servers = ?, ntp_servers = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&dns_json)
        .bind(&ntp_json)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_network(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM networks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn count_nics_in_network(&self, network_id: &str) -> Result<u32> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM nics WHERE network_id = ?")
            .bind(network_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(row.get::<i64, _>("count") as u32)
    }

    fn row_to_network(&self, row: &sqlx::sqlite::SqliteRow) -> NetworkEntry {
        let dns_json: String = row.get("dns_servers");
        let ntp_json: String = row.get("ntp_servers");

        NetworkEntry {
            id: row.get("id"),
            name: row.get("name"),
            ipv4_enabled: row.get::<i32, _>("ipv4_enabled") != 0,
            ipv4_subnet: row.get("ipv4_subnet"),
            ipv6_enabled: row.get::<i32, _>("ipv6_enabled") != 0,
            ipv6_prefix: row.get("ipv6_prefix"),
            dns_servers: serde_json::from_str(&dns_json).unwrap_or_default(),
            ntp_servers: serde_json::from_str(&ntp_json).unwrap_or_default(),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }

    // === NIC operations ===

    pub async fn create_nic(&self, entry: &NicEntry) -> Result<()> {
        let routed_v4_json = serde_json::to_string(&entry.routed_ipv4_prefixes)?;
        let routed_v6_json = serde_json::to_string(&entry.routed_ipv6_prefixes)?;

        sqlx::query(
            r#"
            INSERT INTO nics (id, name, network_id, mac_address, ipv4_address, ipv6_address,
                              routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state,
                              created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.name)
        .bind(&entry.network_id)
        .bind(&entry.mac_address)
        .bind(&entry.ipv4_address)
        .bind(&entry.ipv6_address)
        .bind(&routed_v4_json)
        .bind(&routed_v6_json)
        .bind(&entry.socket_path)
        .bind(entry.state.as_str())
        .bind(&entry.created_at)
        .bind(&entry.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_nic(&self, id: &str) -> Result<Option<NicEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address,
                   routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state,
                   created_at, updated_at
            FROM nics WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| self.row_to_nic(&r)))
    }

    pub async fn get_nic_by_name(&self, name: &str) -> Result<Option<NicEntry>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address,
                   routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state,
                   created_at, updated_at
            FROM nics WHERE name = ?
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| self.row_to_nic(&r)))
    }

    pub async fn list_nics(&self, network_id: Option<&str>) -> Result<Vec<NicEntry>> {
        let rows = match network_id {
            Some(net_id) => {
                sqlx::query(
                    r#"
                    SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address,
                           routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state,
                           created_at, updated_at
                    FROM nics WHERE network_id = ? ORDER BY created_at DESC
                    "#,
                )
                .bind(net_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address,
                           routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state,
                           created_at, updated_at
                    FROM nics ORDER BY created_at DESC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.iter().map(|r| self.row_to_nic(r)).collect())
    }

    pub async fn update_nic_routed_prefixes(
        &self,
        id: &str,
        routed_ipv4_prefixes: &[String],
        routed_ipv6_prefixes: &[String],
    ) -> Result<bool> {
        let routed_v4_json = serde_json::to_string(routed_ipv4_prefixes)?;
        let routed_v6_json = serde_json::to_string(routed_ipv6_prefixes)?;
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            r#"
            UPDATE nics SET routed_ipv4_prefixes = ?, routed_ipv6_prefixes = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&routed_v4_json)
        .bind(&routed_v6_json)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_nic_state(&self, id: &str, state: NicState) -> Result<bool> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query("UPDATE nics SET state = ?, updated_at = ? WHERE id = ?")
            .bind(state.as_str())
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_nic(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM nics WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    fn row_to_nic(&self, row: &sqlx::sqlite::SqliteRow) -> NicEntry {
        let routed_v4_json: String = row.get("routed_ipv4_prefixes");
        let routed_v6_json: String = row.get("routed_ipv6_prefixes");
        let state_str: String = row.get("state");

        NicEntry {
            id: row.get("id"),
            name: row.get("name"),
            network_id: row.get("network_id"),
            mac_address: row.get("mac_address"),
            ipv4_address: row.get("ipv4_address"),
            ipv6_address: row.get("ipv6_address"),
            routed_ipv4_prefixes: serde_json::from_str(&routed_v4_json).unwrap_or_default(),
            routed_ipv6_prefixes: serde_json::from_str(&routed_v6_json).unwrap_or_default(),
            socket_path: row.get("socket_path"),
            state: state_str.parse().unwrap_or(NicState::Created),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }

    // === Address allocation operations ===

    pub async fn allocate_address(
        &self,
        network_id: &str,
        address: &str,
        nic_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO address_allocations (network_id, address, nic_id) VALUES (?, ?, ?)",
        )
        .bind(network_id)
        .bind(address)
        .bind(nic_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn is_address_allocated(&self, network_id: &str, address: &str) -> Result<bool> {
        let row = sqlx::query(
            "SELECT COUNT(*) as count FROM address_allocations WHERE network_id = ? AND address = ?",
        )
        .bind(network_id)
        .bind(address)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<i64, _>("count") > 0)
    }

    pub async fn deallocate_addresses_for_nic(&self, nic_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM address_allocations WHERE nic_id = ?")
            .bind(nic_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // === Routed prefix operations ===

    pub async fn add_routed_prefix(
        &self,
        network_id: &str,
        prefix: &str,
        nic_id: &str,
    ) -> Result<()> {
        sqlx::query("INSERT INTO routed_prefixes (network_id, prefix, nic_id) VALUES (?, ?, ?)")
            .bind(network_id)
            .bind(prefix)
            .bind(nic_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn remove_routed_prefixes_for_nic(&self, nic_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM routed_prefixes WHERE nic_id = ?")
            .bind(nic_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_routed_prefixes_for_network(
        &self,
        network_id: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query("SELECT prefix, nic_id FROM routed_prefixes WHERE network_id = ?")
            .bind(network_id)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .iter()
            .map(|r| (r.get("prefix"), r.get("nic_id")))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NetworkEntry, NicEntryBuilder};
    use tempfile::TempDir;

    async fn setup_store() -> (Store, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path().to_str().unwrap()).await.unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn test_network_crud() {
        let (store, _dir) = setup_store().await;

        // Create
        let entry = NetworkEntry::new(
            "test-net".to_string(),
            true,
            Some("10.0.0.0/24".to_string()),
            true,
            Some("fd00::/64".to_string()),
            vec!["1.1.1.1".to_string()],
            vec![],
        );
        store.create_network(&entry).await.unwrap();

        // Get by ID
        let fetched = store.get_network(&entry.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "test-net");
        assert!(fetched.ipv4_enabled);

        // Get by name
        let fetched = store
            .get_network_by_name("test-net")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.id, entry.id);

        // List
        let networks = store.list_networks().await.unwrap();
        assert_eq!(networks.len(), 1);

        // Update
        store
            .update_network(&entry.id, &["8.8.8.8".to_string()], &[])
            .await
            .unwrap();
        let updated = store.get_network(&entry.id).await.unwrap().unwrap();
        assert_eq!(updated.dns_servers, vec!["8.8.8.8".to_string()]);

        // Delete
        let deleted = store.delete_network(&entry.id).await.unwrap();
        assert!(deleted);
        assert!(store.get_network(&entry.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_nic_crud() {
        let (store, _dir) = setup_store().await;

        // Create network first
        let network = NetworkEntry::new(
            "net1".to_string(),
            true,
            Some("10.0.0.0/24".to_string()),
            false,
            None,
            vec![],
            vec![],
        );
        store.create_network(&network).await.unwrap();

        // Create NIC
        let nic = NicEntryBuilder::new(
            network.id.clone(),
            "52:54:00:12:34:56".to_string(),
            "/tmp/test.sock".to_string(),
        )
        .name(Some("eth0".to_string()))
        .ipv4_address(Some("10.0.0.5".to_string()))
        .build();
        store.create_nic(&nic).await.unwrap();

        // Get
        let fetched = store.get_nic(&nic.id).await.unwrap().unwrap();
        assert_eq!(fetched.mac_address, "52:54:00:12:34:56");
        assert_eq!(fetched.ipv4_address, Some("10.0.0.5".to_string()));

        // List
        let nics = store.list_nics(Some(&network.id)).await.unwrap();
        assert_eq!(nics.len(), 1);

        // Count
        let count = store.count_nics_in_network(&network.id).await.unwrap();
        assert_eq!(count, 1);

        // Delete
        store.delete_nic(&nic.id).await.unwrap();
        assert!(store.get_nic(&nic.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_address_allocation() {
        let (store, _dir) = setup_store().await;

        let network = NetworkEntry::new(
            "net1".to_string(),
            true,
            Some("10.0.0.0/24".to_string()),
            false,
            None,
            vec![],
            vec![],
        );
        store.create_network(&network).await.unwrap();

        // Create NIC first (FK constraint)
        let nic = NicEntryBuilder::new(
            network.id.clone(),
            "52:54:00:12:34:56".to_string(),
            "/tmp/test.sock".to_string(),
        )
        .build();
        store.create_nic(&nic).await.unwrap();

        // Allocate
        store
            .allocate_address(&network.id, "10.0.0.5", &nic.id)
            .await
            .unwrap();

        // Check allocated
        assert!(
            store
                .is_address_allocated(&network.id, "10.0.0.5")
                .await
                .unwrap()
        );
        assert!(
            !store
                .is_address_allocated(&network.id, "10.0.0.6")
                .await
                .unwrap()
        );

        // Deallocate
        store.deallocate_addresses_for_nic(&nic.id).await.unwrap();
        assert!(
            !store
                .is_address_allocated(&network.id, "10.0.0.5")
                .await
                .unwrap()
        );
    }
}
