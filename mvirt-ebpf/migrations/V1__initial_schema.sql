-- Networks table
CREATE TABLE networks (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    ipv4_enabled INTEGER NOT NULL DEFAULT 0,
    ipv4_subnet TEXT,
    ipv6_enabled INTEGER NOT NULL DEFAULT 0,
    ipv6_prefix TEXT,
    dns_servers TEXT NOT NULL DEFAULT '[]',
    ntp_servers TEXT NOT NULL DEFAULT '[]',
    is_public INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- NICs table (uses tap_name instead of socket_path)
CREATE TABLE nics (
    id TEXT PRIMARY KEY,
    name TEXT,
    network_id TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    mac_address TEXT NOT NULL,
    ipv4_address TEXT,
    ipv6_address TEXT,
    routed_ipv4_prefixes TEXT NOT NULL DEFAULT '[]',
    routed_ipv6_prefixes TEXT NOT NULL DEFAULT '[]',
    tap_name TEXT NOT NULL,
    state INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Indexes for performance
CREATE INDEX idx_nics_network_id ON nics(network_id);
CREATE INDEX idx_networks_is_public ON networks(is_public);
CREATE INDEX idx_nics_name ON nics(name);
CREATE INDEX idx_nics_tap_name ON nics(tap_name);
