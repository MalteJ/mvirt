# 0002 — mvirt-cplane: the control plane component

- **Date:** 2026-05-08
- **Status:** Accepted

## Context

mvirt is a multi-host hypervisor cluster: per-host daemons (`mvirt-vmm`, `mvirt-zfs`, `mvirt-ebpf`) own concrete resources (running VMs, ZFS volumes, eBPF maps), and users want to talk to the *cluster* — "create me a VM with this image and these NICs" — without caring which host it lands on.

Something has to:
- Accept user intent (REST API)
- Hold the desired-state model of the cluster
- Survive the loss of any one machine in the cluster
- Pick a host for each resource
- Drive the per-host daemons to make actual state match desired state
- Tell users (and the audit log) what's actually happening

That's `mvirt-cplane`. A single binary that's the brain of an mvirt cluster. It runs on a small set of "control" machines (typically 3 or 5 for raft quorum) and is the only thing in the system that holds cluster-wide state.

The earlier shape of this binary (`mvirt-api`) was REST + raft + a manifest stream that nodes consumed; reconciliation lived on the nodes (see ADR-0003 for why we moved away from that). After the reverse-tunnel + reconciler-move work the cplane took on substantially more responsibility, and the name no longer fit. This ADR documents the cplane as it is now.

## Decision

`mvirt-cplane` is a single binary with **five tightly-integrated subsystems**, all sharing one redb-backed state machine and one raft log:

