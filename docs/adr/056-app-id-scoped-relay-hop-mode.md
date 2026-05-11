# ADR-056: App-ID-Scoped Relay Hop Mode

**Status**: Proposed
**Date**: 2026-04-29
**Authors**: Austin Ruth
**Related ADRs**:
- [ADR-007](007-automerge-based-sync-engine-updated.md) (Automerge-based Sync Engine)
- [ADR-009](009-bidirectional-hierarchical-flows.md) (Bidirectional Hierarchical Flows)
- [ADR-042](042-direct-udp-bypass-pathway.md) (Direct-to-UDP Bypass Pathway)
- [ADR-046](046-targeted-message-delivery.md) (Targeted Message Delivery)
- [ADR-048](048-membership-certificates-tactical-trust.md) (Membership Certificates / Tactical Trust)

---

## Context

### Problem Statement

Peat nodes participate in CRDT sync only for collections they have explicitly subscribed to. A node that has no local application interest in a collection cannot relay changes for that collection to other peers — sync messages simply go nowhere if no document state exists locally.

This creates a hard gap in the mesh for **bridge/relay nodes**: platforms (e.g., a UAV in a MANET hop role) that are positioned between two nodes which cannot communicate directly, but whose only job is to pass traffic through — not to consume it.

```
Ground-A ◄──── out of range ────► Ground-B
    │                                  │
    └──────── UAV-Relay ───────────────┘
```

If Ground-A writes a `nodes` document and UAV-Relay has no subscription to `nodes`, Automerge has no local document state to sync from — the change never reaches Ground-B.

### Current Architecture

The `SyncForwarder` in `peat-mesh` routes sync batches based on `SyncDirection` (derived from collection name) and hierarchy position:

| Collection | Direction | Forwarding Rule |
|---|---|---|
| `nodes`, `beacons`, `platforms`, `summaries` | Upward | Forward to parent only |
| `commands` | Downward | Forward to children only |
| `cells` | Lateral | Forward to same-level peers only |
| `alerts`, `contact_reports`, `events` | Broadcast | Forward to all connected peers |

These direction rules are **applied at every hop**, not only at destinations. A relay node that lacks a meaningful hierarchy position (no configured parent, no children, no lateral peers) for a given direction will drop the batch entirely — it is not a recognized participant in that direction of flow.

Furthermore, Automerge sync is document-oriented: to forward an Automerge sync message for a document, a node must have loaded that document. Without a local subscription, the document is not in the node's store, so the sync protocol cannot operate on it.

### Requirements

1. A node must be able to relay all sync traffic within its `app_id` without requiring explicit local subscriptions
2. This behavior must default to **off** — only relay nodes opt in
3. The relay should be configurable at both the **per-node** and **formation** level
4. Operators must be able to **deny-list specific collections** that should never be relayed by a given node
5. The relay node should not be required to store documents it has no application interest in (pass-through semantics preferred)
6. The relay node should not need to decrypt document content to forward it (opaque relay preferred)

---

## Decision

### Relay Mode Configuration

Introduce a `RelayConfig` struct that can be attached to both node-level and formation-level configuration:

```rust
/// Controls whether a node acts as a relay hop for all app_id traffic
#[derive(Debug, Clone, Default)]
pub struct RelayConfig {
    /// When true, this node relays all sync traffic within its app_id
    /// regardless of local subscriptions.
    /// Default: false
    pub enabled: bool,

    /// Collections this node will never relay, even in relay mode.
    /// Useful for excluding sensitive or high-volume collections from
    /// a resource-constrained relay.
    /// Default: empty (relay all collections)
    pub collections_denylist: Vec<String>,
}
```

#### Node-Level Configuration

```rust
pub struct SidecarConfig {
    pub app_id: String,
    pub shared_key: String,
    // ... existing fields ...

    /// Relay mode for this node instance
    pub relay: RelayConfig,
}
```

#### Formation-Level Configuration

A formation policy may designate nodes as relay-capable, which overrides or supplements per-node config:

```rust
pub struct FormationRelayPolicy {
    /// Nodes in this formation act as relays if they have no active
    /// local subscriptions. Allows the formation to self-organize
    /// relay roles without per-node static config.
    pub auto_relay_unsubscribed_nodes: bool,

    /// Collections excluded from relay at the formation level,
    /// applied to all relay nodes in the formation.
    pub formation_denylist: Vec<String>,
}
```

The effective deny-list for a relay node is the union of its own `collections_denylist` and the formation's `formation_denylist`.

#### YAML Configuration

```yaml
# peat-node config
relay:
  enabled: true
  collections_denylist:
    - large_blobs
    - audit_logs
```

