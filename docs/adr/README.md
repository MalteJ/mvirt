# Architecture Decision Records

Each ADR describes a major component or design choice — not just *what* changed, but the concept behind it: the role the component plays, how it fits into the system, why it's shaped the way it is, and what alternatives we rejected.

Keep it broad enough to be useful as a design document, narrow enough to stay coherent. If a later decision overrides an earlier one, write a new ADR that supersedes the old one rather than rewriting history.

## Format

Each file: `NNNN-short-title.md`. Sections:

- **Status** — Accepted / Superseded by NNNN / Deprecated
- **Context** — The problem and the constraints we were working under
- **Decision** — What we chose, including the design at sufficient depth that a new contributor can use it as a reference
- **Alternatives considered** — What else we looked at and why we didn't pick it
- **Consequences** — What this commits us to (good and bad)

## Index

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-mvirt-overall-architecture.md) | mvirt: overall architecture | Accepted |
| [0002](0002-control-plane.md) | mvirt-cplane: the control plane component | Accepted |
| [0003](0003-reverse-tunnel.md) | Reverse tunnel between node and cplane | Accepted |
| [0004](0004-rest-api-auth.md) | Identity, authentication, authorization, and tenancy | Accepted |
| [0005](0005-cluster.md) | Cluster: a named group of Nodes within an Org | Accepted |
| [0006](0006-node-onboarding.md) | Node onboarding: cluster-bound tokens, mTLS, internal CA | Accepted |

Start with 0001 for the big picture, then 0002 for the cplane internals, then 0003 for how the cplane talks to the per-host daemons. 0004 covers user-facing identity, authorization, and the Org → Project tenancy hierarchy (rauthy as bundled IdP, unified Account model, ServiceAccount credential variants, polymorphic Memberships). 0005 layers Cluster under Org as the placement target for resources. 0006 settles how a fresh hypervisor host actually joins a Cluster: the operator mints a single-use token bound to a specific Cluster, the node redeems it for a client cert from the cplane's internal CA, and the reverse tunnel from then on runs mTLS with `(node_id, cluster_slug)` carried in cert claims.