```
┌────────────────────────── mvirt-cplane ──────────────────────────┐
│                                                                  │
│  ┌────────────┐    ┌──────────────────────────┐    ┌──────────┐  │
│  │  REST API  │───►│      Raft (mraft)        │◄──►│  Peers   │  │
│  │   :8080    │    │   leader election +      │    │  :6001   │  │
│  │            │    │   log replication        │    └──────────┘  │
│  │            │    └──────────────┬───────────┘                  │
│  │            │ Command           │ apply() in redb txn          │
│  │            │                   ▼                              │
│  │            │    ┌──────────────────────────┐                  │
│  │            │◄───│  state.redb              │                  │
│  │            │    │  one table per resource  │                  │
│  │            │    │  value: Data{spec,status}│                  │
│  │            │    │  + Event broadcast       │                  │
│  │            │    └──────────────┬───────────┘                  │
│  │  Response  │                   │ Event                        │
│  └────────────┘                   ▼                              │
│                  ┌──────────────────────────────┐                │
│                  │      Reconciler controller    │               │
│                  │  events + 30s resync,         │               │
│                  │  per-resource reconcilers     │               │
│                  └──────────┬───────────────────┘                │
│                             │ typed gRPC via                     │
│                             │ NodeRegistry lookup                │
│                             ▼                                    │
│                  ┌──────────────────────────┐                    │
│                  │   Tunnel acceptor        │ port 50056         │
│                  │   + NodeRegistry         │ (see ADR-0003)     │
│                  └──────────────────────────┘                    │
│                                                                  │
│                  ┌──────────────────────────┐                    │
│                  │       Scheduler          │ assigns resources  │
│                  │  (capacity-aware)        │ to nodes at create │
│                  └──────────────────────────┘                    │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### Subsystem 1: Raft + state machine

The cluster's source of truth. The state machine is a set of redb tables — one per resource type — backing a `Data { spec, status }` struct per record.

**Data model (spec/status pattern).** Every reconciled resource follows the same shape:

- `Spec` = user intent. Written exclusively by REST handlers via `Create*` / `Update*Spec` / `Delete*` commands.
- `Status` = observed state. Written exclusively by the reconciler controller via `Update*Status` commands.

Spec and status live in one struct (one value per row in the resource's redb table) — physically one KV pair per resource, logically separated by which command wrote each half. We do **not** split them into separate tables: reads stay atomic and one-lookup, deletes are one operation, snapshot serialization is uniform. The actor boundary is enforced at the command set, not at storage.

Resources without observable state (Project, Network) are spec-only.

**Commands.** `command.rs` defines every state mutation. Each carries:
- `request_id` for idempotency (de-duped via an LRU on apply)
- `timestamp` set *before* raft replication — state-machine determinism requires this; calling `Utc::now()` inside `apply` would diverge across peers

Commands are partitioned by ownership:
- `Create*` / `Update*Spec` / `Delete*` — REST handlers build these from user requests
- `Update*Status` — reconciler controller emits these after a daemon RPC completes

**Apply.** `ApiState::apply(cmd)` runs in a redb write transaction: open the affected table(s), validate (existence, conflicts, idempotency cache), mutate the `Data` struct (spec fields *or* status fields, never both in one command), commit, return `(Response, Vec<Event>)`. The transaction commits before raft acks the command — durability and visibility are atomic.

**Events.** Emitted on state change and broadcast via `tokio::sync::broadcast` to in-process subscribers (REST handlers waiting on async outcomes, the reconciler controller). Events are not persisted — they're an in-process notification, not part of the state-machine contract. Subscribers that miss events fall back to the periodic resync (Subsystem 4).

**Storage layout.** Two files under `${data_dir}` (default `/var/lib/mvirt/cplane/`):
- `raft.db` — mraft's raft-log storage (sequence of pending Commands awaiting compaction)
- `state.redb` — the state machine itself (one table per resource type, mmap-backed)

`--dev` mode points both at tempfiles.

**Snapshots and log compaction.** openraft is configured with `SnapshotPolicy::LogsSinceLast(1000)` and `max_in_snapshot_log_to_keep: 0`. Every 1000 applied commands a snapshot is taken and earlier log entries are discarded. With redb-backing, snapshot creation is cheap: take a redb savepoint, stream the table contents to openraft as bytes, drop the savepoint. Restart loads the latest snapshot ("open the redb file at the savepoint") and replays at most ~1000 log entries — replay batches multiple entries per redb txn, so this is well under a second.

1000 (vs the openraft default 100): with cheap snapshots the cost of letting the log run further is just disk space (~500 KB of pending entries) and replay window. The previous default of 100 was defensive against expensive bincode-blob snapshots in older designs; that pressure doesn't apply here. We don't push to 5000+ because the savings are marginal and a tighter bound on recovery time is worth the few extra snapshots per hour.

**Followers.** Catch-up follows openraft's protocol: leader ships the latest snapshot (the redb tables as a chunked byte stream) plus any subsequent log entries. Snapshot transfer is O(state size on disk); log shipping is O(commands since snapshot).

mraft (an internal fork of openraft) handles leader election, log replication, and snapshot coordination. It exposes `write_or_forward(cmd)` so any cplane peer — leader or follower — can accept commands and have them routed.

### Subsystem 2: REST API

Public surface, port 8080. Accepts user intent, returns JSON. Implemented in `rest/` with axum.

- Handlers are split per resource (`rest/handlers/{vms,volumes,nics,...}.rs`).
- Each REST mutation builds a `Command`, submits via `RaftStore::write_command`, and reads the resulting `Response` (or waits on raft application).
- Reads go directly against `ApiState` (any cplane peer can serve reads, leader or not).
- The REST schema (`rest/ui_types.rs`) is a thin DTO layer over the state-machine types — keeps internal evolution decoupled from the wire format.

**URL shape.** The API serves under `/v1` with no `/api` segment (the API gets its own domain). Routes split into two regimes:

```
# Org-scoped (Org appears in the URL — Org-management + project list/create)
GET    /v1/orgs                              # list (filtered by membership)
POST   /v1/orgs                              # create
GET    /v1/orgs/:slug                        # get
PATCH  /v1/orgs/:slug                        # update
DELETE /v1/orgs/:slug                        # delete (rejects if Projects exist)

GET    /v1/orgs/:slug/projects               # list Projects within this Org
POST   /v1/orgs/:slug/projects               # create a Project under this Org

# Flat (Project slug is platform-unique, no Org segment)
GET    /v1/projects                          # list every Project the caller may see (across Orgs)
GET    /v1/projects/:slug                    # get
PATCH  /v1/projects/:slug                    # update
DELETE /v1/projects/:slug                    # delete