```yaml
# formation config
relay_policy:
  auto_relay_unsubscribed_nodes: false
  formation_denylist:
    - audit_logs
```

### Forwarding Behavior: Direction-Blind Relay

When relay mode is enabled, a node forwards all received sync batches to **all connected peers within the same `app_id`**, except the source peer. It does not apply `SyncDirection` rules.

**Why direction-blind?**

A relay node may sit at the boundary between two sub-meshes at different hierarchy levels. Applying directional rules at the relay would require it to have a well-defined hierarchy position for every collection type — which defeats the purpose of a lightweight pass-through node. Directional correctness is enforced at **destination nodes**, which do have subscriptions, hierarchy positions, and proper formation membership.

```
Ground-A (source)
    │
    │  writes nodes document (SyncDirection: Upward)
    │
UAV-Relay (relay mode: enabled)
    │
    │  relay_mode = true → skip direction check
    │  forward to all connected peers except Ground-A
    │
Ground-B (destination)
    │
    │  receives sync batch
    │  applies own SyncDirection::Upward rule
    │  persists if hierarchy position allows
```

Loop prevention uses existing mechanisms:
- **Hop count / TTL** — already present on `DataPacket` (max_hops) and in the peat-btle `RelayEnvelope`
- **Deduplication cache** — `SyncForwarder`'s LRU cache on batch ID prevents re-forwarding the same batch

---

## Open Question: Common Persistence vs. Opaque Transport Relay

This is the most significant unresolved design question for implementation.

### The Tension

Automerge's sync protocol is **convergence-based**, not forwarding-based. Two peers synchronize by exchanging delta change-sets and both converging to the same document state. This means:

- A node can only participate in a document's sync if it has loaded that document locally
- "Relaying" in the Automerge model is not byte-forwarding — it is **common persistence followed by natural re-sync**

The two candidate implementations are:

#### Option A: Common Persistence

The relay node subscribes to all non-denied collections within its `app_id`. Documents are written to its local Automerge store. The existing `SyncForwarder` then propagates them to all connected peers via normal CRDT sync.

```
Ground-A → [Automerge sync] → UAV-Relay (stores doc) → [Automerge sync] → Ground-B
```

**Pros:**
- Architecturally native to Peat — no new code paths in the sync engine
- Full CRDT guarantees at every hop (conflict resolution, convergence)
- Relay node can serve as a durable buffer during intermittent connectivity

**Cons:**
- Relay node stores documents it has no application interest in — storage cost on a resource-constrained platform
- Storage grows unbounded without a TTL/eviction policy (requires integration with ADR-016 lifecycle management)
- Relay node holds plaintext document content (if documents are decrypted at rest)

#### Option B: Opaque Transport Relay

The relay node does not load documents into Automerge. Instead, a new forwarding path at the transport layer receives raw sync bytes from one peer and dispatches them to other connected peers within the same `app_id`, without parsing or decrypting the payload.

```
Ground-A → [raw sync bytes] → UAV-Relay (does not store) → [raw sync bytes] → Ground-B
```

**Pros:**
- Zero storage cost on relay node
- Relay node never sees plaintext content (consistent with opaque relay security model)
- Simpler resource profile for UAV-class platforms

