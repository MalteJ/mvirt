# 0005 — Cluster: a named group of Nodes within an Org

- **Date:** 2026-05-10
- **Status:** Accepted

## Context

ADR-0004 introduced the Org → Project tenancy hierarchy and committed to a `Cluster` entity in its "Forward compatibility" section without nailing down the shape. This ADR settles the shape.

Today, Nodes register with the cplane (per ADR-0003) and the scheduler picks one for each new resource based on capacity alone — every Node is equally eligible. That doesn't work as soon as more than one set of Nodes exists in the same deployment:

- An Org wants its production VMs on a specific set of hardware (different physical site, separate failure domain, certain hardware class) and its staging VMs on a different set.
- Different Orgs want their resources kept on disjoint hardware (multi-tenant deployment).
- The operator wants to add new Nodes to the deployment without those Nodes immediately being eligible to host every tenant's workload.

The straightforward answer is a `Cluster`: a named, explicitly-listed subset of Nodes that acts as a placement target. Resources pick a Cluster; the scheduler picks a Node within it.

Scope decisions, decided by the user in advance of this ADR and recorded here:

1. **Cluster is Org-scoped, not Project-scoped and not platform-scoped.** Every Cluster belongs to exactly one Org. An Org may have many Clusters (typical case: one per physical site / availability zone). Cross-Org sharing is out of scope for v1 — see "What we explicitly defer".
2. **Multiple Clusters per Org from day one.** An Org with one Cluster is a degenerate case of the general shape, not a separate code path.
3. **Cluster slug is platform-wide unique.** Same rule as Project (ADR-0004): the slug is the durable handle that audit logs, dashboards, and runbooks reference. Two Orgs can't both have a Cluster `west-1`; the operator differentiates as `acme-west-1` / `beta-west-1`.

## Decision

### Schema

```rust
struct ClusterData {
    /// URL identifier — kebab-case, platform-wide unique, immutable.
    slug: String,
    /// FK to the parent Org. Immutable: moving a Cluster between Orgs is
    /// out of scope for v1.
    org_slug: String,
    /// Mutable display name.
    name: String,
    /// Optional free-text description.
    description: Option<String>,
    /// Optional free-text site / region label ("frankfurt-rack-3",
    /// "us-east-1a", …). Not parsed by the platform.
    location: Option<String>,
    /// Explicit list of Node ids that belong to this Cluster. A single Node
    /// may appear in more than one Cluster — useful for hardware that
    /// straddles roles (NVMe + GPU). The reconciler decides per resource
    /// where to actually land it; the Cluster is just the eligibility set.
    node_ids: Vec<String>,
    created_at: String,
    updated_at: String,
}
```

Slug rules match the project-slug rules in ADR-0004: kebab-case, lowercase letters / digits / hyphens, no leading or trailing hyphen, ≤63 chars, immutable.

### Membership / authorization

Cluster management (CRUD on the Cluster row, adding/removing Nodes) is governed by the Cluster's parent Org. Anyone with `org-admin` in the parent Org may manage its Clusters; platform-admin overrides as usual.

A separate `MembershipScope::Cluster { cluster_slug }` is reserved but **not** used in v1. We can add Cluster-level roles (cluster-admin / cluster-member) later if hardware delegation within an Org turns out to need finer control than org-admin gives.

Resource *placement* on a Cluster is governed by the Project's parent Org: any user with project-level create permission may place resources on any Cluster of the Project's Org. There is no separate "can this user pick this Cluster" check — if the Cluster is in the same Org as the Project, it's eligible.

### URL shape

Mirrors the Project pattern: Org-scoped at the create boundary, flat below.