GET    /v1/projects/:slug/vms                # nested resource list (likewise networks, volumes, templates, security-groups, …)
POST   /v1/projects/:slug/vms                # create
GET    /v1/projects/:slug/vms/:id            # get
PATCH  /v1/projects/:slug/vms/:id            # update
DELETE /v1/projects/:slug/vms/:id            # delete
```

The Org-vs-flat split follows the K8s-namespace mental model from ADR-0004 (Project = namespace, slug platform-unique). Org appears in URLs only where the route is genuinely Org-scoped: managing the Org itself, or listing/creating its Projects. Once a Project is identified, the URL drops back to flat — the Project slug alone resolves the resource.

`POST /v1/orgs/:slug/projects` is the only Project-create route; the parent Org is taken from the URL, not from a body field. `GET /v1/projects` (cross-Org) and `GET /v1/orgs/:slug/projects` (Org-scoped) are both useful — the first powers the global Projects view, the second the Org-detail page. Both filter through the Authorizer.

UI routes mirror this split: Org-management pages at `/orgs/...`, Project-and-below at `/projects/:slug/...`. UI breadcrumbs reconstruct the parent Org from the Project's `org_id` so the user keeps the context even when it's not in the URL.

Authorization rules per route (which roles may call which endpoint) live in ADR-0004.

### Subsystem 3: Reverse-tunnel acceptor + NodeRegistry

Per-node connection management. Detailed in ADR-0003.

- `tunnel::listen(addr, registry)` accepts TCP from nodes on port 50056 and, per accepted socket, builds an inverted `tonic::Channel`, calls `NodeAgent.Identify`, and stores a `NodeHandle` in the `NodeRegistry` keyed by node id.
- Each `NodeHandle` bundles the typed gRPC clients (`NodeAgentClient`, `VmServiceClient`, `ZfsServiceClient`, `NetServiceClient`) — all sharing the same `Channel` so a single TCP socket carries everything.
- An open `WatchEvents` server-streaming RPC on each tunnel doubles as the liveness signal — its termination triggers deregistration.

The registry is the bridge between the rest of the cplane (which thinks in node ids) and the network (which is a per-node `Channel`). Reconcilers and any future cplane subsystem that needs to talk to a node go through it.

### Subsystem 4: Reconciler controller

Drives convergence. Sits between the state machine and the per-node daemons.

- Subscribes to the raft event broadcast.
- On every event, dispatches to the per-resource reconciler that owns that resource type.
- Independently runs a 30-second full-state resync that walks `ApiState` and reconciles every resource regardless of whether an event was seen for it. Catches drift from missed events, slow-arriving daemon state, and newly-connected nodes.
- Handles broadcast lag (`RecvError::Lagged`) by forcing an immediate resync rather than chasing individual missed events.

**Per-resource reconciler convention.** Every reconciled resource type lives in its own module under `mvirt-cplane/src/reconciler/` and exposes the same two functions:

```rust
pub async fn reconcile(ctx: &Ctx, id: &str) -> Result<()>
pub fn list_ids(state: &ApiState) -> Vec<String>
```

- `reconcile(ctx, id)` is the convergence step for one resource. It must be idempotent: read the current spec from state, check actual state on the daemon (`get_*`), make the daemon match the spec, and submit an `Update*Status` command back through raft. Called both on events and on each resync tick.
- `list_ids(state)` enumerates all resources of this type from `ApiState`. The controller calls it during resync to walk every resource.
- `Ctx` is a shared bundle (`{ store, registry, audit }`) — everything any reconciler might need. Defined once in `reconciler/context.rs` so the function signatures stay uniform.

Adding a new reconciled resource type is mechanical:
1. Create `mvirt-cplane/src/reconciler/<resource>.rs` with the two functions.
2. Add one match arm in `mod.rs`'s event dispatch (and one entry in resync).
3. The Spec/Status types and `Update*Status` command come from the state-machine work that defines the resource.

Free functions instead of a `trait Reconciler`: dyn-async traits are friction in Rust, the central match is the only "registration" point and it's grep-able and explicit, and we don't need polymorphism — every reconciler is named at compile time.

**Finalizers.** Cleanup-on-delete (e.g. tell `mvirt-zfs` to drop the zvol when the cplane volume is removed) is a separate concern not yet uniformly modeled. Currently `Event::*Deleted` variants carry only `id` + a few fields (varies per resource); when we land a uniform deletion model we'll add a third function (`finalize`) to the convention.

This gives k8s-style "level-triggered" semantics: events are an optimization for latency, correctness is anchored to the periodic resync, and reconcilers are idempotent by contract.

### Subsystem 5: Scheduler

Capacity-aware resource → node assignment, used at resource creation time.

- Consulted by REST handlers (and by command processing) when a user creates a VM/volume without a node selector.
- Uses node resource snapshots from `NodeAgent.CurrentResources` (cached in `NodeData` in the state machine) plus desired-state usage already in `ApiState` to pick a target.
- Output is the `node_id` field on the resource — once placed, ownership is sticky and the reconciler drives the chosen node.

### Data flow: a volume creation, end to end

```
1.  POST /v1/projects/X/volumes        ── REST handler builds the Spec
2.  Command::CreateVolume{spec}        ── written through raft
3.  apply (redb txn): insert volumes[id] = Data{spec, status: Pending}
4.  Event::VolumeCreated               ── broadcast
5.  Reconciler controller dispatches to volume::reconcile
6.  registry.get(&data.spec.node_id)   ── per-node Channel
7.  zfs.create_volume(spec)            ── over reverse tunnel
                                       ── node proxies → mvirt-zfs
