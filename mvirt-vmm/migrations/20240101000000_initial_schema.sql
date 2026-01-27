-- Initial schema for mvirt-vmm
CREATE TABLE IF NOT EXISTS vms (
    id TEXT PRIMARY KEY,
    name TEXT,
    state TEXT NOT NULL DEFAULT 'stopped',
    config_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER
);

CREATE TABLE IF NOT EXISTS vm_runtime (
    vm_id TEXT PRIMARY KEY REFERENCES vms(id) ON DELETE CASCADE,
    pid INTEGER NOT NULL,
    api_socket TEXT NOT NULL,
    serial_socket TEXT NOT NULL
);
