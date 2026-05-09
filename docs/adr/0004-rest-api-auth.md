# 0004 — Authentication and authorization for the cplane REST API

- **Date:** 2026-05-09
- **Status:** Accepted

## Context

mvirt-cplane exposes a REST API on port 8080 — the only public surface for users, the web UI, the CLI/TUI, and any external automation. So far the API is unauthenticated. To ship a cluster product we need:

- **Multi-tenancy from day one.** Resources (VMs, volumes, NICs, project-private networks) belong to *projects*; users see only what they're entitled to.
- **Both homelab and enterprise** as supported operator profiles. A homelab user must not need to deploy an IdP; an enterprise user must be able to plug their existing one.
- **Air-gap-tauglich.** No mandatory dependency on a SaaS identity provider.
- **Token revocation.** Multi-tenant means kicking out a service account or compromised credential has to be O(1), not "wait for TTL".
- **A bootstrap path** that doesn't require manual DB edits and can't lock the operator out.

Three identity providers were the test bed for "is OIDC really pluggable?": Zitadel, Keycloak, Auth0, and Microsoft Entra ID (Azure AD). All speak OIDC core; their differences live almost entirely in how groups/roles are exposed (Entra's groups-overage, Auth0's audience semantics, Zitadel's namespaced role claims). Pulling authorization data from the IdP would force a per-provider claim-mapping config surface and would also create a naming clash for Entra (where "tenant" already means "customer organization"), neither of which earns its keep at v1.

## Decision

**OIDC is used for authentication only. Authorization — projects, memberships, roles — lives entirely in mvirt.** Local accounts are first-class peers of OIDC accounts, so the homelab profile works without any IdP.

### Identity model

- **Subject** is the unit of authorization — every API request resolves to exactly one Subject. Both **Users** and **ServiceAccounts** are Subjects; handlers and authz checks never branch on the kind.
- **Users** are humans. They authenticate either with a local password (+ optional TOTP) or via one of the configured OIDC providers.
- **ServiceAccounts** are machines. They live inside a project (or are cluster-scoped) and authenticate exclusively via long-lived API tokens issued by mvirt.
- **Subject identity key for OIDC users is `(issuer, sub)`**, never email. Emails change; `sub` is stable per IdP. Email is stored as a display attribute and as the join key for the invite flow.

### Provider configuration

Multiple OIDC providers can be configured in parallel on a single cluster. Per-provider config is small on purpose:

```
issuer_url
client_id
client_secret
scopes              (default: openid email profile)
admin_claim         (optional — see "Admin bootstrap")
claim_mapping       (reserved, empty in v1 — see "What we explicitly defer")
```

mvirt uses the `openidconnect` crate for discovery, JWKs, JWT verification, PKCE, and Device Code. Provider presets ("Zitadel", "Keycloak", "Auth0", "Entra ID") fill sane defaults — notably for Entra, issuer validation is `pattern + tid-check` instead of equality, because multi-tenant Entra apps see per-user issuers like `https://login.microsoftonline.com/{guid}/v2.0`.

### Tenancy

- **Project** is the tenant boundary. VMs, volumes, NICs, and project-private networks belong to exactly one project.
- **Cluster-global** entities have no project: nodes, IdP-provider configs, the project registry itself, the user/subject registry, and (for now) shared networks and templates.
- Project hierarchy is flat in v1. Adding folders/orgs later is additive; resource-to-project edges don't change.
- **Membership = `(Subject, Project, Role)`**. Standard roles `admin / editor / viewer` are built in; custom roles are deferred but the schema leaves room.
- **Cluster-admin** is a separate role independent of project membership. It implies access to all projects but is administered separately so a project-admin can't escalate to it.

### Tokens

- **All API requests carry `Authorization: Bearer <token>`.** No cookie-only paths in handlers. The UI may use a session cookie as transport, but the handler sees a bearer token.
- **Tokens are opaque random strings, looked up in cplane state, stored hashed.** Revocation is a single state-machine command; no JWT denylist, no short-TTL/refresh dance. Validation latency is acceptable because cplane is the issuer — no edge-validation use case justifies JWT.
- **OIDC ID-tokens never flow through mvirt's API.** The OIDC login flow exchanges a verified ID-token for an mvirt-issued opaque session token. Decouples API surface from IdP, makes revocation uniform, makes local and OIDC sessions indistinguishable downstream of login.
- **CLI / machine flows:** users authenticate via Device Code (RFC 8628) against any configured OIDC provider, or with a local password, then receive an mvirt token. Service accounts use pre-provisioned long-lived API tokens — never OIDC.

### Login flows

```
  Browser (UI)            mvirt-cplane             OIDC IdP
       │                       │                       │
       │  GET /login           │                       │
       │──────────────────────►│                       │
       │  302 to IdP authz     │                       │
       │◄──────────────────────│                       │
       │  authz + PKCE         │                       │
       │──────────────────────────────────────────────►│
       │  302 to /callback?code                        │
       │◄──────────────────────────────────────────────│
       │  GET /callback?code   │                       │
       │──────────────────────►│                       │
       │                       │  exchange code        │
       │                       │──────────────────────►│
       │                       │  id_token (verified)  │
       │                       │◄──────────────────────│
       │                       │  upsert Subject       │
       │                       │  by (issuer, sub)     │
       │                       │  issue mvirt token    │
       │  Set-Cookie + redir   │                       │
       │◄──────────────────────│                       │
       │  GET /v1/...          │                       │
       │  Bearer <mvirt token> │                       │
       │──────────────────────►│                       │

  CLI: identical, but Device Code instead of redirect. Local accounts skip
  the IdP column entirely; everything else is identical.
```

### Admin bootstrap

Two mechanisms, both supported, neither required to be the *only* way in:

1. **One-time bootstrap token printed to the cplane log on first start** (Gitea/Vaultwarden pattern). The first browser to present the token at `/setup` becomes cluster-admin. After consumption it's discarded. No env-var redeploy, no DB edit.
2. **Optional `admin_claim` per OIDC provider.** Carve-out from "no IdP-driven authz": *one* claim, *one* boolean outcome — anyone whose ID-token contains `claim_path == value` is treated as cluster-admin. This is the durable recovery backdoor: even if the operator deletes their only mvirt admin, granting the corresponding role/group in the IdP restores access. Same pattern as Grafana's `role_attribute_path`. Three config fields, ~50 LOC in the login handler, no implication of further claim mapping.

For the inaugural Zitadel setup specifically: define an `mvirt-admin` project role in Zitadel, enable "Project Roles + User Roles inside Token" on the OIDC client, and point `admin_claim` at `urn:zitadel:iam:org:project:roles → mvirt-admin`.

### Onboarding

OIDC users are JIT-created on first successful login (`(issuer, sub)` not seen before → new Subject row, no project memberships). Admins promote them into projects.

Pre-provisioning is supported via an **invite flow**: an admin enters an email; a pending Subject is created and bound on first OIDC login that yields a matching email. This avoids the chicken-and-egg of "I want to add Alice to project X, but Alice has never logged in".

## What we explicitly defer

- **Group/role sync from the IdP.** Default is mvirt-managed authz. The provider-config schema reserves a `claim_mapping` block so it can be added later as opt-in without an API break. Same pattern as Grafana, Argo, Vault.
- **SCIM** for lifecycle sync (provisioning/deprovisioning). Heavy; appropriate as an enterprise upsell on a later track.
- **Custom roles beyond `admin/editor/viewer`.** Schema leaves room; UI/API surface deferred.
- **Project hierarchy** (folders, orgs, nesting). Flat in v1.
- **Back-channel logout / RP-initiated logout.** Patchy provider support; short mvirt session TTL is the mitigation.
- **Token-introspection-based edge validation.** Not needed while cplane is the issuer.

## Alternatives considered

- **JWT as the mvirt API token.** Rejected: revocation in a multi-tenant system has to be cheap and immediate. The denylist or short-TTL-refresh choreography you'd need is more code than the opaque-token-lookup it replaces, and we have no edge-validation use case. The OIDC ID-token *is* a JWT — we just don't propagate it past the login boundary.
- **Pull groups/roles from the IdP via claim mapping (v1 default).** Rejected for v1: per-provider claim paths (`groups` / `realm_access.roles` / `urn:zitadel:...` / Entra App Roles), Entra's groups-overage requiring Microsoft Graph fallback, and the IdP-tenant-vs-mvirt-tenant naming clash all bring config surface that doesn't earn its keep when authz can live coherently inside mvirt. Reachable later as opt-in.
- **mTLS for REST API users.** Rejected for the user-facing API: certificate provisioning per browser/CLI user is a UX cliff. mTLS belongs on the internal mesh (reverse tunnel, inter-cplane Raft) and gets its own ADR.
- **Cookie-only sessions in handlers.** Rejected: forces UI and CLI down divergent code paths and complicates service-account use. Bearer-everywhere is uniform; the UI uses a cookie *as transport* but handlers don't know the difference.
- **OIDC ID-token passed straight through to the mvirt API.** Rejected: couples our API surface to whichever IdP issued the token, makes revocation IdP-dependent, and forces clients to refresh against the IdP. Exchange-on-login keeps mvirt the sole authority for what its tokens mean.

## Consequences

- Homelab operators can skip OIDC entirely — local accounts + TOTP + API tokens is a complete story.
- Multi-IdP per cluster is trivial: Subjects key on `(issuer, sub)`, no name collisions across providers.
- Provider-config surface is the same four/five fields for every IdP. Per-provider quirks (Entra issuer pattern, App Roles vs groups) live in presets, not in handler code.
- New users land with no project memberships — admins must onboard them. With group-sync this would be automatic; the trade is a smaller surface and a coherent authz model. The invite flow softens the manual step.
- Offboarding is a two-step concern: disabling a user in the IdP blocks future logins, but does not invalidate already-issued mvirt tokens. Mitigated by short session TTLs and an explicit "revoke all tokens for subject" admin action; full IdP-driven offboarding waits for SCIM.
- Token validation is a state-machine lookup, not a JWT verify. Cheap on the leader, requires a read on followers — acceptable for a control-plane API; the data path is unaffected.
- The reverse tunnel and Raft inter-cplane channel are out of scope for this ADR. They will use mTLS and are covered separately.
