# 0006 — Node onboarding: cluster-bound tokens, mTLS, internal CA

- **Date:** 2026-05-10
- **Status:** Accepted

## Context

ADR-0003 introduced the reverse tunnel: nodes TCP-dial the cplane on port 50056 and host gRPC services over the dialed socket. ADR-0005 introduced the Cluster as the named, explicitly-listed group of Nodes a resource can land on. Neither ADR settled how a fresh hypervisor host actually *joins* the deployment.

The pre-onboarding posture was: every running node-agent that could reach `:50056` was implicitly trusted, identified itself by a self-chosen `node_id`, and the cplane wrote a Node row on `Identify`. That was acceptable while there was a single cluster of trusted hardware behind a private network. It is not acceptable any more, for three reasons:

1. **Multi-Cluster.** With multiple Clusters per Org (ADR-0005), the cplane has to know which Cluster a connecting node belongs to. There's nothing in a TCP handshake that tells it that, and accepting `cluster_slug` from the node-agent's own `Identify` payload would let any node-agent claim membership in any Cluster.
2. **Identity at the wire layer.** Reconciliation dispatches on `node_id`. Today the cplane trusts the `node_id` the agent claims. A leaked binary on a third host could impersonate a real node by reusing its id and start receiving the real node's reconciliation traffic. Authenticating the connection — not the message — is the fix.
3. **Operator workflow.** Adding a node should be a single explicit step the operator performs ("here, this token, put it on rack-3-node-5"). Today it's an implicit "wait for the agent to phone home and hope it lands in the right place".

The constraints we want the design to honor:

- **NAT-friendly direction is preserved.** ADR-0003's invariant — the node makes the outbound TCP connection, not the cplane — must hold. Any auth scheme that needs the cplane to dial the node (e.g. server-initiated cert push) is out.
- **No secret-on-wire after bootstrap.** Long-lived bearer tokens that travel on every request are a liability: every log line, every nginx access entry, every tcpdump capture is a credential leak waiting to happen. Identity should be bound to the connection, not carried in the request.
- **No external CA dependency for the inter-component channel.** Public CAs can't issue certs for internal hostnames or custom OIDs, and we don't want our control plane gated on Let's Encrypt rate limits. Public certs stay on the operator-facing REST surface only.
- **Ops complexity proportional to the deployment size.** A homelab operator and a multi-tenant SaaS run the same code; the homelab one shouldn't have to stand up Vault to add a node.
- **Pre-launch posture stands.** Per ADR-0004, dev clusters get wiped on schema change. No backfill code for pre-onboarding nodes.

## Decision

**Bootstrap once with a cluster-bound, single-use token over plain TLS; run forever after on mTLS with certs issued by an internal CA the cplane operates.**

### Token issuance

The operator (org-admin or platform-admin in the parent Org) creates an onboarding token bound to a specific Cluster:

```
POST /v1/clusters/:cluster-slug/onboarding-tokens
body: { ttl_seconds?: u64, description?: String }
resp: { id, token, cluster_slug, expires_at, description }
                  ^---- the only time the bare token is ever revealed
```

`ttl_seconds` defaults to 3600 (1h) and is capped at 7d. `token` is 32 random bytes, base64url-encoded (43 chars). The cplane stores only `sha256(token)` — the bare token leaves the API response and is the operator's responsibility from then on.

State (new redb table `ONBOARDING_TOKENS`):

```rust
struct OnboardingToken {
    id: String,                      // tok_<short-random>, stable display id
    token_hash: [u8; 32],
    cluster_slug: String,            // immutable; binds the token to one Cluster
    description: Option<String>,     // operator-supplied, "rack-3 node 5"
    expires_at: String,
    used_at: Option<String>,         // None = redeemable, Some = consumed
    used_by_node_id: Option<String>,
    created_by_account: String,
    created_at: String,
}
```

Lifecycle:

- **Pending** → `used_at = None`, before `expires_at`. Listed in `GET /v1/clusters/:slug/onboarding-tokens` as redeemable.
- **Consumed** → `used_at = Some`, set atomically during the redeem apply (see below). Stays in the table for audit; never re-redeemable.
- **Expired** → `used_at = None` and past `expires_at`. Garbage-collected eventually; rejected at redeem time regardless.
- **Revoked** → operator-initiated `DELETE /v1/clusters/:slug/onboarding-tokens/:id`. Hard-removed; redeem attempts return 401.

The token's *cluster binding* is the central decision. A token is **not** an org-level capability that the redeemer chooses a Cluster from; the operator picks the Cluster at issuance time, encodes the operator's intent, and leaves nothing for the redeemer to decide.

