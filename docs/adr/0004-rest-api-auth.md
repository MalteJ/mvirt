# 0004 — Identity, authentication, authorization, and tenancy

- **Date:** 2026-05-10
- **Status:** Accepted

## Context

mvirt-cplane exposes a REST API on port 8080 — the only public surface for users, the web UI, the CLI/TUI, and any external automation. To ship a multi-tenant cluster product we need:

- **Multi-tenancy from day one.** Resources (VMs, volumes, NICs, project-private networks) belong to projects, projects belong to organizations, and users see only what they are entitled to.
- **A first-class organization layer.** Hosting providers, enterprises with internal tenant separation, and per-tenant policy knobs all need an `Org` to attach to. Adding it retroactively means re-keying every Project, every URL, every audit-log entry — strictly more work than putting it in on day one.
- **Both homelab and enterprise** as supported operator profiles. A homelab user must not need to shop for an IdP; an enterprise user must be able to plug their existing one in.
- **Air-gap-tauglich.** No mandatory dependency on a SaaS identity provider.
- **O(1) credential revocation.** Multi-tenant means kicking out a service account or compromised token is a single state-machine command, not "wait for TTL".
- **A bootstrap path** that fits IaC: no manual DB edits, no copy-paste from container logs.

The tempting shortcut in a single-binary cluster manager is to build local username+password auth and skip OIDC entirely. We rejected that path on cost-vs-CVE analysis. "Local password done right" requires:

- argon2id with safe parameters (memory_cost, time_cost, parallelism — wrong values mean either insecure or DoS-prone)
- rate-limiting on login, plus account lockout with constant-time responses to avoid user enumeration
- password reset flow: token issuance, email delivery, replay protection, expiry
- email infrastructure: SMTP config, deliverability, DKIM/SPF, bounce handling
- email verification on registration
- CSRF on every state-changing endpoint
- session lifecycle: idle timeout, absolute timeout, "log out from all devices"
- compliance knobs: complexity, expiry, no-reuse-of-last-N
- account recovery when the email is gone (recovery codes? admin override?)
- MFA (TOTP, recovery codes, WebAuthn)

That's roughly 4–6 weeks for a solo developer plus ongoing CVE-class maintenance — exactly the cost that pushed every other product into outsourcing identity. mvirt-cplane is meant to be a cluster manager, not an identity product.

The other major question was how ServiceAccounts authenticate. Static API tokens issued by mvirt are simple and work for homelab CI, but they block the modern workload-identity pattern (GitHub Actions OIDC, AWS IRSA, GCP Workload Identity Federation) where no shared secret ever exists.

Three identity providers were the test bed for "is OIDC really pluggable?": rauthy, Zitadel, Keycloak, Auth0, and Microsoft Entra ID. All speak OIDC core; their differences live in issuer-pattern matching (Entra) and discovery-URL shape, plus per-provider claim semantics if you wanted to pull authorization data out of the IdP — which is exactly the trap we want to avoid.

The last big shape decision was whether `Account` should be cluster-wide or per-Org. Per-Org Accounts give SaaS-grade tenant isolation at the identity layer (the same OIDC `sub` in two Orgs is two separate Accounts, two separate audit trails, no cross-Org visibility). The cost is that login becomes "which Org are you signing in to?" — an awkward pattern for users who legitimately belong to multiple Orgs (a contractor, a platform team) and a UX cliff for the single-Org case. Cluster-wide Accounts with Org-scoped *memberships* solve the same problem without the IdP-routing question.

## Decision

### Tenancy hierarchy: Org → Project → Resources

Every Project belongs to exactly one `Org`. Every resource (VM, Network, NIC, Volume, Template, SecurityGroup, ServiceAccount) belongs to exactly one Project. There is no "Org-less Project" state, no auto-created default Org, and no "single-org mode" that hides the Org layer.

