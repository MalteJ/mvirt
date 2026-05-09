# E2E Test Plan - Lokales Debugging

## Aktueller Stand (2026-02-04)

### Datenbanken
| Pfad | Komponente | Größe |
|------|-----------|-------|
| `/var/lib/mvirt/cplane/raft.db` | Cplane (Raft) | 3.6MB |
| `/var/lib/mvirt/vmm/mvirt.db` | VMM | 49KB |
| `/var/lib/mvirt/zfs/metadata.db` | ZFS | 73KB |
| `/var/lib/mvirt/log/logs.db` | Audit Log | ~4MB |
| `/var/lib/mvirt/ebpf/networks.db` | eBPF | 90KB |
| `/var/lib/mvirt/net/networks.db` | Legacy Net | 45KB |

### ZFS Datasets (mvirt-managed)
- `mvirt/templates/*` - Templates
- `mvirt/volumes/*` - Volumes

---

## Phase 1: Cleanup & Build

- [ ] Stoppe mvirt-cplane
- [ ] Lösche Datenbanken
- [ ] Lösche ZFS test-datasets
- [ ] `cargo build --release`

## Phase 2: Komponenten einzeln testen

### A) mvirt-zfs (Storage) ✓ PASSED (2026-02-04)
- [x] Starten
- [x] Volume erstellen
- [x] Template importieren (URL → qcow2 → zvol)
- [x] Clone from template
- [x] Snapshot erstellen
- [x] Volume löschen

### B) mvirt-vmm (Hypervisor) ✓ PASSED (2026-02-04)
- [x] Starten
- [x] VM erstellen (disk boot)
- [x] VM start/stop/kill
- [ ] Console attach (interactive, skipped)
- [x] VM löschen

### C) mvirt-ebpf (Networking) ✓ PASSED (2026-02-04)
- [x] Network erstellen
- [x] NIC erstellen (auto MAC/IP)
- [x] Security Group erstellen & attachen

### D) mvirt-cplane (Control Plane) ✓ PASSED (2026-02-04)
- [x] Starten (Raft single-node, leader elected)
- [x] REST: Project CRUD
- [ ] REST: Template import (needs Node agent)
- [ ] REST: Volume/VM/NIC erstellen (needs Node agent)

## Phase 3: Integration (Node-Agent) ✓ PASSED (2026-02-04)
- [x] mvirt-node connected & registered
- [x] Manifest-Sync funktioniert
- [x] Status-Updates kommen zurück
- [x] Template Import via REST API → Node reconciles

**BUG GEFUNDEN:** Manifest revision loop - Status updates trigger new manifests continuously (revision 1 → 1500+ in seconds). Funktional OK, aber Performance-Problem.

---

## Notizen

### Test-Session 2026-02-04

**Getestet:**
- mvirt-zfs: Volume CRUD, Template Import, Clone, Snapshot ✓
- mvirt-vmm: VM Create/Start/Stop/Kill/Delete ✓
- mvirt-ebpf: Network, NIC, SecurityGroup CRUD ✓
- mvirt-cplane: Raft bootstrap, REST API, Project CRUD ✓
- mvirt-node: Register, Manifest-Sync, Reconciliation ✓
- E2E: Template Import → Volume Clone via REST API ✓

**Bugs gefunden:**
1. **Manifest Loop** - Status updates trigger continuous manifest regeneration
   - Revision grows from 1 to 1500+ in seconds
   - Funktional OK, aber Performance/Log-Spam Problem
   - Location: mvirt-cplane manifest broadcast or mvirt-node status update logic
