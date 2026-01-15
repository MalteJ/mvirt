-- Migration: Remove UNIQUE constraint from volumes.name
--
-- This migration exists for databases created before this constraint was removed.
-- The initial_schema migration (20240101000000) already creates volumes without UNIQUE.
--
-- For existing databases where UNIQUE was already removed manually or by previous
-- migration attempts, this is a no-op.
--
-- Note: If your database still has UNIQUE on volumes.name, run this manually:
--   PRAGMA foreign_keys = OFF;
--   CREATE TABLE volumes_new AS SELECT * FROM volumes;
--   DROP TABLE volumes;
--   ALTER TABLE volumes_new RENAME TO volumes;
--   PRAGMA foreign_keys = ON;

SELECT 1;