```
# Org-scoped (Cluster lives inside an Org; create + list)
GET    /v1/orgs/:org-slug/clusters             # list within this Org
POST   /v1/orgs/:org-slug/clusters             # create under this Org

# Flat (Cluster slug is platform-unique; everything else by slug)
GET    /v1/clusters                            # cross-Org list (filtered by membership)
GET    /v1/clusters/:slug                      # get
PATCH  /v1/clusters/:slug                      # update name / description / location
DELETE /v1/clusters/:slug                      # delete (rejects if resources reference it)

# Node-membership management — explicit add / remove rather than whole-list PUT
POST   /v1/clusters/:slug/nodes/:node-id       # add node
DELETE /v1/clusters/:slug/nodes/:node-id       # remove node
```

`POST /v1/orgs/:slug/clusters` is the only Cluster-create route; the parent Org comes from the URL, not the body. Mirrors `POST /v1/orgs/:slug/projects`.

### Resource placement

VM / Volume / Template / Network creation today carries a `node_id` (Shared Nothing per ADR-0002). With Cluster:

- Resource specs gain a `cluster_slug: String` field (the Cluster the resource is placed in).
- The `node_id` field stays on the resource's *status* — it's now derived: the scheduler picks a Node from the Cluster's `node_ids` at create time and writes the chosen Node into status, the same way it does today.
- Cluster mismatch is rejected: if a VM is in a Project owned by Org A, its `cluster_slug` must reference a Cluster also owned by Org A. Validation lives in the apply handler.

