# 0001 — mvirt: overall architecture

- **Date:** 2026-05-08
- **Status:** Accepted

## Context

mvirt is a hypervisor cluster: a small set of physical machines (we run on Proxmox-style host hardware) that together expose a Kubernetes-shaped resource model — projects, networks, NICs, VMs, volumes — but at the *infrastructure* layer, not the workload layer. We want:

- Multi-tenant resource isolation per project
- VMs (cloud-hypervisor MicroVMs) and pods (containers inside MicroVMs)
- Per-host ZFS storage with thin-provisioned volumes and templating
- Per-host eBPF-based networking with security groups and overlay routing
- A single API surface (REST + a UI) that hides which machine a resource lives on
- Survival of any one machine's failure — both the orchestration layer and the per-host daemons must be independently restartable
- Out-of-the-box deployability via NixOS, with all binaries statically linked (musl) so a host needs nothing but the artefact

Existing options didn't fit. Kubernetes operates above this layer (it manages workloads, not hypervisors). Proxmox doesn't expose a Kubernetes-shaped declarative API. OpenStack is too heavyweight. So mvirt is purpose-built.

This ADR describes the system at a glance — what the components are, how they fit, and the design principles that ripple through everything. Component-specific design lives in the linked ADRs.

## Decision

mvirt is a **single control plane** plus **per-host daemons**, talking via **reverse-tunneled gRPC**, with **raft-replicated desired state** and **k8s-style reconciliation**.

### Components

```
┌──────────────────────── control plane (3 or 5 machines, raft quorum) ────────────────────────┐
│                                                                                              │
│  mvirt-cplane    ◄── Raft consensus ──►  mvirt-cplane    ◄── Raft consensus ──►  mvirt-cplane│
│  (REST :8080,                            (peer)                                  (peer)      │
│   tunnel :50056,                                                                             │
│   reconciler,                                                                                │
│   scheduler)                                                                                 │
│                                                                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
                                            ▲
                                            │ reverse tunnel: nodes dial cplane (NAT-friendly)
                                            │ inverted gRPC: cplane = client, node = server
                                            │
┌─────────────────────────────────── per-host (any number) ────────────────────────────────────┐
│                                                                                              │
│   mvirt-node (per-host agent: hosts NodeAgent + byte-level proxies for the local daemons)    │
│                                                                                              │
│      │                                                                                       │
│      ├──► mvirt-vmm   :50051   VMs (cloud-hypervisor) + pods (microVMs hosting containers)   │
│      │                          └──► mvirt-one (PID 1 inside guests, vsock back to vmm)      │
│      ├──► mvirt-zfs   :50053   storage (volumes, snapshots, templates)                       │
│      └──► mvirt-ebpf  :50054   networking (TAP, NAT, security groups, overlay routes)        │
│                                                                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────┘

         All daemons + cplane ──audit log──►  mvirt-log  (per-cluster)
         journald (host) ──mvirt-shipper──►   mvirt-log  (per-host)

         Clients:  mvirt-cli (terminal TUI),  mvirt-ui (React/Vite web UI)
                   both speak REST/JSON to mvirt-cplane :8080
```

| Component             | Role                                                                              | See        |
|-----------------------|-----------------------------------------------------------------------------------|------------|
| **mvirt-cplane**      | Cluster brain: REST API, raft state machine, scheduler, reconciler, tunnel acceptor| ADR-0002   |
| **mvirt-node**        | Per-host agent: dials cplane, proxies daemon RPCs, no orchestration logic         | ADR-0003   |
| **mvirt-vmm**         | Cloud-hypervisor process management, TAP setup, cloud-init ISO; also PodService   |            |
| **mvirt-zfs**         | ZFS volume CRUD, snapshots, template imports (HTTP/file → zvol)                   |            |
| **mvirt-ebpf**        | eBPF-based DHCP/ICMP/NAT, security groups, overlay routes (replaces mvirt-net)    |            |
| **mvirt-one**         | Init (PID 1) inside MicroVMs hosting containers; vsock gRPC                       |            |
| **mvirt-log**         | Centralized audit log service (gRPC, redb-backed, raft-replicated for HA)         |            |
| **mvirt-shipper**     | Journald → mvirt-log forwarder, runs alongside mvirt-node                         |            |
| **mvirt-cli**         | Terminal client + ratatui-based TUI                                               |            |
| **mvirt-ui**          | React + Vite + Tailwind web UI                                                    |            |
| **mvirt-daemon-protos** | Shared proto bindings for vmm/zfs/net (used by cplane clients + node proxies)   |            |

### Resource model

mvirt's resources mirror the Kubernetes shape: declarative `Spec`s, observed `Status`, lifecycle `Phase`s.