8.  Command::UpdateVolumeStatus{Ready, path, used_bytes}
9.  apply (redb txn): update volumes[id].status
10. GET /v1/projects/X/volumes         ── observes status.phase=Ready
```

Every step except (7) is in-process inside one cplane instance (writes go through raft so they reach all peers). The only network hop for a successful create is the cplane → node tunnel call. Commands (2) and (8) are owned by different actors (REST + reconciler) and write disjoint halves of `Data` — that separation is the contract.

## Alternatives considered

- **Split into multiple binaries** (e.g. `mvirt-rest` + `mvirt-raft` + `mvirt-reconciler`). Rejected: the subsystems share a state machine and a raft log; splitting them imposes serialization across IPC for every read and forces a second consensus layer to coordinate. The single-binary cplane has a healthy degree of internal coupling that we'd be forced to invent in another form anyway.
- **Push reconciliation back to the nodes** (the previous shape). Rejected — see ADR-0003 for the reasoning.
- **External orchestrator** (Kubernetes, Nomad). Considered as an inspiration for the resource model and reconciler shape, but mvirt is a hypervisor, not a workload scheduler — the resources we manage (VMs, ZFS volumes, eBPF networks) live below the abstraction those orchestrators offer.
- **In-memory state machine + bincode-blob snapshots.** The starting shape. Rejected: snapshot creation is O(state size) in CPU and memory because the whole state has to be re-serialized; pushed us toward a tiny snapshot interval (100 commands) to bound that cost, which in turn meant constant snapshotting and large log-vs-snapshot churn. A redb-backed state machine inverts the trade-off — snapshots are savepoint references, the log can run further.
- **Spec and status as separate KV pairs.** Considered for sharper actor separation. Rejected: doubles the lookups for every read, complicates deletes (two rows to clean up), buys us nothing redb doesn't already give us via cheap point updates of the single value. The actor boundary is enforced at the command set; storage stays simple.
- **sled or fjall instead of redb.** Both are viable pure-Rust embedded stores. redb wins for our use case on transactional semantics (true ACID, no flush-controversy like sled), simpler operational model (single file), and active maintenance.

## Consequences

- The cplane is the single point of orchestration. Loss of all cplane peers means no new resource creates and no reconciliation, but already-running resources keep running on the nodes — daemons are independent.
- The cplane is the bottleneck under load: every resource event traverses it and every daemon RPC originates from it. Scaling is vertical for now (one cplane process per machine, raft quorum across machines).
- Followers can serve reads but writes must reach the leader; raft round-trip is the floor for any state-changing operation.
- Reconciliation latency on a happy path is bounded by raft apply + event broadcast + tunnel RPC (single-digit ms locally). Drift recovery is bounded by the 30s resync interval.
- The state machine is the cluster's contract. Adding a new resource type means: a new redb table, `Spec`/`Status` types, `Create*`/`Update*Spec`/`Update*Status`/`Delete*` commands and handlers, REST handler, reconciler. The shape is repetitive but predictable.
- Storage on disk grows in two directions: `state.redb` grows with the resource count (mmap-backed; RSS does not grow proportionally); `raft.db` is bounded by the snapshot interval (~1000 commands of pending log).
- Snapshot transfer to a recovering follower is O(state size on disk). For early deployments this is megabytes; well-behaved at the scales we target.
- Apply is now I/O-bound (redb write txn) instead of memory-only. With redb's mmap + COW BTree this is microseconds per command in practice — a real cost but not a noticeable one.

## What lives outside the cplane

For clarity, things the cplane does *not* own:
- **Concrete resources** — VMs, ZFS datasets, eBPF maps. Owned by per-host daemons.
- **Audit log storage** — `mvirt-log` is a separate service; the cplane is just a client.
- **Host-level log shipping** — `mvirt-shipper` runs alongside `mvirt-node`, independent of the cplane and the tunnel.
- **Container init inside MicroVMs** — `mvirt-one` is PID 1 inside guests, talks to `mvirt-vmm` via vsock.
