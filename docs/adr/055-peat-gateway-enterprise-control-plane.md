# ADR-055: Peat Gateway — Enterprise Control Plane

**Status**: Proposed
**Date**: 2026-03-10
**Authors**: Kit Plummer
**Organization**: (r)evolve - Revolve Team LLC (https://revolveteam.com)
**Relates To**: ADR-043 (Consumer Interface Adapters), ADR-045 (Zarf/UDS Integration), ADR-048 (Membership Certificates), ADR-049 (Peat-Mesh Extraction), ADR-050 (SDK Integration), ADR-054 (UDS Registry Replication)

**Amendments**:
- **2026-05-08 — Amendment A: Full-duplex NATS flow.** Adds control-plane ingress (subscriber path) alongside the original CDC egress (publisher) treatment. NATS becomes a bidirectional channel; Kafka and Redis Streams remain egress-only. See "2a. Control-Plane Ingress (Amendment A — NATS)" below. Tracking: peat#839, peat-gateway#91, peat-gateway#94.

---

## Executive Summary

`peat-mesh-node` is a single-formation, single-authority mesh node for tactical edge deployment. Enterprise and cloud environments need a dedicated, horizontally scalable control plane — `peat-gateway` — providing multi-org tenancy, CDC to external event streams, IDAM/ICAM-federated enrollment, an admin UI, and first-class Zarf/UDS packaging for air-gapped deployment.

## Context

`peat-mesh-node` participates in one mesh, manages one certificate store, and exposes a broker API for local consumers. This is the right design for a tactical edge node, but enterprise deployments need:

- **Multi-tenancy**: Multiple organizations sharing a gateway instance, each with independent formations (app IDs), isolated key material, and separate data paths.
- **Change Data Capture (CDC)**: CRDT document mutations must flow to external event infrastructure — Kafka, NATS, Redis Streams — for downstream analytics, audit, and integration pipelines.
- **Identity federation**: Enrollment and access control must delegate to enterprise IDAM/ICAM (Keycloak, Okta, Azure AD) rather than static bootstrap tokens.
- **Operational visibility**: Administrators need a UI to inspect mesh topology, peer health, document state, enrollment status, and certificate lifecycle across all managed formations.
- **Packaging and delivery**: The gateway must be deployable as a Zarf package into UDS clusters, including air-gapped environments, with SSO, monitoring, and policy enforcement out of the box.

**Amended 2026-05-08 (Amendment A)**: The gateway is also a *consumer* of control-plane events on NATS — formation lifecycle, peer enrollment requests, certificate revocations, and IdP claim refreshes can originate from external orchestration systems and arrive over the same broker the gateway publishes CDC events to. NATS therefore needs to be designed as a bidirectional integration, not a one-way sink.

None of these belong in `peat-mesh-node` — they require a dedicated service.

## Decision

### Introduce `peat-gateway`

A new standalone repository (`defenseunicorns/peat-gateway`) that depends on `peat-mesh` as a library and provides the enterprise control plane layer. Same pattern as `peat-registry`.

### Tenancy Model

```
Gateway Instance
  │
  ├── Org: "acme-corp"
  │     ├── Formation: app_id="logistics-mesh"   mesh_id=a1b2c3d4
  │     │     ├── Genesis keypair + cert chain
  │     │     ├── Certificate store
  │     │     ├── Enrollment service → Keycloak realm "acme"
  │     │     └── CDC sinks: [kafka:acme-logistics, webhook:acme-siem]
  │     │
  │     └── Formation: app_id="sensor-grid"      mesh_id=e5f6a7b8
  │           ├── Genesis keypair + cert chain
  │           ├── Certificate store
  │           ├── Enrollment service → Keycloak realm "acme"
  │           └── CDC sinks: [nats:acme.sensors.>]
  │
  └── Org: "taskforce-north"
        └── Formation: app_id="c2-mesh"          mesh_id=c9d0e1f2
              ├── Genesis keypair + cert chain
              ├── Certificate store
              ├── Enrollment service → CAC/mTLS (SAML)
              └── CDC sinks: [nats:tf-north.c2.>]
```

**Isolation guarantees**:
- Each org has independent key material — no cross-org certificate trust
- CDC events are routed only to the org's configured sinks
- IDAM provider configuration is per-org (different Keycloak realms, different IdPs)
- API access is scoped by org membership (RBAC)
- Data storage is logically partitioned; optionally physically partitioned (separate PVCs)

### Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                         peat-gateway                              │
│                                                                  │
│  ┌───────────────┐  ┌──────────────┐  ┌────────────────────────┐│
│  │  Tenant       │  │  CDC Engine  │  │  AuthZ Proxy           ││
│  │  Manager      │  │              │  │                        ││
│  │               │  │  • Watch doc │  │  • OIDC/SAML           ││
│  │  • Org CRUD   │  │    changes   │  │  • Token exchange      ││
│  │  • Multi-app  │  │  • Per-org   │  │  • Per-org IdP config  ││
│  │    genesis    │  │    fan-out   │  │  • Policy engine       ││
│  │  • Cert       │  │  • At-least- │  │  • Role → MeshTier    ││
│  │    authority  │  │    once      │  │    mapping             ││
│  │  • Enrollment │  │    delivery  │  │  • Enrollment          ││
│  │    delegation │  │              │  │    delegation          ││
│  └──────┬────────┘  └──────┬───────┘  └─────────┬──────────────┘│
│         │                  │                     │               │
│  ┌──────┴──────────────────┴─────────────────────┴─────────────┐│
│  │                  peat-mesh (library)                          ││
│  │  MeshGenesis · CertificateStore · SyncProtocol · Iroh        ││
│  └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │                     Admin API / UI                            ││
│  │  • Org management            • Peer health dashboard         ││
│  │  • Formation CRUD            • Enrollment management         ││
│  │  • Document browser          • Stream sink config            ││
│  │  • Certificate lifecycle     • IDAM provider config          ││
│  │  • Audit log viewer          • Cross-org analytics           ││
│  └──────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────┘
          │             ▼ ▲                 │
          ▼             │ │                 ▼
   ┌────────────┐  ┌────────────┐   ┌──────────────┐
   │ Mesh Nodes │  │ Kafka/NATS │   │ Keycloak/    │
   │ (tactical) │  │ Redis Strm │   │ Okta/AzureAD │
   └────────────┘  └────────────┘   └──────────────┘
```

> **Amendment A (2026-05-08)**: The dual `▼ ▲` arrow on the middle column reflects NATS as bidirectional — CDC events flow *out* (publisher) and control-plane events flow *in* (subscriber). Kafka and Redis Streams remain egress-only. Subscriber connection is gateway-initiated, so the existing outbound NATS path carries traffic in both directions. See "2a. Control-Plane Ingress" below.

### Component Details

#### 1. Tenant Manager

Top-level orchestrator managing organizations and their formations.

**Org model**:
```rust
struct Organization {
    org_id: String,                     // unique slug, e.g. "acme-corp"
    display_name: String,
    idp_config: IdpConfig,              // per-org IDAM provider
    formations: Vec<FormationConfig>,
    quotas: OrgQuotas,                  // max formations, peers, documents
    created_at: u64,
}

struct FormationConfig {
    app_id: String,                     // user-facing, unique within org
    mesh_id: String,                    // derived from genesis
    genesis: MeshGenesis,
    enrollment_policy: EnrollmentPolicy,
    cdc_sinks: Vec<SinkConfig>,
}

struct OrgQuotas {
    max_formations: u32,
    max_peers_per_formation: u32,
    max_documents_per_formation: u32,
    max_cdc_sinks: u32,
}
```

Each formation runs its own `MeshGenesis`, `CertificateStore`, and enrollment service. The tenant manager supervises lifecycle (create, suspend, destroy) and enforces quotas.

Configuration is stored in Postgres (multi-tenant production) or SQLite (single-tenant / dev). Key material is stored in Postgres with envelope encryption, or delegated to a KMS (AWS KMS, HashiCorp Vault) in hardened deployments.

#### 2. CDC Engine

Watches Automerge document changes across all formations and produces events to org-scoped sinks.

**Event model**:
```rust
struct CdcEvent {
    org_id: String,
    app_id: String,
    document_id: String,
    change_hash: Vec<u8>,
    actor_id: String,       // peer that made the change
    timestamp_ms: u64,
    patches: Vec<Patch>,    // Automerge patches (JSON-compatible)
    metadata: HashMap<String, String>,
}
```

**Sinks** (pluggable via trait):
- **Kafka** — topic-per-app or topic-per-document, configurable partitioning
- **NATS** — JetStream subjects with org/app hierarchy (e.g., `acme.logistics.docs.>`)
- **Redis Streams** — XADD with consumer groups
- **Webhook** — HTTP POST with retry and dead-letter queue
- **stdout/file** — for debugging and log aggregation

**Delivery guarantees**: At-least-once. Each sink tracks its cursor (last emitted change hash per document). On restart, replays from cursor. Cursors are stored alongside formation state in the persistent backend.

#### 2a. Control-Plane Ingress (Amendment A — NATS)

*Added 2026-05-08. Tracking: peat#839, peat-gateway#91, peat-gateway#94.*

NATS is bidirectional. The CDC Engine above publishes change events outbound; the gateway also **subscribes** to a parallel set of NATS subjects to receive control-plane events from external orchestration systems — formation lifecycle requests, peer enrollment intents, certificate revocations, IdP claim refreshes.

**Subject schema** (separate namespace from CDC egress):

| Direction | Pattern | Example |
|---|---|---|
| Egress (existing — CDC) | `{org}.{app}.docs.>` | `acme.logistics.docs.changes` |
| Ingress — org-level lifecycle | `{org}.ctl.>` | `acme.ctl.formations.create` |
| Ingress — per-formation control | `{org}.{app}.ctl.>` | `acme.logistics.ctl.peers.enroll` |

The leaf discriminator (`docs` vs `ctl`) is what separates egress from ingress within a single org's namespace. This shape preserves the per-org NATS account model: an org's account can be granted publish rights on its own `{org}.*.ctl.>` and *only* its own — there is no cross-org subject path.

**Initial event classes** (non-exhaustive — handlers register subjects at runtime; payload schemas are deferred to peat-gateway#91):

| Subject (template) | Purpose |
|---|---|
| `{org}.ctl.formations.create` | Provision a new formation under an existing org |
| `{org}.ctl.formations.suspend` | Suspend an active formation |
| `{org}.ctl.formations.destroy` | Tear down a formation and its key material |
| `{org}.{app}.ctl.peers.enroll.request` | External system asks the gateway to issue a mesh certificate for a peer |
| `{org}.{app}.ctl.peers.revoke.request` | Revoke a peer's membership |
| `{org}.{app}.ctl.certificates.revoke.request` | Revoke a specific certificate by serial / fingerprint |
| `{org}.ctl.idp.claims.refresh` | Force a re-introspection / claim refresh against the org's IdP |

**Tenant isolation guarantees** (extending the org isolation model from the Tenancy section):

- Subscriptions are *strictly* scoped to subjects under `{org}.>` for orgs the gateway instance manages — no wildcard `>` and no cross-org subscriptions
- Subscription lifecycle is bound to org/formation lifecycle: subscribe on create, unsubscribe on suspend/destroy. Orphan subscriptions are an isolation bug
- Per-org NATS account / permission rules MUST grant publish on `{org}.*.ctl.>` only — gateway integration tests assert that a publish to another org's subject is rejected at the broker
- Inbound payloads are tagged with `org_id` extracted from the subject and re-validated against tenant-manager state before any handler runs (defence in depth — broker ACL is primary, in-process check catches misconfiguration)
- Ingress events do NOT bypass the AuthZ Proxy: any event that mutates state runs through the same policy engine as the equivalent REST call

**Delivery & connection model**:

- Subscriber connection is initiated from the gateway → broker (same TCP direction as the publisher), so no new NetworkPolicy ingress rule is required (see UDS Package CR below)
- JetStream durable consumers per `(gateway-instance, org)` for replay and at-least-once delivery — ephemeral subscriptions are not used for state-changing events
- Reconnect on broker bounce; replay from the durable consumer cursor

**Out of scope for this amendment** (tracked separately):
- Migration from current core-NATS publish to JetStream — peat-gateway#92
- NATS as an IDAM federation provider — peat-gateway#93

#### 3. AuthZ Proxy

Bridges enterprise identity into mesh enrollment and access control, configured per-org.

**Identity federation**:
- OIDC discovery + token introspection (Keycloak, Okta, Azure AD)
- SAML assertion consumption (DoD/gov CAC environments)
- mTLS client certificate mapping (zero-trust architectures)

**Token-to-certificate flow**:
```
Client → Gateway:  Bearer <OIDC token> + enrollment request + org_id + app_id
Gateway → IdP:     Token introspection (org's configured provider)
Gateway:           Validate org membership, map claims → MeshTier + permissions
Gateway → Client:  MeshCertificate (signed by formation authority)
```

**Policy engine**:
- Per-org IdP configuration (different Keycloak realms, different providers entirely)
- Claim-to-tier mapping rules (e.g., `role:admin` → `MeshTier::Authority`)
- Per-formation enrollment policies (open, controlled, strict)
- Permission bit assignment from IDAM roles/groups
- Rate limiting and enrollment quotas per org

#### 4. Admin API / UI

**REST API**:
```
# Org management
POST   /orgs                                    # create org
GET    /orgs                                    # list orgs
GET    /orgs/{org_id}                           # org details + quotas
PATCH  /orgs/{org_id}                           # update org config
DELETE /orgs/{org_id}                           # destroy org + all formations

# Formation management (scoped by org)
POST   /orgs/{org_id}/formations                # create formation (genesis)
GET    /orgs/{org_id}/formations                # list formations
GET    /orgs/{org_id}/formations/{app_id}       # formation details
DELETE /orgs/{org_id}/formations/{app_id}       # destroy formation

# Mesh state (scoped by org + formation)
GET    /orgs/{org_id}/formations/{app_id}/peers         # peer list, health
GET    /orgs/{org_id}/formations/{app_id}/documents     # document inventory
GET    /orgs/{org_id}/formations/{app_id}/certificates  # cert inventory
POST   /orgs/{org_id}/formations/{app_id}/certificates/{id}/revoke

# CDC
GET    /orgs/{org_id}/formations/{app_id}/cdc/sinks     # configured sinks
POST   /orgs/{org_id}/formations/{app_id}/cdc/sinks     # add sink
DELETE /orgs/{org_id}/formations/{app_id}/cdc/sinks/{id}

# Enrollment
POST   /orgs/{org_id}/formations/{app_id}/enrollment/tokens  # issue tokens

# System
GET    /health                                  # aggregated health
GET    /metrics                                 # Prometheus metrics
```

**Web UI** (SvelteKit — consistent with UDS ecosystem):
- Multi-org dashboard: org list, formation counts, aggregate peer/document stats
- Per-org view: formations, IdP status, quota usage
- Per-formation drill-down: topology graph, peer table, document browser
- Certificate management: issue, inspect, revoke, expiry timeline
- CDC monitoring: sink status, event throughput, cursor lag
- IDAM configuration: provider setup, claim mapping rule editor

### Zarf / UDS Deployment

The gateway is packaged for the Defense Unicorns ecosystem as a first-class UDS capability, following the same patterns established in ADR-045 and ADR-054.

#### Zarf Package Structure

```
zarf-peat-gateway/
├── zarf.yaml
├── chart/
│   └── peat-gateway/            # Helm chart
│       ├── Chart.yaml
│       ├── values.yaml
│       ├── templates/
│       │   ├── deployment.yaml        # or StatefulSet if using local PVC
│       │   ├── service.yaml
│       │   ├── service-monitor.yaml   # Prometheus scrape
│       │   ├── network-policy.yaml    # Istio-aware
│       │   ├── uds-package.yaml       # UDS Package CR
│       │   ├── uds-exemptions.yaml    # Pepr exemptions if needed
│       │   ├── configmap.yaml         # formation config, sink config
│       │   ├── secret.yaml            # IdP client secrets, KMS keys
│       │   ├── pvc.yaml               # formation state storage
│       │   └── hpa.yaml               # horizontal pod autoscaler
│       └── values/
│           ├── unicorn.yaml           # UDS Core defaults
│           └── airgap.yaml            # air-gapped overrides
└── images/
    └── peat-gateway:0.1.0            # multi-arch (amd64/arm64)
```

#### UDS Package CR

```yaml
apiVersion: uds.dev/v1alpha1
kind: Package
metadata:
  name: peat-gateway
  namespace: peat-system
spec:
  network:
    expose:
      # Admin UI + API via Istio VirtualService
      - service: peat-gateway
        gateway: tenant
        host: peat
        port: 8080
    allow:
      # Outbound to NATS (in-cluster) — bidirectional via subscription (Amendment A)
      - direction: Egress
        remoteNamespace: nats
        port: 4222
        description: "CDC egress + control-plane ingress (subscription is gateway-initiated, no separate Ingress rule needed)"
      # Outbound to Kafka (in-cluster or external)
      - direction: Egress
        remoteNamespace: kafka
        port: 9092
        description: "CDC events to Kafka"
      # Outbound to Keycloak (SSO)
      - direction: Egress
        remoteNamespace: keycloak
        port: 8080
        description: "OIDC token introspection"
      # Inbound from mesh nodes (Iroh QUIC)
      - direction: Ingress
        port: 11204
        description: "Iroh mesh sync"
      # Inbound from mesh nodes (enrollment ALPN)
      - direction: Ingress
        port: 11205
        description: "Enrollment protocol"
  sso:
    - name: peat-gateway
      clientId: uds-peat-gateway
      redirectUris:
        - "https://peat.{{ .Values.domain }}/auth/callback"
      groups:
        peat-admin:
          description: "Full gateway admin access"
        peat-org-admin:
          description: "Org-scoped admin access"
        peat-viewer:
          description: "Read-only access"
```

#### UDS Bundle

For air-gapped deployment, a UDS bundle wraps the gateway with its dependencies:

```yaml
# uds-bundle.yaml
kind: UDSBundle
metadata:
  name: peat-gateway-bundle
  version: 0.1.0

packages:
  # NATS (CDC sink)
  - name: nats
    repository: ghcr.io/defenseunicorns/packages/nats
    ref: 2.10.0

  # Peat Gateway
  - name: peat-gateway
    path: ./zarf-peat-gateway
    ref: 0.1.0
    optionalComponents:
      - kafka-sink      # include Kafka sink support
      - admin-ui        # include web admin UI

  # Optional: PostgreSQL for multi-tenant state
  - name: postgres
    repository: ghcr.io/defenseunicorns/packages/postgres
    ref: 16.0.0
```

#### Helm Values (Production Defaults)

```yaml
# values/unicorn.yaml — UDS Core integration
replicaCount: 2

image:
  repository: ghcr.io/defenseunicorns/peat-gateway
  tag: "0.1.0"

persistence:
  enabled: true
  storageClass: "local-path"      # overridden per cluster
  size: 10Gi

database:
  type: postgres                   # or sqlite for dev/edge
  host: postgres-rw.postgres.svc
  name: peat_gateway
  existingSecret: peat-gateway-db

sso:
  enabled: true
  provider: keycloak
  issuerUri: "https://sso.{{ .Values.domain }}/realms/uds"

cdc:
  defaultSink: nats
  nats:
    url: "nats://nats.nats.svc:4222"

# Amendment A (2026-05-08): NATS as control-plane ingress.
# Reuses the broker URL from `cdc.nats.url` above. The implementation PR
# (peat-gateway#91) may unify these into a single top-level `nats:` block
# with `egress`/`ingress` subkeys; the surface is intentionally minimal here.
nats:
  ingress:
    enabled: true
    # Subjects are auto-derived from registered orgs/formations:
    #   {org}.ctl.>           — org-scoped lifecycle events
    #   {org}.{app}.ctl.>     — per-formation control events
    # Per-org overrides may be supplied if non-default scoping is needed:
    overrides: {}
    # Durable JetStream consumer name template; one per (instance, org):
    consumerNameTemplate: "peat-gateway-{instance}-{org}-ctl"

monitoring:
  serviceMonitor:
    enabled: true
  dashboards:
    enabled: true                  # Grafana dashboards for gateway metrics

networkPolicies:
  enabled: true                    # Istio-aware network policies

resources:
  requests:
    cpu: 250m
    memory: 512Mi
  limits:
    cpu: "2"
    memory: 2Gi
```

#### Container Build

```dockerfile
# Multi-stage, multi-arch
FROM rust:1.94 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin peat-gateway

FROM cgr.dev/chainguard/glibc-dynamic:latest
COPY --from=builder /build/target/release/peat-gateway /usr/local/bin/
EXPOSE 8080 11204 11205
ENTRYPOINT ["peat-gateway"]
```

Base image: Chainguard `glibc-dynamic` for minimal CVE surface (consistent with DU container practices). Multi-arch build via `docker buildx` for amd64 + arm64.

#### CI/CD Pipeline

```yaml
# .github/workflows/release.yml
# On tag push (v*):
# 1. cargo test + clippy
# 2. Build multi-arch container (buildx)
# 3. cosign sign container image
# 4. Generate SBOM (syft)
# 5. Push to GHCR
# 6. Build Zarf package (zarf package create)
# 7. Publish Zarf package to GHCR OCI
# 8. Optionally: publish to crates.io
```

### Feature Flags (Cargo)

| Feature | What it enables |
|---------|----------------|
| `gateway` | Tenant manager, formation lifecycle, admin API |
| `kafka` | Kafka CDC sink (rdkafka) |
| `nats` | NATS JetStream CDC sink (egress) + control-plane ingress subscriber (Amendment A) |
| `redis-streams` | Redis Streams CDC sink |
| `webhook` | HTTP webhook CDC sink |
| `oidc` | OIDC token introspection |
| `saml` | SAML assertion consumer |
| `admin-ui` | Embedded SvelteKit admin UI (static assets) |
| `postgres` | Postgres backend for multi-tenant state |
| `full` | All features |

### Crate / Repository

**Separate repo**: `defenseunicorns/peat-gateway`. The gateway has a fundamentally different dependency tree (rdkafka, OIDC client, SAML parser, Postgres driver, admin UI assets), release cycle, and deployment model from the mesh library. Same pattern as `peat-registry`.

```toml
[dependencies]
peat-mesh = { version = "0.5", features = ["automerge-backend", "broker"] }
```

## Consequences

### Positive

- Enterprise deployments get a production-ready control plane without forking `peat-mesh-node`
- Org-level multi-tenancy enables SaaS and shared-infrastructure deployments
- CDC enables integration with existing enterprise data pipelines and SIEM/audit systems
- IDAM integration removes the need for static enrollment tokens in production
- Full Zarf/UDS packaging makes the gateway deployable in air-gapped DoD environments
- Admin UI reduces operational burden and enables non-CLI users
- Consistent with DU ecosystem (Chainguard images, Pepr policies, Keycloak SSO, Grafana monitoring)
- *(Amendment A)* Gateway is reachable from external orchestration systems via the existing NATS broker — no additional inbound port, no new NetworkPolicy ingress rule

### Negative

- New repo and codebase to build and maintain
- Heavy dependency footprint (Kafka C library, OIDC, SAML, Postgres)
- Leader election / formation sharding adds operational complexity
- Admin UI is a separate frontend stack (SvelteKit) to maintain
- Zarf packaging and UDS integration adds CI/CD complexity
- *(Amendment A)* NATS surface is now bidirectional — per-org broker ACL discipline becomes a tenant isolation boundary, increasing operational and review burden

### Risks

- CDC ordering guarantees across concurrent CRDT changes need careful design
- IDAM integration latency could slow enrollment in high-churn scenarios
- Multi-org key management (many root keypairs) increases blast radius of gateway compromise — mitigate with KMS delegation
- Org isolation bugs could leak data across tenants — requires thorough integration testing
- *(Amendment A)* Misconfigured per-org NATS ACLs could enable cross-tenant control-plane writes — mitigated by defence in depth (broker ACL + in-process `org_id` revalidation) and asserted in functional tests

## Implementation Phases

### Phase 1: Foundation
- [ ] Create `peat-gateway` repo with `peat-mesh` dependency
- [ ] Tenant manager: org CRUD, formation lifecycle, persistent config (SQLite first, Postgres later)
- [ ] Multi-genesis: concurrent MeshGenesis instances with per-formation cert stores
- [ ] Admin REST API: org/formation CRUD, peer listing, certificate management
- [ ] Health and Prometheus metrics endpoints
- [ ] Dockerfile (multi-arch, Chainguard base)

### Phase 2: CDC + NATS Full-Duplex (Amendment A)
- [ ] CDC event model and watcher (Automerge document change subscription)
- [ ] Sink trait with cursor tracking and at-least-once delivery
- [ ] NATS JetStream sink (egress)
- [ ] Kafka sink
- [ ] Webhook sink
- [ ] **NATS control-plane ingress subscriber** (Amendment A — peat-gateway#91)
  - [ ] Tenant-scoped subject schema: `{org}.ctl.>`, `{org}.{app}.ctl.>`
  - [ ] Subscription lifecycle bound to org/formation lifecycle (subscribe on create, unsubscribe on suspend/destroy)
  - [ ] Per-tenant ACL enforcement at the broker + in-process `org_id` revalidation (defence in depth)
  - [ ] Reconnect / replay against JetStream durable consumer cursor
  - [ ] Functional tests: happy path, tenant isolation, reconnect (uses harness from peat-gateway#90)

### Phase 3: Identity Federation
- [ ] OIDC token introspection and claim extraction
- [ ] Per-org IdP configuration
- [ ] Claim-to-tier policy engine
- [ ] Enrollment delegation (OIDC token → mesh certificate)
- [ ] SAML assertion consumer (gov/DoD environments)

### Phase 4: Admin UI
- [ ] SvelteKit project scaffold
- [ ] Multi-org dashboard and formation overview
- [ ] Per-formation drill-down: topology graph, peer table, document browser
- [ ] Certificate lifecycle management UI
- [ ] CDC sink configuration and monitoring

### Phase 5: Zarf / UDS Packaging
- [ ] Helm chart with UDS Package CR, network policies, SSO config
- [ ] Zarf package definition (zarf.yaml)
- [ ] UDS bundle with NATS and optional Postgres
- [ ] Grafana dashboards for gateway metrics
- [ ] CI pipeline: test → build → sign → SBOM → Zarf package → publish

### Phase 6: Production Hardening
- [ ] Postgres backend with envelope encryption for key material
- [ ] KMS integration (AWS KMS, HashiCorp Vault) for root key protection
- [ ] Horizontal scaling with leader election per formation
- [ ] Integration test suite: multi-org isolation, CDC end-to-end, OIDC flow, Zarf deploy
- [ ] Load testing: concurrent formations, CDC throughput, enrollment rate

## References

- [peat-mesh](https://github.com/defenseunicorns/peat-mesh) — mesh networking library
- [peat-registry](https://github.com/defenseunicorns/peat-registry) — OCI registry sync (same crate pattern)
- [Zarf](https://docs.zarf.dev/) — air-gap package manager
- [UDS Core](https://github.com/defenseunicorns/uds-core) — secure runtime platform
- [Pepr](https://github.com/defenseunicorns/pepr) — K8s policy middleware