**Cons:**
- Bypasses the Automerge protocol — the relay is not a true CRDT participant
- Requires a new code path outside the existing sync engine
- During network partitions, relay cannot serve as a re-sync point for lagging peers (no local state to diff against)
- Deduplication must be implemented separately (cannot rely on Automerge's built-in change-set dedup)

#### Recommendation

**Option A (common persistence) is recommended as the initial implementation** for the following reasons:

1. It is architecturally honest with the Peat CRDT model and requires no new sync engine code paths
2. Storage cost is bounded in practice: relay nodes on tactical networks typically connect to 2–5 peers; document sets are small
3. ADR-016 TTL semantics already provide an eviction mechanism — relay-mode nodes can apply aggressive TTLs to non-subscribed collections
4. Common persistence turns relay nodes into durable buffers, which is operationally valuable in DDIL environments where Ground-A and Ground-B may not be simultaneously connected to UAV-Relay

Option B should be revisited if profiling shows that common persistence is untenable on the most resource-constrained platforms in the formation.

---

## Consequences

### Positive

- Bridge nodes can relay all app_id traffic without per-collection subscription configuration
- Formation topology becomes more resilient — any node can act as a relay hop when positioned between disconnected peers
- Configuration is opt-in and default-off — no behavioral change for existing deployments
- Collection deny-listing gives operators fine-grained control over what a constrained relay node will carry

### Negative

- Common persistence: relay nodes accumulate documents they have no application interest in; requires TTL discipline
- Direction-blind forwarding increases bandwidth on relay nodes (they forward everything, not just directionally relevant batches)
- Debugging mesh propagation becomes more complex when some nodes are relay-mode and others are not

### Neutral

- Formation-level relay policy requires formation coordinator to know which nodes have no active subscriptions — this may require a lightweight capability advertisement in the beacon schema
- Relay mode interacts with ADR-046's `TransitBehavior::RelayOnly`: targeted documents with relay-only transit already expect intermediate nodes to forward without persisting; relay mode is the same concept applied at the node level rather than the document level

---

## Alternatives Considered

### 1. Subscribe to All Collections Manually

Operators configure relay nodes with explicit subscriptions to every collection.

**Rejected**: Requires operators to know and enumerate all collection names at deployment time. Fragile as collections are added by application teams. Relay mode automates this.

### 2. Direction-Aware Relay (Require Hierarchy Position)

Relay nodes apply `SyncDirection` rules, requiring them to have a configured parent, children, and lateral peers.

**Rejected**: Defeats the purpose of a lightweight pass-through node. A relay positioned between two ground nodes has no natural hierarchy membership — forcing one creates operational configuration burden.

### 3. Formation-Level Only (No Per-Node Flag)

Only allow relay mode to be set at the formation level, not per-node.

**Rejected**: Prevents single-node relay deployments and makes it harder to pre-configure known relay platforms (e.g., a UAV that is always a relay) without formation context at startup.

### 4. Permanent Broadcast Subscription for Specific Collections

Mark certain collections (e.g., `nodes`, `commands`) as always-relayed, removing the per-node opt-in.

**Rejected**: Changes default behavior for all existing nodes, violates the principle that relay behavior is a deployment-time opt-in.

---

## Security Considerations

- **app_id boundary is preserved**: Relay mode only forwards batches within the node's own `app_id`. Cross-app_id forwarding is never performed.
- **Formation key (shared_key) still gates participation**: A relay node must share the same `shared_key` as the formation to participate in sync at all. No new trust surface is introduced.
- **Common persistence and plaintext**: If document content is encrypted at rest (AES-256-GCM via peat-node), relay nodes store ciphertext only. If encryption at rest is not enabled, relay nodes will hold plaintext content for all relayed collections — operators should account for this in threat models.
- **Opaque relay option**: If Option B (opaque transport relay) is implemented, relay nodes never hold decrypted document content, which is preferable for platforms with a higher compromise risk profile.
- **Deny-list integrity**: The `collections_denylist` is a local config value and is not cryptographically enforced by the formation. A misconfigured or compromised relay node could ignore its deny-list. This is acceptable given that formation key membership already gates who can participate.

---

## Implementation Plan

### Phase 1: Configuration Schema

- [ ] Add `RelayConfig` struct to `peat-mesh` sync types
- [ ] Wire `RelayConfig` into `SidecarConfig` in `peat-node`
- [ ] Add `FormationRelayPolicy` to formation coordinator
- [ ] YAML configuration parsing support

### Phase 2: Direction-Blind Forwarding

- [ ] Extend `SyncForwarder::forward_targets()` to detect relay mode
- [ ] When relay mode enabled: return all connected peers (same app_id) except source, bypassing `SyncDirection` logic
- [ ] Apply effective deny-list (node + formation union) before returning targets
- [ ] Verify existing hop count and dedup cache cover relay forwarding paths

### Phase 3: Common Persistence (Option A)

- [ ] When relay mode enabled, auto-subscribe relay node to all non-denied collections
- [ ] Apply aggressive TTL defaults for auto-subscribed (relay-only) collections via ADR-016 lifecycle
- [ ] Add `relay_only: bool` tag to auto-subscriptions so the node does not surface these documents to local application subscribers

### Phase 4: Beacon Advertisement

- [ ] Add `relay_mode_enabled: bool` field to beacon schema
- [ ] Formation coordinator uses this to identify relay-capable nodes for auto-relay policy

### Phase 5: Observability

- [ ] Metrics: bytes relayed, collections relayed, peers served via relay
- [ ] Log relay forwarding decisions at debug level with source/destination peer IDs

---

## References

- ADR-007: Automerge-based Sync Engine
- ADR-009: Bidirectional Hierarchical Flows
- ADR-016: TTL and Data Lifecycle Abstraction
- ADR-042: Direct-to-UDP Bypass Pathway
- ADR-046: Targeted Message Delivery (`TransitBehavior::RelayOnly` — analogous concept at document level)
- ADR-048: Membership Certificates / Tactical Trust

---

**Last Updated**: 2026-04-29
**Status**: PROPOSED - Awaiting review
