# ADR-059: Data as a Capability

**Status**: Proposed
**Date**: 2026-05-11
**Authors**: Austin Ruth
**Related ADRs**:
- [ADR-018](018-ai-model-capability-advertisement.md) (AI Model Capability Advertisement)
- [ADR-025](025-blob-transfer-protocol.md) (Blob Transfer Protocol)
- [ADR-054](054-UDS-registry-replication-ddil.md) (UDS Registry Replication-to-Sync for DDIL)
- [ADR-055](055-peat-gateway-enterprise-control-plane.md) (peat-gateway Enterprise Control Plane)
- [ADR-056](056-app-id-scoped-relay-hop-mode.md) (App-ID-Scoped Relay Hop Mode)

---

## Context

### Problem Statement

Data sources — SQL databases, object stores, streaming feeds, vector indexes — exist in isolation across UDS deployments. Applications that need that data have no standard way to discover it, request access to it, or consume it, especially across organizational or network boundaries.

The gap is not just transport. It is the lack of a common vocabulary for:

1. **Discovery** — what data exists, where, and in what shape
2. **Access control** — who is allowed to consume it
3. **Consumption** — how a consumer actually receives the data
4. **Observability** — who accessed what and when

Peat already solves analogous problems for AI model distribution (ADR-018) and software package delivery (ADR-045). This ADR extends the same capability advertisement pattern to arbitrary data sources.

### Scope

This ADR covers:
- The capability descriptor schema for data sources
- How descriptors are kept live as sources change
- Access control via UDS RBAC
- Two consumption modes (cloud/on-prem ISV and tactical edge)
- Audit trail via peat-gateway CDC

This ADR does **not** cover:
- The internal data plane (lakehouse, vector store, streaming infrastructure)
- ETL pipeline orchestration
- Query language or federation semantics

---

## Decision

### 1. Data Capability Descriptor

A data source is advertised as a **capability descriptor** — a live CRDT document in the `data_capabilities` collection. The descriptor has a common envelope and a source-type-specific schema block.

#### Common Envelope

```rust
pub struct DataCapabilityDescriptor {
    /// Stable identifier for this capability, unique within the formation
    pub capability_id: Uuid,

    /// Human-readable name
    pub name: String,

    /// Free-text description
    pub description: Option<String>,

    /// Source type — determines the shape of `schema`
    pub source_type: DataSourceType,

    /// Source-type-specific schema skeleton (see below)
    pub schema: DataSourceSchema,

    /// Node ID of the advertising node
    pub owner_node_id: String,

    /// Formation app_id this capability belongs to
    pub app_id: String,

    /// Data classification / sensitivity label
    pub classification: Option<String>,

    /// When this descriptor was last updated
    pub updated_at: u64,

    /// Whether this capability is currently available
    pub available: bool,
}

pub enum DataSourceType {
    PostgreSQL,
    S3,
    Kafka,
    // extensible
}
```

#### Source-Type-Specific Schema Blocks

**PostgreSQL:**
```rust
pub struct PostgresSchema {
    pub tables: Vec<TableDescriptor>,
}

pub struct TableDescriptor {
    pub name: String,
    pub columns: Vec<ColumnDescriptor>,
    pub row_count_estimate: Option<u64>,
}

pub struct ColumnDescriptor {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}
```

**S3 / Object Store:**
```rust
pub struct S3Schema {
    pub buckets: Vec<BucketDescriptor>,
}

pub struct BucketDescriptor {
    pub name: String,
    pub prefixes: Vec<String>,
    pub object_count_estimate: Option<u64>,
    pub size_bytes_estimate: Option<u64>,
    pub content_types: Vec<String>,
}
```

**Kafka / Streaming:**
```rust
pub struct KafkaSchema {
    pub topics: Vec<TopicDescriptor>,
}

pub struct TopicDescriptor {
    pub name: String,
    pub partition_count: u32,
    pub message_schema: Option<String>, // e.g., Avro schema JSON
}
```

New source types are added by implementing a new `DataSourceSchema` variant — the common envelope is unchanged.

### 2. Live Descriptor Updates

The advertising node is responsible for keeping its descriptor current. It polls or watches the underlying source and writes an updated CRDT doc to `data_capabilities/{capability_id}` whenever the schema changes (new table, dropped column, bucket added, etc.).

Update frequency is source-dependent and operator-configured. A reasonable default is 60 seconds for schema polling.

Because the descriptor is a CRDT document, updates propagate to all subscribed nodes via normal Automerge sync — consumers do not need to poll; they subscribe to `data_capabilities` and receive change events.

### 3. Access Control

Access is managed via **UDS RBAC** (Keycloak roles), enforced at peat-gateway. The data owner does not manage access grants directly.

```rust
pub struct DataCapabilityAccessPolicy {
    pub capability_id: Uuid,
    /// Keycloak roles that may read the descriptor (discovery)
    pub discovery_roles: Vec<String>,
    /// Keycloak roles that may request a connection
    pub consumer_roles: Vec<String>,
}
```

- **Discovery** (seeing the capability exists and its schema) requires a role in `discovery_roles`
- **Connection** (requesting access to the actual data) requires a role in `consumer_roles`

Access policies are stored and enforced by peat-gateway. A node advertising a capability registers its policy with peat-gateway at startup.

Access can be revoked by updating the policy at peat-gateway. Active consumer connections are terminated on the next access check.

### 4. Consumption Modes

There are two consumption modes. A consumer declares which mode it needs at connection time.

#### Mode A: Control-Plane-Only (Cloud / On-Prem ISV)