| Resource         | Project-scoped | Node-bound | Notes |
|------------------|----------------|------------|-------|
| Project          | (root)         | no         | Tenant boundary |
| Network          | yes            | no         | L3 with optional IPv4/IPv6 prefixes, DHCP, DNS, NTP |
| NIC              | yes            | implicit (via VM) | Member of one network; attached to ≤1 VM |
| SecurityGroup    | yes            | no         | Firewall rules, attached to NICs |
| Volume           | yes            | yes        | ZFS-backed; may clone from a template; carries phase |
| Template         | yes            | yes        | Image source for volume clones; phase tracks import progress |
| VM               | yes            | yes        | cpu/memory/volume/nic/image; carries phase |
| Snapshot         | yes (via volume) | yes      | Point-in-time on a volume |
| Pod *(future)*   | yes            | yes        | Container inside a MicroVM, hosted by mvirt-one |

"Node-bound" resources (volume, template, VM, snapshot) carry an explicit `node_id` — mvirt is shared-nothing for storage and shared-nothing for compute. The scheduler picks the node at create time; reconciliation drives that specific node.

### Communication

Three planes, distinct in role:

1. **User plane** — REST/JSON over HTTP to `mvirt-cplane :8080`. Public; the only surface clients ever see.
2. **Cluster plane** — Raft gRPC between cplane peers on port 6001. Internal; replicates `Command`s and snapshots the state machine.
3. **Host plane** — Reverse-tunneled gRPC. Each `mvirt-node` dials its cplane peer on port 50056 (outbound, NAT-friendly), and over that single TCP connection the cplane drives all per-host operations as a normal gRPC client. The node hosts `NodeAgent` (lifecycle/events) plus byte-level HTTP/2 proxies for the local daemons. Detail in ADR-0003.

Out of band: every component logs to `mvirt-log` directly via gRPC; host journald is shipped via `mvirt-shipper`. Log shipping is intentionally independent of the host plane — it must keep working when the agent is unhealthy.

### Design principles

- **Single source of truth.** All cluster-wide desired state lives in the raft-replicated state machine on the cplane peers. Nodes hold no orchestration state.
- **Daemons own their truth.** Each per-host daemon is the authority on its own concrete resources (running VMs, ZFS datasets, eBPF maps). The cplane *orchestrates* but never duplicates this state. Daemons are independently restartable.
- **Level-triggered reconciliation.** Events drive convergence quickly; periodic resync (30s) ensures correctness when events are missed. Reconcilers are idempotent.
- **gRPC + protobuf throughout.** No REST or JSON over the wire between internal components. One IDL, one transport, one mental model.
- **NAT-friendly outbound from nodes.** Nodes never need an inbound port. Operators can deploy nodes anywhere they have outbound TCP to the cplane.
- **Static binaries (musl) + NixOS.** Reproducible builds, drop-in deployment, no shared-library version skew.
- **MicroVMs over containers.** cloud-hypervisor + direct kernel boot for fast, isolated guests; containers run *inside* MicroVMs (mvirt-one), not on the host.
- **eBPF over bridges.** Per-host network is in-kernel via eBPF programs, not Linux bridges + iptables.

## Alternatives considered

- **Build on Kubernetes** (operator pattern). Kubernetes manages *workloads*, not hypervisors — its resource model and scheduler are tuned for ephemeral pods, not persistent VMs with attached storage and complex networking. Forcing mvirt into a CRD shape would either fight k8s's model or replicate so much of its own machinery that the dependency adds nothing.
- **Build on Proxmox** (extend with custom services). Proxmox doesn't expose a clean declarative API and assumes a different operational model (perl + datacenter clustering); we'd be writing as much glue as we'd save in core functionality.
- **Single-node only, ditch consensus.** Considered for early bring-up but rejected — the deployment target is multi-host clusters and "later add HA" is consistently more expensive than designing for it from the start. Single-node mode (`--dev`) is preserved for development.
- **Hand-rolled REST → IPC bus → daemons** (like libvirt/kvm). Workable but loses the rich type system gRPC gives us; we'd reinvent half of protobuf.

## Consequences

- The cplane is the single bottleneck for orchestration. Loss of all cplane peers freezes new resource creation but doesn't affect already-running VMs (daemons keep running). HA = run cplane on 3 or 5 machines.
- Adding a new resource type touches every layer: new commands + state-machine handler + REST route + reconciler + UI. The pattern is repetitive but predictable.
- The system has many crates (10+ at this writing). Coordination across them — proto changes, version bumps, NixOS module updates — is non-trivial. The shared `mvirt-daemon-protos` crate exists specifically to keep one ground truth for daemon-side message types.
- Static linking (musl) + NixOS lock us into Linux. Acceptable: cloud-hypervisor and KVM are Linux-only anyway.
- The "daemons own their truth" principle means the cplane has to *ask* daemons what's actually running rather than rely on its own mirror. This is correct (eventual consistency under daemon restart) but adds RPC traffic during reconciliation.
