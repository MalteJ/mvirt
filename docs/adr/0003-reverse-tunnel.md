# 0003 — Reverse tunnel between node and cplane

- **Date:** 2026-05-08
- **Status:** Accepted

## Context

mvirt-node sits on each hypervisor host and historically was the gRPC *server* — it served `NodeService.Sync`, a bidirectional stream that the cplane connected into to push manifests and pull status. Two problems:

1. The cplane had to be able to *reach* every node. Behind NAT or a firewall (which is the common deployment), this is awkward — operators would have to expose every node, set up VPN tunnels, or run discovery infrastructure.
2. Reconciliation ran on the node. Every per-resource decision required an out-of-band fetch of cluster state from the cplane via the manifest stream, and the node had to re-implement scheduling-adjacent logic to use that state correctly. The node became a small distributed controller of its own.

We wanted: NAT-friendly outbound-only connection from the node, with the cplane in the driver's seat (and reconciliation following — see ADR-0002).

## Decision

The TCP direction inverts: the node dials the cplane (outbound, NAT-friendly). The gRPC direction also inverts: on the dialed socket, the *node* hosts the gRPC server, the *cplane* is the gRPC client. Tonic supports this directly — `Server::serve_with_incoming(stream)` accepts any stream of `AsyncRead+AsyncWrite`, and `Endpoint::connect_with_connector(conn)` accepts any tower service that returns one. So both ends of a single TCP socket can host whichever role we want.

Auth is intentionally absent for now; mTLS belongs in a later ADR.

## Component communication

```
                  ┌─────────────────┐                ┌─────────────────┐
   user, UI ──────►   mvirt-cplane  │ ◄── Raft ─────►   mvirt-cplane  │
   REST /v1/...   │   (this binary) │   port 6001    │   (peer leader) │
   port 8080      └────────┬────────┘                └─────────────────┘
                           │
                           │ accepts reverse tunnel (port 50056)
                           │ inverted gRPC: cplane = client
                           ▼
                  ┌─────────────────┐
                  │   mvirt-node    │  hosts on the dialed socket:
                  │  (per host)     │    • NodeAgent
                  └────────┬────────┘    • VmService / PodService  ┐
                           │             • ZfsService              │ byte-level
                           │             • NetService              │ HTTP/2 proxies
                           │                                       ┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
       ┌───────────┐ ┌──────────┐ ┌──────────┐
       │ mvirt-vmm │ │ mvirt-zfs│ │mvirt-ebpf│  local daemons
       │   :50051  │ │   :50053 │ │  :50054  │  loopback gRPC
       └─────┬─────┘ └──────────┘ └──────────┘
             │ vsock
             ▼
       ┌───────────┐
       │ mvirt-one │  PID 1 inside MicroVM
       └───────────┘

  All daemons + cplane ──audit log──►  mvirt-log :50052
  journald (host) ──mvirt-shipper──►  mvirt-log :50052
```

### User / UI plane
- **mvirt-cli, mvirt-ui** → **mvirt-cplane**: REST/JSON over HTTP, port 8080. The only public surface.

### Cluster plane
- **mvirt-cplane** ↔ **mvirt-cplane** (peers): Raft gRPC via `mraft`, port 6001. Leader election, log replication of `Command`s, snapshots.

### Reverse tunnel (the inverted bit)
- **mvirt-node** dials TCP to the cplane on port 50056 (NAT-friendly outbound).
- After the TCP connect, the cplane builds a tonic `Channel` from the accepted socket via `Endpoint::connect_with_connector`. It calls `NodeAgent.Identify(...)` to learn the node's id, registers a `NodeHandle` (with typed daemon clients sharing the same `Channel`) in the global `NodeRegistry`, then opens `NodeAgent.WatchEvents` as a long-lived server-streaming RPC. That stream is the liveness signal — when it errors, the node is removed from the registry.
- The node side wraps the dialed socket as `stream::once(Ok(sock)).chain(stream::pending())` and runs `Server::serve_with_incoming(...)`, hosting four sets of services on it:
  - **NodeAgent** (real impl on the node): `Identify`, `WatchEvents` (server-streaming, node pushes asynchronous events upward), `CurrentResources` (scheduler input).
  - **VmService + PodService** (`mvirt.VmService`, `mvirt.PodService`): byte-level HTTP/2 proxy to `mvirt-vmm` on `[::1]:50051`.
  - **ZfsService** (`mvirt.zfs.ZfsService`): proxy to `mvirt-zfs` on `[::1]:50053`.
  - **NetService** (`mvirt.net.NetService`): proxy to `mvirt-ebpf` on `[::1]:50054`.

  Proxies forward at the HTTP/2 layer (one `tower::Service<Request<Body>>` impl per service name, all delegating to a shared hyper-util `Client`) so streaming RPCs work transparently and the node has no compile-time knowledge of the daemon protos.

### Local plane (per node, loopback only)
- **mvirt-node proxies** → **mvirt-vmm / mvirt-zfs / mvirt-ebpf**: gRPC over loopback. The node never originates these calls itself — it only forwards on behalf of the cplane.
- **mvirt-vmm** ↔ **mvirt-one**: vsock gRPC `OneService`. Pod lifecycle inside MicroVMs.

### Audit + observability (out of band)
- **All daemons + cplane** → **mvirt-log** (port 50052): direct gRPC `LogService.Log` calls via the `AuditLogger` helper.
- **journald (per host)** → **mvirt-log**: forwarded by **mvirt-shipper**, which runs alongside mvirt-node and is independent of the reverse tunnel — log shipping must keep working when the agent is unhealthy.

## Alternatives considered

- **Frame-mux protocol over a single bidi-stream.** Define a single bidi-stream RPC, mux RPC requests/responses inside it. Rejected as unnecessary mechanism — tonic's role-swap pattern gets us standard gRPC services on each end with no custom mux.
- **Two TCP connections (one per direction).** Rejected: doubles connection state, doesn't actually buy anything since HTTP/2 is already full-duplex multi-stream.
- **Typed daemon-service forwarders on the node** (impl every gRPC trait by hand and call the local typed client). Rejected: ~300 LOC of mechanical boilerplate, awkward for streaming RPCs, and pins the node to the daemon proto schema. The byte-level proxy gets streaming for free and decouples node from daemon-proto changes.

## Consequences

- Node is now firewall-friendly: only outbound TCP needed.
- Cplane → node RPCs are standard typed gRPC clients; reconcilers look exactly like they would if dialing a normal server.
- Node-originated events flow up via `WatchEvents` server-streaming — natural fit for asynchronous notifications, also doubles as liveness signal (stream errors when tunnel breaks).
- A single TCP socket per node carries everything (NodeAgent + 4 daemon services). When that socket dies, all gRPC clients on the cplane side fail together — clean failure semantics.
- The node has no compile-time knowledge of vmm/zfs/net protos. New RPCs in those daemons are reachable by the cplane immediately, no node redeploy.

## Bugs found while implementing

- `stream::once(Ok(socket))` on the node side caused tonic's `serve_with_incoming` to exit the accept loop the moment the once-stream was exhausted, even though the spawned per-connection task was still alive. Fix: chain `stream::pending()` after the once. The misdiagnosis cost a detour through hyper internals chasing a phantom idle-disconnect.