- A fresh cplane starts with **zero Orgs and zero Projects.** The first thing the platform-admin does, after the OIDC bootstrap completes, is `POST /v1/orgs` — via UI, CLI, or IaC.
- The UI **always** shows the Org context (in breadcrumbs, switcher, and metadata), even when only one Org exists. No hide-the-switcher branch.
- The Org appears in URLs *only at the Org boundary* — Org-management routes and Project listing/creation under an Org. Once a Project is identified, the URL drops back to flat (`/v1/projects/:slug/...`). Full route table in [ADR-0002, Subsystem 2](0002-control-plane.md#subsystem-2-rest-api).

The cost of this rule is one explicit `mvirt org create` step in any deployment script before the first project. The payoff is no config flag, no default-Org auto-creation logic, no UI conditional — every line of which would be a place for a tenant-isolation bug to hide.

#### Org schema

```rust
struct OrgData {
    id: String,                              // uuid v4
    slug: String,                            // unique platform-wide, kebab-case, URL-safe
    name: String,                            // mutable display text
    default_static_key_ttl_days: u32,        // default 90 (see ServiceAccount credentials)
    disallow_static_keys: bool,              // default false (see ServiceAccount credentials)
    created_at: String,
    updated_at: String,
}
```

Slug is the URL identifier and is **immutable after creation** (changing it would break audit-log links and bookmarked URLs). Name is mutable display text.

Project gains `org_id: String` (NOT NULL). `CreateProject` requires the parent Org explicitly (via the URL — `POST /v1/orgs/:slug/projects`).

#### Resource scoping

Resources stay Project-scoped via `project_id`. We do **not** denormalize `org_id` onto every resource:

- The Project → Org mapping is stable in v1 (Projects do not move between Orgs).
- The Authorizer needs `org_id` per resource for the Org-scope cascade, but it's a one-extra-hash-lookup at check time — well below the cost of a redb read.
- Adding `org_id` columns to seven resource tables is exactly the migration we want to avoid retroactively. We can add it later as a denormalization optimization if query patterns demand it; nothing in v1 needs it.

#### Projects are namespaces

The mental model maps directly onto Kubernetes:

- **Project = namespace.** Every project-scoped resource lives in exactly one Project. Project slugs are platform-wide unique, just as K8s namespace names are cluster-wide unique.
- **Resource name uniqueness is per-Project.** `web` is a perfectly valid VM name in any number of Projects simultaneously, the same way `web` can be a Pod name in any number of K8s namespaces.
- **Org sits *above* the namespace** — a billing/policy/membership container that owns a set of namespaces but doesn't appear in resource keying.

| Entity                                | Uniqueness scope                                          |
|---------------------------------------|-----------------------------------------------------------|
| `Org` (slug)                          | Platform-wide                                             |
| `Project` (slug — the namespace name) | Platform-wide                                             |
| `Volume`, `SecurityGroup` (name)      | Per-Project                                               |
| `VM`, `Network`, `Template` (name)    | Per-Project                                               |
| `Account` (User and ServiceAccount)   | Per `(iss, sub)` (User) or per `(project_id, name)` (SA)  |
| `Node`                                | Platform-wide                                             |

Because the Project slug alone resolves a Project, the URL **drops the Org segment** below the Org boundary — Org-scoped routes only at the Org-management surface, flat at Project and below. (Full route table in [ADR-0002, Subsystem 2](0002-control-plane.md#subsystem-2-rest-api).)

The cost is a small cross-Org leak: an org-admin attempting to create a Project with a slug already taken in an Org they cannot see gets a 409, learning that the slug exists somewhere. We accept this trade because platform-wide-unique Project slugs make audit-log entries and external references (Grafana dashboards, runbooks, IaC) decoupled from Org renames or moves — the namespace name is the durable identifier.

### Identity provider: bundled but swappable

- **mvirt-cplane is an OIDC client only.** No password handling, no email infrastructure, no MFA code in cplane. The cplane validates JWTs against a configured issuer's JWKS and trusts the issuer for human auth.
- **Default deployment bundles [rauthy](https://github.com/sebadob/rauthy)** as the IdP: a ~20MB Rust binary with embedded SQLite/Hiqlite that ships all the password/MFA/reset/email features standalone. Initialization is fully declarative — bootstrap admin email + OIDC client config via env vars and config files, no UI clicks.
- **Operators with an existing IdP swap rauthy out** by replacing the IdP container and pointing `MVIRT_OIDC_ISSUER` at their issuer. Tested provider profiles: rauthy (default), Zitadel, Keycloak, Authentik, Auth0, Microsoft Entra ID.
- **Multiple OIDC providers in parallel** is supported by the `(issuer, sub)` keying — no name collisions across providers.

### Account model: a single Subject type

- One `Account` row per identity. `kind: User | ServiceAccount` discriminates, but **handlers and authorization checks never branch on kind**. Membership-based access control treats both uniformly.
- **User**: a human. Identity key is `(external_iss, external_sub)`, never email — emails change, `sub` is stable per IdP. Cached `email`, `display_name` from claims for UI. Lazy-created on first successful OIDC login if pre-provisioning was not used.
- **ServiceAccount**: a machine. Lives inside a Project (`project_id` is the SA's "home" — lifecycle parent) or is platform-scoped (`project_id = NULL`, rare; reserved for SAs used by the platform itself). Credentials stored in mvirt, **never in the IdP**.
- **Memberships are polymorphic** over a discriminated scope:

  ```rust
  enum MembershipScope {
      Platform,                        // applies to the entire mvirt deployment
      Org { org_id: String },          // applies to the Org and all its Projects
      Project { project_id: String },  // applies to a single Project
  }

  struct Membership {
      account_id: String,
      scope: MembershipScope,
      role: Role,
      created_at: String,
  }
  ```

  Stored as a single redb table keyed by `(account_id, scope_kind, scope_id)`. One polymorphic table keeps the Authorizer and audit logic uniform; handlers and authorization checks never branch on scope kind. The same shape covers User and ServiceAccount memberships identically.

### ServiceAccount credentials: pluggable, multiple per SA

Each SA can have N credential rows. Three kinds, listed by security from strongest:

1. **FederatedTrust** (recommended for production)
   No secret stored in mvirt. The credential is a *trust policy*: "accept JWTs from issuer X if claims match Y". The actual token is short-lived and issued at runtime by an external system — GitHub Actions OIDC, AWS IRSA, GCP Workload Identity Federation, or the operator's own IdP for IdP-managed service users. mvirt validates signature against the issuer's JWKS and matches required claims (e.g. `repository`, `ref`, `sub`). No exfiltrable secret exists.

2. **SignedJwt**
   SA holds an ed25519 private key, mvirt stores the public key. Service signs short-lived JWTs locally; mvirt verifies. Slightly worse than FederatedTrust because the private key has to be deployed somewhere, but better than StaticApiKey because the key was never on mvirt's side and need not transit on every request.

3. **StaticApiKey**
   mvirt generates a random secret, displays once at creation, stores BLAKE3 hash. Convenient for homelab CI and local scripts. Default expiry comes from the parent Org's `default_static_key_ttl_days` (default 90). Rotation via "create new credential, both valid for grace period, old auto-revoked". A per-Org policy can disable static keys outright (`disallow_static_keys: true` on `OrgData`) for stricter setups.

The SA itself is identity; credentials are how it proves that identity. Same SA can have a FederatedTrust for production CI and a StaticApiKey for emergency manual scripting.

### No human-bound PATs

Humans authenticate exclusively via OIDC interactive flows. No human-owned long-lived API tokens.

- **Browser**: standard Authorization Code + PKCE → mvirt-issued opaque session token (cookie as transport, but handlers see Bearer).
- **CLI**: Authorization Code + PKCE with localhost loopback redirect (the `gh auth login` / `gcloud auth login` pattern). Headless: Device Code (RFC 8628). Refresh token stored in `~/.config/mvirt/credentials.json` (mode 0600).
- For automation a human would have used a PAT for, they create a project-scoped ServiceAccount instead. Lifecycle is owned by the project, survives the human leaving.

This avoids the "departing employee → orphaned tokens" pattern and keeps machine credentials separate from human lifecycle.

### Authorization

- **Membership = `(account_id, scope, role)`.** This is the only authorization input. v1 has hardcoded roles per scope:

  | Scope    | Roles                                                 | Notes |
  |----------|-------------------------------------------------------|-------|
  | Platform | `platform-admin`, `platform-viewer`                   | The mvirt deployment as a whole. Roles are deliberately *not* called `cluster-admin` so that the word `cluster` stays free for the upcoming `Cluster` entity (a named subset of Nodes — see Forward compatibility). platform-admin overrides everything. |
  | Org      | `org-admin`, `org-member`, `org-viewer`               | org-admin manages Projects within the Org and grants Org/Project memberships within the Org. |
  | Project  | `project-admin`, `project-member`, `project-viewer`   | project-admin manages resources within the Project and grants Project memberships. |

- **Roles in higher scopes dominate.** Possessing `org-admin` in Org X grants the `project-admin` permission set in every Project of Org X without explicit Project memberships. platform-admin grants everything everywhere.

- **All authorization decisions go through an `Authorizer` trait** with one `check(ctx, op) -> Decision` method and a `list_authorized_projects(account_id, action)` for filtered list queries. Default impl is `HardcodedRbacAuthorizer` doing membership lookups. Future swaps (OPA via sidecar, Zanzibar-style relation-based, in-house custom roles) re-implement the trait without touching handlers.

- **`Authorizer::check` evaluates in cascade.** For an operation targeting a resource in Project P (which belongs to Org O):

  1. `(account_id, Platform, platform-admin or above-required)` → allow.
  2. `(account_id, Org(O), org-admin or above-required)` → allow.
  3. `(account_id, Project(P), project-admin or above-required)` → allow.
  4. Otherwise → deny.

  Read operations (`*-viewer`) cascade the same way. List operations use `Authorizer::list_authorized_projects(account_id, action)` which unions Project memberships with Projects-of-Orgs-where-account-has-Org-membership and returns everything if the account is platform-admin.

- **`AuthContext` is structured to grow**: today carries `account_id`, tomorrow can carry `request_ip`, `user_agent`, `mfa_level`, `request_time` for PBAC/conditions without API breaks.

- **Resources are Project-scoped via `project_id`**, no per-resource ACLs in v1. Per-resource permissions (e.g. "creator can delete, others cannot") would land as additional fields on the resource, evaluated in handlers — no Authorization-system extension needed.

### Initial admin bootstrap

- **`MVIRT_INITIAL_ADMIN_EMAIL`** (preferred) or **`MVIRT_INITIAL_ADMIN_SUB`** (stricter; immutable) env vars.
- On first successful OIDC login that matches the expected email *and* has `email_verified = true` in the ID token *and* no platform-admin exists yet → cplane grants the freshly-created Account `(scope=Platform, role=platform-admin)`.
- Idempotent: once any platform-admin exists, the env vars are inert. Operator may remove them or leave them harmless.
- The freshly-bootstrapped platform-admin then explicitly creates the first Org and the first Project — there is no auto-bootstrap of either. The state machine starts empty; tests and CI fixtures create them as part of test setup.
- Boot log makes the bootstrap state explicit:
  ```
  No platform-admin found. Will auto-grant platform-admin to the first verified
  login matching email=admin@example.com. Set MVIRT_INITIAL_ADMIN_EMAIL to change.
  ```

### Token validation pipeline

```
incoming request → require_auth middleware
  ├─ Cookie "mvirt_session"             → lookup session, resolve account_id
  ├─ Authorization: Bearer mvirt_sa_*   → lookup api_key by hash, resolve SA account
  ├─ Authorization: Bearer <JWT>        → validate against trusted issuers:
  │     ├─ matches MVIRT_OIDC_ISSUER    → resolve User account by (iss, sub),
  │     │                                 lazy-create if needed
  │     └─ matches a FederatedTrust pol → resolve SA account from trust policy
  └─ → AuthContext { account_id, ... }

→ handler calls authorizer.check(ctx, Operation::Project { project_id, action })
→ allow / deny
```

A single resolution step yields an `Account`, regardless of credential kind. Authorization is downstream and uniform.

### Credential storage

- mvirt session tokens and StaticApiKey: stored as BLAKE3 hashes. The plaintext is never persisted; visible to the user once at creation and never again.
- SignedJwt public keys: stored in plaintext (they're public).
- FederatedTrust policies: stored in plaintext (issuer URL, audience, required claim values).
- Bootstrap email/sub: read from env at runtime, not persisted.

## Forward compatibility: Cluster (a named subset of Nodes)

A planned follow-up entity, **deferred to its own ADR** but flagged here because it shapes the scope hierarchy we commit to today: a `Cluster` is a named, explicitly-listed subset of Nodes (a placement target). Clusters can be:

- **Project-scoped** — owned by one Project, only that Project may place resources on its Nodes. Use case: a customer brings their own hardware.
- **Org-scoped** — visible to every Project in one Org. Use case: an enterprise puts compliance-bound Projects on FIPS-validated hardware.
- **Platform-scoped (global)** — visible to every Project. Use case: the operator's general-purpose pool.

The `MembershipScope` enum extends naturally:

```rust
enum MembershipScope {
    Platform,
    Org { org_id: String },
    Project { project_id: String },
    // Added in the Cluster ADR:
    // Cluster { cluster_id: String },
}
```

…and the cascade similarly grows a Cluster level (admin/member/viewer for managing the Cluster's Node list, picking a Cluster as placement target, etc.). Resource scheduling moves from "any Node" to "any Node in a Cluster the requester is allowed to use", with the existing Project membership still gating which resources the requester can create at all.

The Cluster entity intentionally does *not* land in this ADR — it touches the scheduler, NodeRegistry, and reconciler in a non-trivial way. What we are committing to here is only that the Membership scope and role-naming choices leave room for it without another rename round.

Naming reconciliation: ADR-0001 and ADR-0002 use "the cluster" colloquially for the whole mvirt deployment. With `Cluster` becoming a first-class entity, those documents will need a wording pass when this lands; the role-rename in this ADR (`platform-admin` instead of `cluster-admin`) is the load-bearing change.

## What we explicitly defer

- **The `Cluster` entity itself** — see Forward compatibility. Lands in its own ADR with the scheduler / placement changes.
- **Custom roles beyond the hardcoded set.** The `Authorizer` trait is the swap point. Operators wanting "ops-readonly + restart VMs" granularity will get OPA/Rego integration as a sidecar before mvirt grows its own policy DSL.
- **Per-resource ACLs.** "User A can see VM-1 but not VM-2 in the same project" needs SpiceDB-class infrastructure or per-resource permission rows. Defer until a real use case shows up; meanwhile `created_by_account_id` on resources covers the common "only the creator can delete" case in handler code.
- **Group/role sync from IdP.** Per-IdP claim paths and semantics differ enough (Entra App Roles, Auth0 audience semantics, Zitadel namespaced role claims, Keycloak realm_access.roles) that supporting it generically forces config surface that doesn't earn its keep when authz can live coherently inside mvirt. Reachable later as opt-in.
- **SCIM provisioning.** Heavy; appropriate as enterprise upsell on a later track.
- **Project-move-between-Orgs.** Use create-then-migrate-then-delete in v1.
- **Cross-Org resource sharing.** No "Org A's network shared into Org B's project" in v1. The `is_public` flag on Network is currently platform-wide; it stays that way and gets re-scoped to the owning Org when this is reopened.
- **Org-level quotas and billing.** The Org row has no resource-cap or billing-id columns yet. ADR-class change when we have a concrete shape.
- **Per-Org IdP.** ADR-0004's BYO-IdP is platform-wide — one issuer per deployment. Per-Org IdPs (where Org A federates with Auth0 and Org B with Entra) are a real SaaS feature but require multi-issuer trust and audience routing. Not in v1.
- **Back-channel OIDC logout / RP-initiated logout.** Patchy provider support; short mvirt session TTL is the mitigation.
- **MFA enforcement at the mvirt layer.** Delegate to the IdP — it's their job, and rauthy/Zitadel/Entra all do it well.
- **Schema compatibility with pre-Org snapshots.** mvirt is pre-launch; we do not commit to forward-loading older state-machine snapshots. Developer state.redb files predating this ADR are wiped on upgrade. Documented in CHANGELOG and the dev-cluster ops runbook.

## Alternatives considered

- **Build username+password in mvirt-cplane.** Rejected on cost-vs-CVE analysis (see Context). ~3000 LOC for argon2 + reset + email + MFA + lockout + complexity policies, plus ongoing security maintenance. Bundling rauthy gets all of it with a 20MB RAM container.

- **Don't bundle any IdP, document "configure your own".** Works for tier-2/3 operators but creates a UX cliff for tier-1 (homelab/single-node). Bundling rauthy gives a working out-of-box experience without forcing operators to choose an IdP on day one.

- **Bundle Zitadel instead of rauthy.** Heavier resource footprint (~500MB RAM with Postgres dependency), UI-driven configuration that's awkward to script, slower bootstrap. rauthy fills the same role at ~20MB RAM with fully declarative config. Zitadel remains a perfectly fine *swap* target for operators who want it.

- **Allow human-owned PATs (GitHub-style).** Rejected because the pattern ties machine credentials to human lifecycle. Departing employee → orphaned tokens unless cascade deletion is rigorously enforced, which is exactly the operational pain enterprises avoid. Humans use ephemeral OIDC-backed sessions; machines use ServiceAccounts whose lifecycle is owned by the project.

- **Federated trust only for SAs (no StaticApiKey).** Rejected because FederatedTrust assumes some external workload-identity infrastructure (GitHub Actions, AWS IRSA, etc.). Tier-1 homelab CI scripts running on a NAS or cron job have nothing to federate against; static keys are the pragmatic minimum. Static keys are explicitly second-tier and can be disabled per-org.

- **Service users in the IdP** (Zitadel-style PATs, Auth0 M2M apps) as the canonical SA representation. Rejected because every IdP handles machine identity differently; forcing a particular IdP-side capability would compromise the BYO-IdP promise. SAs as mvirt-side entities work uniformly across all OIDC providers, including IdPs that have no service-user concept.

- **JWT as mvirt's own session/API token** (instead of opaque). Rejected: revocation in a multi-tenant system has to be cheap and immediate. The denylist or short-TTL/refresh choreography you'd need is more code than the opaque-token-lookup it replaces, and we have no edge-validation use case. The OIDC ID-token *is* a JWT — we just don't propagate it past the login boundary.

- **Bootstrap token printed to stderr on first start** (Gitea/Vaultwarden pattern). Workable but the only deployment step that resists IaC: an operator running `terraform apply` against a fresh cluster has to extract a token from container logs before the first action. The `MVIRT_INITIAL_ADMIN_EMAIL` approach is declarative end-to-end and matches the operational model where the operator already knows their own email.

- **Pull groups/roles from the IdP via claim mapping.** Rejected for v1: per-provider claim paths (`groups` / `realm_access.roles` / `urn:zitadel:...` / Entra App Roles), Entra's groups-overage requiring Microsoft Graph fallback, and the IdP-tenant-vs-mvirt-tenant naming clash all bring config surface that doesn't earn its keep when authz can live coherently inside mvirt. Reachable later as opt-in.

- **mTLS for REST API users.** Rejected for the user-facing API: certificate provisioning per browser/CLI user is a UX cliff. mTLS belongs on the internal mesh (reverse tunnel, inter-cplane Raft) and gets its own ADR.

- **Skip Org entirely; "platform = tenant".** Rejected on the retroactive-migration cost (see Context). The cleanest fix for a future "we need Orgs" request is to have built it now; doing it later means a synchronized data, URL, and UI migration across all operators.

- **Hide Org in single-org mode.** Rejected. Means a config flag, two URL shapes with backward-compat redirects, default-Org auto-creation logic, and UI conditionals. The cost of always-visible Org is one explicit `mvirt org create` step before the first Project — cheaper than the always-on/sometimes-on duality.

- **Per-Org Accounts (hard identity isolation per tenant).** Rejected as inconsistent with the one-Account-per-`(iss, sub)` rule. It also breaks the multi-Org-membership use case (contractor working for two tenants, platform team with cross-Org visibility) and forces an "Org selector at login" step that no widely-used IdP supports natively. Tenants needing identity-layer isolation can run separate mvirt deployments — that's a deployment choice, not a data-model choice.

- **Org as a label/tag on Project, not a first-class entity.** Rejected because the per-Org policy knobs (default static-key TTL, `disallow_static_keys`) need a row to attach to, audit logs need a stable Org id to filter by, and cross-Project queries scoped to an Org need an index. A label gets us the URL prefix and nothing else.

- **Allow Projects to belong to multiple Orgs.** Rejected as a query/billing nightmare. Resources have one parent Project; Projects have one parent Org. A "shared resource between Orgs" use case is real but cheaper to model as a Project-level *share*-relation later than to make the parent edge multi-valued now.

- **Denormalize `org_id` onto every resource row.** Rejected for v1 — adds complexity and migration risk for a one-hash-lookup optimization. The one place we'd want it is the Authorizer, where it's a redb point-lookup on the Project, not a table scan. If it ever becomes a hot path we add it as a pure performance change.

- **Drop platform-admin; platform-admin = org-admin in default Org.** Rejected because we already commit to a deployment-wide super-role as a distinct concept (cluster bootstrap, Org-create permission, cross-Org visibility for support cases). Folding it in would re-introduce the "who can create Orgs?" question that platform-admin already answers.

- **Move Projects between Orgs as a v1 feature.** Rejected as out of scope. It needs all the deferred denormalizations to be reversible safely, plus a UX (transfer-confirm flow, audit-log discontinuity policy, billing reconciliation) we have no concrete requirement for. Land it when a real customer asks; until then, "create new project, migrate resources, delete old project" is the documented path.

## Consequences

- **Default deployment is `docker compose up`-ready.** rauthy + mvirt-cplane + mvirt-ui, fully wired, admin email pre-configured, login works on first browse to the UI. The platform-admin then runs one `org create` + one `project create` before the first VM.
- **No password code in mvirt-cplane.** Removes a class of CVE risk and ~4–6 weeks of work. Maintenance burden of identity primitives is on rauthy's team.
- **Operators with existing IdP infrastructure** (Zitadel/Auth0/Entra ID/Okta) swap rauthy out by changing 3–4 env vars; no data migration in mvirt itself.
- **Multi-tenancy model is identical regardless of IdP.** mvirt owns authorization; the IdP is "just" the identity supplier. Switching IdPs leaves Memberships/Roles/Projects/Orgs unchanged.
- **Modern CI/CD without shared secrets** is supported via FederatedTrust against GitHub Actions OIDC (and equivalents). No PAT distribution, no secret rotation drama.
- **Departing-employee story is clean** because human credentials (OIDC sessions) and machine credentials (ServiceAccount) are separate. Disabling the user in the IdP blocks future logins; ServiceAccounts owned by the project are unaffected.
- **The Authorizer trait is the seam** for later RBAC sophistication (OPA, custom DSLs, per-resource ACLs, ABAC). Handler code never sees the underlying policy engine.
- **Bootstrap is fully IaC-able.** `MVIRT_INITIAL_ADMIN_EMAIL` plus a `mvirt org create` + `mvirt project create` in the deployment script is the entire setup story.
- **Tier-0 break-glass**: an operator who locks themselves out (only platform-admin disabled in IdP, or IdP unavailable) can recover by setting `MVIRT_INITIAL_ADMIN_EMAIL` again *and* manually clearing the existing platform-admin row in mvirt state — last-resort, documented in the ops runbook, no special "emergency token" code path.
- **Offboarding is two-step**: disabling a user in the IdP blocks future logins, but does not invalidate already-issued mvirt session tokens. Mitigated by short session TTLs and an explicit "revoke all tokens for subject" admin action; full IdP-driven offboarding waits for SCIM.
- **Token validation is a state-machine lookup**, not a JWT verify. Cheap on the leader, requires a read on followers — acceptable for a control-plane API; the data path is unaffected.
- **Project = namespace.** Project slugs are platform-wide unique; resource names within a Project are per-Project unique. The mental model is K8s namespaces with an Org layer above. External references (audit log, dashboards, IaC) key on the Project slug alone — the durable namespace name — and remain stable across Org renames. The trade-off is a small cross-Org leak: an org-admin attempting to claim a slug already taken in an Org they cannot see gets a 409.
- **Polymorphic Membership table** carries Platform/Org/Project scopes today and is the extension point for the future `Cluster` entity. One redb table, one Authorizer code path.
- **The reverse tunnel and inter-cplane Raft channel are out of scope** for this ADR. They will use mTLS and are covered separately.
- **Storage choice (mraft) and the future `Cluster` entity** are decided in their own ADRs; this ADR is intentionally agnostic to the former and forward-compatible with the latter.
