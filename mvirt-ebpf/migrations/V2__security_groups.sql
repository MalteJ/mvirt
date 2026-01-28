-- Security Groups table
CREATE TABLE security_groups (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    description TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Security Group Rules table
-- Rules are always "allow" (like AWS Security Groups)
-- No explicit "deny" rules, no priorities
CREATE TABLE security_group_rules (
    id TEXT PRIMARY KEY,
    security_group_id TEXT NOT NULL REFERENCES security_groups(id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK(direction IN ('ingress', 'egress')),
    protocol TEXT NOT NULL CHECK(protocol IN ('all', 'tcp', 'udp', 'icmp', 'icmpv6')),
    port_start INTEGER,
    port_end INTEGER,
    cidr TEXT,  -- IPv4: "10.0.0.0/8", IPv6: "fd00::/8", NULL = any
    description TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Junction table for NIC <-> Security Group many-to-many relationship
CREATE TABLE nic_security_groups (
    nic_id TEXT NOT NULL REFERENCES nics(id) ON DELETE CASCADE,
    security_group_id TEXT NOT NULL REFERENCES security_groups(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (nic_id, security_group_id)
);

-- Indexes for performance
CREATE INDEX idx_security_group_rules_sg_id ON security_group_rules(security_group_id);
CREATE INDEX idx_nic_security_groups_nic_id ON nic_security_groups(nic_id);
CREATE INDEX idx_nic_security_groups_sg_id ON nic_security_groups(security_group_id);
