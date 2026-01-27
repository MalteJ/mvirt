-- Remove FOREIGN KEY from vm_runtime to allow pod VMs
-- SQLite doesn't support ALTER TABLE for constraints, so recreate the table

CREATE TABLE vm_runtime_new (
    vm_id TEXT PRIMARY KEY,
    pid INTEGER NOT NULL,
    api_socket TEXT NOT NULL,
    serial_socket TEXT NOT NULL
);

INSERT INTO vm_runtime_new SELECT * FROM vm_runtime;

DROP TABLE vm_runtime;

ALTER TABLE vm_runtime_new RENAME TO vm_runtime;