### Bootstrap exchange

**Endpoint:** `POST /v1/bootstrap/onboarding` on the regular operator-facing REST API (port 8080, behind nginx with a Let's Encrypt cert). One TLS endpoint to operate, no separate listener.

**Request:**

```http
POST /v1/bootstrap/onboarding HTTP/2
Authorization: Bearer <onboarding-token>
Content-Type: application/json

{
  "csr_pem": "-----BEGIN CERTIFICATE REQUEST-----\n...",
  "host": {
    "hostname": "rack3-node5",
    "arch": "x86_64",
    "kernel_version": "6.10.0",
    "agent_version": "0.4.0"
  }
}
```

The CSR carries an **empty subject** and **no SANs** — it serves only to convey the node's freshly-generated public key. The cplane reads `csr.public_key` and ignores everything else in the CSR. `Subject`, `SubjectAltName`, requested attributes — all overridden by the issuer.

This dissolves the chicken-and-egg problem the node would otherwise have: the node has no `node_id`, no `cluster_slug`, no idea what claims to put in a CSR. It generates a keypair, wraps the public key in the standard request envelope, and lets the cplane fill in identity from the consumed token plus a freshly-minted node id.

**cplane side, atomic in one raft apply:**

1. Parse `Authorization: Bearer <token>`. Compute `sha256(token)`.
2. Look up `OnboardingToken` by hash. Reject if not found (401), already used (401), or expired (401). Don't tell the caller which.
3. Parse CSR (signature self-consistent, key algorithm in the allowed set: Ed25519 or P-256). Reject malformed CSRs (400).
4. Generate `node_id = "node_" + uuid()`.
5. Sign a leaf cert with the internal CA (see "Internal CA" below):
   - `Subject: CN=<node_id>`
   - `SubjectAltName: URI:mvirt://node/<node_id>` (canonical form — used for cert-pinning, debug)
   - Custom extensions:
     - OID `1.3.6.1.4.1.<arc>.1` → node id (UTF-8 string)
     - OID `1.3.6.1.4.1.<arc>.2` → cluster slug (UTF-8 string)
   - `notBefore = now - 5min` (clock skew), `notAfter = now + 90 days`
   - `KeyUsage: digitalSignature`, `ExtendedKeyUsage: clientAuth`
6. Write atomically:
   - `OnboardingToken.used_at = now`, `used_by_node_id = node_id`
   - new `Node` row: `{ id: node_id, cluster_slug, status: Onboarded, cert_serial, cert_expires_at, host_info, registered_at }`
   - append `node_id` to `Cluster.node_ids` for the bound `cluster_slug`
7. Audit-log the redeem.

**Response:**

```json
{
  "node_id":          "node_<uuid>",
  "cluster_slug":     "acme-west-1",
  "client_cert_pem":  "-----BEGIN CERTIFICATE-----...",
  "ca_cert_pem":      "-----BEGIN CERTIFICATE-----...",
  "tunnel_endpoint":  "tunnel.mvirt.io:50056",
  "cert_renew_after": "2026-08-07T12:00:00Z"
}
```

The node persists `client_cert_pem`, `ca_cert_pem`, and the local private key into `/var/lib/mvirt-node/`. The onboarding token is **deleted from local config** — it is consumed and useless from this point. The bootstrap exchange happens once in the lifetime of the node.

The four error paths all return without leaking: token-invalid → 401, csr-invalid → 400, cluster-deleted-since-token-issued → 410 with `{ "error": "cluster gone" }`, transient → 503. The node fails loud (no partial state on disk) and is restarted by the operator with the same token, which they discover is consumed; they issue a fresh one.

### Reverse-tunnel mTLS

Port `50056` switches from the previous "raw TCP, then HTTP/2" to "TLS, then HTTP/2 inverted". The TLS server requires client certificates.

Listener configuration:

- **Server cert:** issued by the internal CA, leaf cert with `Subject: CN=cplane`, `SAN: DNS:tunnel.mvirt.io`. Validity 90d, rotated by the cplane itself when the leader notices it's <20% of lifetime remaining. The internal CA root is what the node verifies against (the `ca_cert_pem` it received at bootstrap).
- **Client cert verification:** required. Trust anchor = same internal CA root. Reject if no client cert presented, if cert signature doesn't chain to the root, if `notBefore`/`notAfter` invalid, or if cert serial appears in the revocation set (see below).
- **Identity extraction:** after successful handshake, parse the custom OIDs out of the verified peer cert. `(node_id, cluster_slug)` becomes the connection's authenticated identity. The cplane checks: a `Node` row with `id == node_id` exists and has `cluster_slug == <peer's claim>`. Mismatch → close connection. This costs us one redb lookup per connect; connections are long-lived (single tunnel for the node's lifetime), so cost is negligible.
- **No fallback to the previous unauthenticated TCP path.** The code path is removed; a node-agent built before this ADR cannot connect.

After handshake, the existing reverse-gRPC machinery from ADR-0003 continues unchanged. `node_id` is no longer a self-claim from the gRPC `Identify` call — it's pinned to the cert and any `Identify` payload that disagrees is rejected.

`tunnel_endpoint` is returned in the bootstrap response so deployments can put the tunnel on a separate hostname / port from the REST API. Default in dev: `localhost:50056`. nginx is **not** in front of `50056` — the cplane terminates the mTLS itself, because the cert claims (`node_id`, `cluster_slug`) need to reach application code, and "nginx pass cert via header" plus trusting the header is the kind of stack we don't want to inherit. If TLS-passthrough at the L4 (nginx `stream` or HAProxy) is needed for ops reasons, that's deployment configuration; the cplane still terminates.

### Internal CA

The cplane bootstraps a self-signed root CA on first leader election if one doesn't exist:

- Algorithm: Ed25519 (smaller, faster, no key-size choices to argue about; rcgen supports it).
- Subject: `CN=mvirt internal CA, O=<deployment>, OU=mvirt-cplane`.
- Validity: 10 years. Rotation is a separate ADR if it ever becomes a real concern; for the foreseeable horizon, leaf-cert rotation is the rotation that matters.
- Storage: single row in the new `INTERNAL_CA` redb table, `{ ca_cert_pem, ca_key_pem }`.

For v1 the CA private key is stored **plain in raft state**. This is a deliberate choice with eyes open:

- A raft snapshot leak compromises the CA. The operator who has read access to raft storage already has read access to every other secret in the system (templates, scheduler hints, audit logs), so the CA isn't a uniquely sweet target.
- We don't want to gate v1 on a key-management subsystem (passphrase prompt, KMS integration, sealed secrets) that some operators will need and others will refuse. Path forward, when there's a concrete deployment that needs it, is one of:
  - operator-supplied passphrase (`mvirt-cplane init --ca-passphrase` once; then via systemd credential or env on subsequent starts) used to AES-GCM-wrap the key before persisting.
  - external KMS via a small `KeyProvider` trait (HashiCorp Vault, AWS KMS).

The forward-compat is in the data: the table is `{ ca_cert_pem, ca_key_pem }` today; tomorrow it becomes `{ ca_cert_pem, ca_key_envelope: KeyEnvelope }` where `KeyEnvelope` is an enum over plain / passphrase-wrapped / kms-managed. No protocol change.

### Cert renewal

Once the node has a working tunnel, no further onboarding tokens are needed. Renewal happens over the existing authenticated mTLS connection via a `NodeAgent.RenewCert(csr_pem) -> { client_cert_pem, ca_cert_pem }` RPC.

- Triggered by the node-agent at 80% of lifetime (~72d for a 90d cert) and retried with backoff if the connection drops mid-renew.
- The cplane re-uses the bootstrap-time validation: empty subject, public key only, signs a fresh leaf with the same custom OIDs from the *connection* identity (not from the CSR). The operator can rotate the keypair by generating a fresh one before calling `RenewCert`; the cplane doesn't care whether the public key changed.
- Old cert is **not** revoked just because a renewal happened. Both certs are valid until the old one expires. This avoids a flapping-renewal race and keeps the revocation set small (revocation is for compromise / decommission, not normal rotation).

The one nuance: if `RenewCert` fails because the node is no longer in the Node table (operator deleted it), the cplane returns `Code::Unauthenticated` and the node-agent should log loudly and exit, leaving its (now-orphan) cert files on disk for the operator to investigate. Auto-re-onboarding without a fresh token is exactly the foot-gun we're avoiding.

### Revocation

A new redb table `REVOKED_CERTS`: `{ serial, revoked_at, reason: Decommission | Compromise | Other }`. Populated by:

- `DELETE /v1/nodes/:node-id` (operator-initiated decommission) → revoke the current cert serial, also revoke the Node from every Cluster's `node_ids`.
- `POST /v1/nodes/:node-id/revoke` (operator-initiated compromise response) → revoke the cert serial, force the node into `status: Revoked` (still in Cluster `node_ids` so the operator's audit trail keeps a clear trace).

The tunnel listener loads a snapshot of the revocation set at startup and refreshes it on every raft apply that touches the table. Per-connection check is a `HashSet::contains` on the cert serial — O(1), already in memory. We do not implement standard CRL or OCSP because there is no off-host party that needs to check revocation; the cplane is both issuer and verifier.

### URL / API summary

```
# Token management (all gated by org-admin in the parent Org)
POST   /v1/clusters/:slug/onboarding-tokens
GET    /v1/clusters/:slug/onboarding-tokens
DELETE /v1/clusters/:slug/onboarding-tokens/:token-id

# The bootstrap exchange itself (token-authenticated, no Account auth)
POST   /v1/bootstrap/onboarding

# Node management
GET    /v1/nodes
GET    /v1/nodes/:id
DELETE /v1/nodes/:id                     # decommission + revoke
POST   /v1/nodes/:id/revoke              # compromise response (cert revoke, Node kept)

# Cluster ↔ Node membership (from ADR-0005, unchanged)
POST   /v1/clusters/:slug/nodes/:node-id     # move/add a node to another Cluster
DELETE /v1/clusters/:slug/nodes/:node-id
```

`POST /v1/bootstrap/onboarding` is the only route in the REST API authenticated by an onboarding token rather than by an Account session; it's namespaced under `/v1/bootstrap/` so authorization middleware can short-circuit before the Account-bearer middleware ever runs.

### Node-agent file layout

```
/var/lib/mvirt-node/
  key.pem            # node private key, 0600, never leaves the host
  cert.pem           # current client cert (PEM)
  ca.pem             # internal CA root (used to verify cplane server cert)
  state.toml         # cached node_id + cluster_slug + cert_serial for fast startup
```

`/etc/mvirt-node.toml` carries the **onboarding token only on first boot** and is rewritten by the agent (token field deleted) once the bootstrap exchange succeeds. Operator-supplied config (cplane endpoint, log level) is unaffected.

## Alternatives considered

- **Bearer token at every connect (no mTLS).** Simpler: cplane signs an opaque session token at bootstrap, node sends it on every reverse-tunnel reconnect. Rejected. The token would have to live in plaintext on disk just like a private key, but unlike a private key it travels on the wire on every reconnect, lands in nginx logs and tcpdump captures, and a replay defeats it. mTLS binds identity to the connection; no replay surface and the wire never carries the secret. Cost is the internal CA, which isn't large.

- **mTLS with cplane having a Let's Encrypt cert and the node verifying via WebPKI.** Rejected. LE can issue for the cplane's public hostname, but it can't issue the *client* certs the nodes need (LE doesn't issue clientAuth EKU certs to anonymous CSRs). We'd still need an internal CA for the node side, at which point running a single internal CA for both sides is simpler.

- **Long-lived client certs (5+ years), no auto-renewal.** Rejected. Cert rotation is the canonical mitigation for "private key got copied to a backup nobody remembers"; making rotation rare also makes it scary, and scary infra doesn't get rotated. 90 days is the sweet spot the public-cert ecosystem has converged on; we copy that.

- **Token also encodes the cluster slug as a JWT claim, no server lookup.** Considered for the elegance of stateless tokens. Rejected because we need server-side state anyway to enforce one-time-use ("has this token been redeemed?"). Once you're storing per-token state, putting the slug in the row instead of the JWT claim costs nothing and lets the operator revoke a token without rotating a JWT signing key.

- **Node-agent runs `mvirt-node bootstrap <token>` once, then a separate `mvirt-node` daemon takes over.** A clean two-binary split. Rejected for ergonomics: the bootstrap is so quick (one HTTP round trip, write three files) that splitting it out adds an entire systemd unit and a user-visible "first time setup" step for no real benefit. The daemon checks for cert files on startup and runs the bootstrap inline if they're missing.

- **Cluster auto-creates a token at create time and the operator copies *that*.** Rejected. Coupling token issuance to Cluster creation makes it harder to add a second node to an existing Cluster (the token from create time is gone, was it?), and it forces the operator into "create Cluster knowing exactly one node will join". Decoupling lets the operator issue tokens on demand.

- **Tokens are Org-scoped; redeemer picks the Cluster at bootstrap time.** Rejected. Lets a leaked token be redeemed against any Cluster in the Org, including a sensitive one. The operator's intent ("this hardware is for *this* Cluster") gets lost. Cluster-binding the token costs one more form field at issuance and pins the operator's intent into the token.

- **CSR carries the requested node id and cluster slug; cplane validates instead of choosing.** The standard PKI shape: CA validates a CSR's claims rather than overriding them. Rejected because the node has no source for those values — the bootstrap round trip would have to be split into "ask the cplane what id to use" → "submit a CSR with that id" → "cplane signs", which is two round trips for no gain. Letting the cplane be the source of identity matches our world: the node_id is meaningful only to the cplane.

- **Use the gRPC `NodeAgent.Identify` call as the bootstrap (over plain TLS), have the cplane respond with a cert, then upgrade the same connection to mTLS.** Rejected. TLS doesn't support post-handshake re-handshake-with-client-cert in any clean way (TLS 1.3 dropped renegotiation). The bootstrap-as-REST + reconnect-as-mTLS shape is two TCP connections in the lifetime of the node; the simplicity is worth it.

- **Use SPIFFE/SPIRE for workload identity.** Rejected for v1 — bringing up SPIRE servers, agents, and trust bundles adds a whole identity-mesh tier the deployment doesn't need yet. The data shape we're picking (Ed25519 client certs with custom claim OIDs) is what SPIFFE produces under the hood; if SPIFFE adoption ever becomes a goal, the migration is "swap the issuer" not "swap the auth model".

## What we explicitly defer

- **CA private key encryption-at-rest.** v1 stores it plain in raft. The data shape leaves a hole for a `KeyEnvelope` enum the day this matters. Tracked.
- **Hardware attestation (TPM, SEV-SNP, AMD/Intel platform certs).** Out of scope. The CSR `attributes` field is the standard place to carry attestation evidence; v1 has no consumer for it.
- **Per-Cluster CAs.** All Clusters share the cplane's single internal CA. If a deployment ever needs cryptographic isolation between Clusters (one Cluster's compromised CA must not let an attacker mint certs for another Cluster), that's a per-Cluster intermediate CA layered on top — additive, not breaking.
- **Standard CRL or OCSP endpoints.** The cplane is both issuer and verifier; there is no third party that consults revocation. If an external system ever needs to verify a node-issued cert, this becomes worth doing.
- **Token issuance via the UI.** v1 is REST-only. UI work lands once the cplane endpoints have settled.
- **Node-initiated decommission ("graceful leave").** Today the operator deletes a node and the cplane revokes the cert. A node that wants to leave on its own (say, hardware retirement triggered by a daemon) calls into the operator-side delete via tooling; there's no separate `NodeAgent.Decommission()` RPC. Add one if a real workflow needs it.
- **OAuth2-style token introspection.** Onboarding tokens are not access tokens; the bootstrap endpoint is the only consumer. No introspection surface.

## Consequences

- **One new long-lived secret to operate: the internal CA private key.** Acceptable and well-understood. The operator's runbook gains "if you suspect CA key compromise, here's the rotation procedure" — out of scope to write today, in scope to acknowledge.
- **The reverse-tunnel handshake gains a TLS layer.** Per-connection cost is one TLS handshake at startup; tunnels are long-lived (one per node lifetime, modulo restarts), so the steady-state overhead is zero.
- **`node_id` is now cplane-assigned, not agent-claimed.** A small but real change for any tooling that grepped logs for hand-typed IDs. Mitigation: the operator can put any human-readable label they want in the onboarding token's `description`; the audit log carries that label alongside the assigned `node_id`.
- **Operator workflow gains explicit token issuance.** Net positive — replaces an implicit "let the agent talk and trust it" flow with a clear "here's a token for this specific machine" flow. The UI work to make this nice is in the deferred list.
- **Bootstrap traffic shares the operator REST endpoint.** One TLS surface to operate, one set of LE certs, one nginx config block. The bootstrap route is publicly reachable, which means rate-limiting it is a real concern; we'll add per-IP rate limiting on the route specifically (deferred to implementation, not a separate ADR).
- **No backwards compatibility with pre-onboarding nodes.** Pre-launch state gets wiped per ADR-0004. Post-launch, this becomes a hard cutover for anyone running the system; the implementation will land paired with the schema-wipe migration.
- **Cluster `node_ids` gets touched by two paths:** explicit `POST /v1/clusters/:slug/nodes/:node-id` (move existing node) and the bootstrap apply (initial assignment). Same write target, two callers — apply handler dedupes if a redeemer somehow hits the same `node_id` twice (it can't, because `node_id` is freshly minted, but the dedupe is cheap and the invariant is worth holding).
- **Cert claims (`node_id`, `cluster_slug`) become load-bearing for authorization.** The reconciler's "which node owns this resource" decision is now backed by a TLS handshake, not a self-claim. Future authorization decisions ("can this node read template X?") get the same backing for free.
