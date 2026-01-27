-- Add microvm flag to distinguish pod VMs from regular VMs
ALTER TABLE vms ADD COLUMN microvm BOOLEAN NOT NULL DEFAULT FALSE;