Migration: existing resources with `node_id` only (no `cluster_slug`) keep working unchanged. New resource-creation paths require `cluster_slug`. Backfill of legacy resources to a synthetic per-Node Cluster is **not** done — the dev clusters that predate this ADR will be wiped (ADR-0004's pre-launch posture stands).

### Node lifecycle vs. Cluster membership

- Nodes register with the cplane as today (`NodeAgent.Identify` via reverse tunnel, see ADR-0003).
- A freshly-registered Node is **unassigned** — it appears in `list_nodes` with no Cluster references and **cannot host resources**.
- `POST /v1/clusters/:slug/nodes/:node-id` adds the Node to the Cluster, validating that the Node is registered.
- `DELETE /v1/clusters/:slug/nodes/:node-id` removes it. Rejects with 409 if the Cluster has resources placed on that specific Node (status.node_id == node-id). The admin must drain first.
- Deregistering a Node implicitly removes it from every Cluster, but is rejected if any resource still references the Node — same drain-first rule.

### Cluster deletion

Reject with 409 if any resource (VM, Volume, Template, …) has `cluster_slug = this`. The admin must delete or migrate resources first; no automatic cascade. Same shape as Org-with-Projects in ADR-0004.

### Reconciler integration

Reconcilers continue to dispatch by `status.node_id` (chosen by the scheduler at create time). The Cluster is only consulted at scheduling time. Once a resource has a Node assigned, the reconciler doesn't care which Cluster it came from. This keeps the existing reconciler loop unchanged.

If a Node leaves the Cluster while a resource is still running on it, the resource keeps running — it's bound to the Node, not the Cluster. New resources can no longer land there until the Node is re-added.

## Alternatives considered

- **Project-scoped Clusters.** Rejected. The motivating case — "this customer's hardware" — is satisfied by giving the customer their own Org plus its Clusters. Adding a Project-scoped Cluster kind opens the question of cross-Project sharing within an Org (which Projects can see which Clusters?) without giving us anything we can't model with Org-scope.

- **Platform-scoped (global) Clusters.** Rejected for v1. The "general-purpose pool" case is satisfied by an operator-owned Org with Clusters. Cross-Org sharing of Clusters — if it ever becomes a real requirement — lands as a separate sharing relation, not as a third Cluster scope.

- **Cluster slug per-Org unique (instead of platform-wide).** Rejected. Matches Project's reasoning in ADR-0004: the slug is the durable identifier in audit logs, runbooks, IaC, dashboards. Platform-wide uniqueness means those external references don't need to also carry the Org slug to disambiguate. Cost: a small cross-Org leak at create-time (the user gets a 409 informing them the slug is taken somewhere they may not see). Same trade-off we accepted for Project.

- **Mutable `org_slug` (move Cluster between Orgs).** Rejected. The set of Projects that may place resources on the Cluster is derived from the Org; flipping the Org would silently re-scope every resource's eligibility. If the use case ever appears, it's a transfer flow with explicit consequences, not a PATCH.

- **Mutable Cluster slug.** Rejected. Same reasoning as Project / Org slugs.

- **Cluster as a label selector over Nodes (k8s NodeSelector style)** instead of an explicit `node_ids` list. Rejected. The k8s pattern is powerful but pays for it in surprise behavior: a Node gaining a label silently joins every matching Cluster. The explicit list is easier to reason about; if we want selector semantics later they can land as a "this Cluster's node_ids are computed from label X" mode without changing the data model.

- **Auto-create a "default" Cluster per Org.** Rejected. Same reasoning as auto-create-default-Org in ADR-0004: the cost of "always-present" code paths (when does it auto-create? what if the user wants two Clusters?) outweighs the one explicit `mvirt cluster create` step.

- **One Node belongs to exactly one Cluster.** Considered for the cleaner mental model. Rejected because hardware roles do overlap in practice (a beefy host could legitimately serve both a `gpu-pool` Cluster and a `nvme-pool` Cluster); the explicit list already gives us the override and there's no enforcement we'd want to bake in.

- **Resource references a Node directly, Cluster is only a placement *hint*** (scheduler picks any Node in any Cluster matching some criteria). Rejected. Picking a Cluster at create time and writing the chosen Node into status is exactly what the existing scheduler does — just gated by a smaller candidate set. Keeping the "resource → Node" hard binding preserves the reconciler's contract (ADR-0002 §1).

## What we explicitly defer

- **Project-scoped Clusters and platform-scoped Clusters.** See alternatives. If the cross-Org sharing case becomes real, it gets its own ADR with the sharing relation, not a new Cluster scope.
- **Cluster-level memberships (`MembershipScope::Cluster`).** Reserved in ADR-0004's enum, not implemented in v1. org-admin manages all Clusters in the Org. Add when hardware-delegation use case appears.
- **Label-selector node membership.** v1 is explicit-list only. If label-selectors land later, the `node_ids` field becomes derived; the schema otherwise unchanged.
- **Cluster-level scheduling policy** (anti-affinity, spread-vs-pack, dedicated-host). Today's scheduler is capacity-aware only; that doesn't change here. Per-Cluster policy lands when we have a concrete second case beyond capacity.
- **Moving Clusters between Orgs**. Use create-new-cluster + migrate-resources + delete-old-cluster, like Project moves.
- **Backfilling pre-Cluster resources** to a synthetic Cluster. Pre-launch state gets wiped on schema change (per ADR-0004); no migration code.
- **Cluster-level quotas** (e.g. "Project A may use ≤50% of this Cluster"). Not in v1; ADR-class addition when a real number is needed.

## Consequences

- **Org gains a 1:N child entity.** API and UI both get `/v1/orgs/:slug/clusters` (list + create) and a `/v1/clusters/:slug` per-Cluster surface. Symmetric with the Org → Project shape from ADR-0004.
- **Resources gain a mandatory `cluster_slug` on their spec.** Existing creation paths break until updated. Migration on pre-launch deployments is "wipe state", per ADR-0004.
- **Newly-registered Nodes are inert until added to a Cluster.** Previously every Node was immediately eligible for placement. This is a behavior change for operators; mitigated by the create-then-add-nodes flow being short.
- **Node deregistration grows a precondition** (must not be in any Cluster with active resources). The op was already idempotent and rare; this turns a silent foot-gun into an explicit 409.
- **Scheduler keeps its existing capacity-aware logic**, just constrained to the Cluster's `node_ids`. No new algorithms.
- **Reconciler is unchanged.** It dispatches on `status.node_id` as today.
- **Authorizer is unchanged.** Cluster management uses the existing Org-scope roles; Cluster placement is gated by Project-level create permission.
- **The slug `cluster-admin` is still free** because ADR-0004 already renamed the platform-wide super-role to `platform-admin`. We can introduce `cluster-admin` later (per-Cluster role) without another rename round.