Peat handles discovery, access verification, and connection handoff. The actual data path is out of scope for Peat — the consumer connects to the source directly using native tooling after receiving a verified connection grant.

```
ISV App
  │
  ├─ queries peat-gateway → receives list of available DataCapabilityDescriptors
  │
  ├─ requests connection to capability_id (Keycloak token presented)
  │
  ├─ peat-gateway verifies role, issues ConnectionGrant
  │   { capability_id, connection_details, expires_at }
  │
  └─ ISV App connects directly to source using connection_details
     (e.g., a PostgREST endpoint, S3 presigned URL, Kafka broker address)
```

**Open question**: The developer experience for ISV apps consuming data in a cloud/on-prem UDS deployment is not fully defined. Specifically: what is the standard SDK surface for requesting a `ConnectionGrant` and using it? This will be resolved as the UDS Data Platform matures.

#### Mode B: Peat-Owned Data Path (Tactical Edge / ATAK)

Peat owns the full data path — fetch, local cache, and delta sync on reconnect. This is the right model for edge consumers that may go offline and need local-first access.

```
ATAK Client (peat-node, relay-capable)
  │
  ├─ subscribes to data_capabilities collection
  │
  ├─ sees MapDataCapability advertised by a connected node
  │
  ├─ requests access (Keycloak token or formation membership)
  │
  ├─ peat-gateway verifies, access granted
  │
  ├─ data blob fetched via iroh-blobs to local blob_work_dir
  │   (ADR-025 blob transfer)
  │
  ├─ consumer reads data from local cache — no connectivity required
  │
  └─ on reconnect: delta sync brings local cache up to date
     (new blobs fetched, stale blobs expired via ADR-016 TTL)
```

For Mode B, the data unit is a blob (file, archive, tile set, etc.) transferred via the existing iroh-blobs infrastructure. Structured data (e.g., a subset of a SQL table exported as Parquet) is packaged as a blob by the advertising node before transfer.

### 5. Audit Trail

All access events are captured by peat-gateway's CDC engine and emitted to configured sinks (NATS, Kafka, webhook):

| Event | Fields |
|-------|--------|
| `capability.discovered` | capability_id, consumer_node_id, timestamp |
| `capability.connection_requested` | capability_id, consumer_node_id, role, timestamp |
| `capability.connection_granted` | capability_id, consumer_node_id, expires_at, timestamp |
| `capability.connection_denied` | capability_id, consumer_node_id, reason, timestamp |
| `capability.connection_revoked` | capability_id, consumer_node_id, reason, timestamp |

The audit trail records who accessed what and when. It does not capture the content of data transferred.

---

## Consequences

### Positive

- Data sources become first-class mesh citizens — discoverable with the same pattern as AI models (ADR-018) and software packages (ADR-045)
- Live descriptors mean consumers always see the current schema without manual coordination
- RBAC via Keycloak reuses existing UDS identity infrastructure — no new auth system
- Mode B gives edge consumers local-first data access with automatic DDIL recovery
- CDC audit trail is a natural extension of peat-gateway's existing CDC engine

### Negative

- Advertising nodes must run a polling/watch loop against their source — operational overhead
- Mode A's ISV developer experience is an open question; without a clear SDK surface, adoption friction is high
- Mode B requires the advertising node to pre-package structured data as blobs, which may not suit all source types or query patterns

### Neutral

- The descriptor schema is extensible by source type, but new source types require implementation work on the advertising node side
- Access policy is stored at peat-gateway, not on the advertising node — the advertising node trusts gateway enforcement

---

## Alternatives Considered

### Advertise a query endpoint instead of a schema skeleton

Rather than exposing a schema skeleton, the advertisement could expose a query endpoint (e.g., a PostgREST URL or GraphQL endpoint). Consumers query directly.

**Rejected for now**: Requires the source to be reachable from the consumer at query time, which breaks Mode B (edge/ATAK). Schema skeleton + blob export is more DDIL-tolerant. An endpoint advertisement could be added as a future `DataSourceType` variant for always-connected environments.

### Static descriptors (snapshot at advertisement time)

Advertise the schema once and treat it as stable.

**Rejected**: Schema drift is real — tables get added, columns change type. A stale descriptor misleads consumers. Live updates are required.

### Single consumption mode

Unify Mode A and Mode B into one model.

**Rejected**: The connectivity assumptions are fundamentally different. Forcing an edge consumer through the control-plane-only model breaks offline access. Forcing an ISV through the blob transfer model adds unnecessary complexity for always-connected deployments. Both modes are valid and serve different archetypes.

---

## Open Questions

1. **ISV SDK surface (Mode A)**: What is the standard API for an ISV app to request a `ConnectionGrant` and use it? This is the primary unresolved design question for cloud/on-prem deployments.
2. **Structured query support (Mode B)**: Should the advertising node support parameterized exports (e.g., "give me rows where region = 'PACOM'") or always export full datasets? Partial exports reduce transfer size but require a query interface on the advertising node.
3. **Descriptor visibility before access grant**: Should a consumer be able to see the schema skeleton before they have `consumer_roles`? (i.e., is discovery always open within a formation, with access gating only the connection step?) Currently the ADR gates discovery separately via `discovery_roles`, but the right default is undecided.

---

## References

- ADR-018: AI Model Capability Advertisement
- ADR-025: Blob Transfer Protocol
- ADR-016: TTL and Data Lifecycle Abstraction
- ADR-045: Zarf/UDS Integration
- ADR-054: UDS Registry Replication-to-Sync for DDIL
- ADR-055: peat-gateway Enterprise Control Plane

---

**Last Updated**: 2026-05-11
**Status**: PROPOSED - Awaiting review
