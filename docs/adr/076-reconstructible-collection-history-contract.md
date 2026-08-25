# ADR-076: Reconstructible Collection History Contract

**Status**: Proposed
**Implementation Approval**: Approved for implementation under peat#1084
**Date**: 2026-08-24
**Authors**: Kit Plummer
**Related**: ADR-016 (TTL and Data Lifecycle), ADR-019 (QoS and Data Prioritization), ADR-021 (Document-Oriented Architecture), ADR-034 (Record Deletion), ADR-074 (Schema Single Source of Truth), ADR-075 (Top-Level Rust Facade)
**Tracking**: [peat#1084](https://github.com/defenseunicorns/peat/issues/1084)

---

## Context

Peat currently exposes three synchronization modes: `FullHistory`,
`LatestOnly`, and `WindowedHistory`. Those names describe how replicas exchange
state, but deployments have also inferred storage retention, replay,
provenance, and durability guarantees from them. Those inferences are unsafe.

A `LatestOnly` collection can converge on a current value after replacing its
prior causal graph. That does not preserve the observations, transformations,
or prior states that produced the value. Conversely, a `FullHistory` CRDT can
retain every causal operation without recording enough domain information to
explain why an application derived a value.

The distinction matters whenever an implementation needs replay, correction,
provenance, diagnostics, or after-action reconstruction. It also matters on
constrained nodes: a node must be able to prioritize current state and remove
an acknowledged historical copy without weakening the durability promised by
the collection contract.

Synchronization, causal retention, reconstructibility, and durability are
therefore separate interoperability concerns. They require one canonical,
transport-independent contract rather than collection-name conventions or
backend-specific configuration.

## Decision

### 1. Collections declare independent policy axes

Every production-writable collection MUST have an effective collection history
policy with these independent axes:

1. **Synchronization behavior**: complete causal synchronization,
   current-state synchronization, or bounded-window synchronization.
2. **CRDT causal retention**: retain the complete graph, retain it until a
   coordinated durable checkpoint, or permit current-state replacement.
3. **Domain-history requirement**: current state only, bounded reconstructible
   history, or complete reconstructible history.
4. **Segmentation and retention**: finite segment rotation limits and, for
   bounded history, the declared retention interval.
5. **Durability**: the persistence target that must acknowledge a sealed
   segment before local removal can be considered.
6. **Over-budget behavior**: retain locally, backpressure the producer, or
   reject the write.
7. **Epoch recovery behavior**: reject stale writers with the active successor
   epoch and reject, quarantine, or remove-again resurrected expired history.

No axis silently weakens another. In particular, a synchronization choice does
not establish a domain-history or durability guarantee.

### 2. Sync modes have narrow meanings

- `LatestOnly` guarantees convergence of the current state among participating
  replicas operating under compatible policy. It does not guarantee replay,
  provenance, derivation, or correction history.
- `FullHistory` preserves the causal operations required by the selected CRDT
  synchronization algorithm. It does not by itself establish that those
  operations are a sufficient domain event record.
- `WindowedHistory` bounds synchronization to a declared window. It does not by
  itself bound local storage or authorize removal of causal or domain history.

Raw CRDT operations count as reconstructible domain history only when the
collection contract and schema explicitly define a complete, deterministic
mapping from those operations to the domain events required for reconstruction.

### 3. Reconstructible history uses an explicit history source

A collection that claims bounded or complete reconstructibility MUST identify
a history source. The reference is generic and contains a collection identity
and stable source identity, with optional checkpoint and segment-catalog
collections; it does not encode a track, telemetry, or other domain-specific
primitive.

A common representation is:

```text
current:{entity}                    current-state projection
history:{entity}:{epoch}:{segment} immutable history segment
checkpoint:{entity}:{epoch}        optional reconstruction checkpoint
```

The current projection may use `LatestOnly`. The history source contains the
domain events or states needed to reconstruct it. Optional checkpoints reduce
replay cost but do not authorize deletion of required events unless the
collection's retention and durability policies independently permit deletion.

### 4. History is segmented and immutable after sealing

Reconstructible history MUST use finite segments rather than one indefinitely
mutable document. A segment rotates when any configured time, event-count,
serialized-byte, or revision limit is reached.

Segments follow this lifecycle:

```text
Active -> Sealed -> Durably Acknowledged -> Retention Eligible -> Removed
```

- **Active** segments accept events.
- **Sealed** segments are immutable. Corrections are new events or segments.
- **Durably Acknowledged** means the declared persistence target has confirmed
  storage. Attempted transmission, receipt, or observation is not an
  acknowledgement.
- **Retention Eligible** means both durability and retention requirements are
  satisfied.
- **Removed** means a particular replica has evicted its eligible local copy.
  It does not imply a mesh-wide deletion unless a separate deletion contract
  authorizes that operation.

Every segment descriptor carries a stable source, epoch, and segment identity;
an inclusive source-local sequence range; optional time coverage and
predecessor/checkpoint references; and, once sealed, a content encoding and
SHA-256 content identity. The digest covers the exact immutable payload bytes
stored and transferred under the declared content encoding; it does not cover
the descriptor and is not computed from a non-canonical re-encoding.
These fields let heterogeneous implementations discover and order segments,
detect gaps, verify immutable content, resume after restart, and fence stale
writers without interpreting domain payloads.

Complete history has no time-based retention expiry. Bounded history declares
a positive retention interval. A local-only durability target never authorizes
removing its sole durable copy.

### 5. Failure cannot silently weaken history

If a node cannot satisfy the durability target or storage budget, it MUST apply
the declared over-budget behavior: retain locally, backpressure, or reject.
It MUST NOT accept a write under a reconstructible-history claim and then
silently discard the only qualifying copy.

During a partition, a node may continue accepting writes only while it can
honor the declared local retention and admission budget. There is no protocol
that can guarantee complete history while allowing every isolated replica to
delete its only copy.

### 6. Recovery and stale replicas are explicit

Segment identity and rotation boundaries MUST be stable across restart.
Sealed segments MUST remain immutable after rejoin. Each history source has one
active epoch. Closing an epoch persists its successor identity before the old
epoch rejects further writes. A write addressed to a closed epoch MUST be
rejected with the active successor epoch; it MUST NOT be silently redirected
because that would obscure the producer's ordering decision.

Every explicit policy chooses how history that returns after retention expiry
is handled: reject it, quarantine it for operator review, or remove it again.
Unknown or unspecified resurrection behavior resolves to quarantine. A stale
replica cannot make returned history live merely by reconnecting.

A coordinated checkpoint or epoch transition may replace causal history only
when it defines the active replica set, concurrent-write fencing, checkpoint
identity, persistent epoch metadata, stale-replica handling, and sync-state
reset. A quorum acknowledgement alone does not make arbitrary pre-checkpoint
replicas causally compatible.

Rolling versions that do not understand this contract resolve unspecified or
unknown policy conservatively: preserve causal history and do not authorize
history removal, reject writes to unknown epochs, and quarantine returned
expired history. They MUST NOT interpret an unknown value as `LatestOnly` or as
permission to discard data.

### 7. Policy and enforcement are observable

Implementations MUST expose:

- the effective policy and whether it is explicit, defaulted, or migrated;
- validation and enforcement state;
- active and sealed segment state;
- active/closed epoch state and successor identity;
- durability target and acknowledgement progress;
- retention eligibility, its not-before time, and evaluation time; and
- admitted, retained, backpressured, and rejected writes.

Production startup validation MUST reject missing or incoherent declarations
for writable collections once the operator surface supports this contract.
Library boundaries may retain a conservative undeclared default during
migration.

### 8. Invalid combinations fail validation

At minimum, these declarations are invalid:

- bounded or complete reconstructibility without a history source;
- bounded reconstructibility without finite rotation and a positive retention
  interval;
- complete reconstructibility with a finite retention expiry;
- a windowed synchronization mode without a positive window;
- full-history synchronization with replaceable current-state causal retention;
- windowed synchronization without retained causal history for its window;
- replicated durability without a meaningful replica count;
- local removal before durability acknowledgement;
- retention eligibility before the durability target is met; and
- retention eligibility before its declared not-before time;
- any reconstructible policy whose over-budget behavior can silently drop an
  accepted event.

Validation reports the field and violated invariant. It never rewrites an
explicit invalid declaration into a weaker valid one.

### 9. Shared type ownership

Wire-visible policy and status types originate in `peat-schema` under a
versioned package, per ADR-074. `peat-protocol` exposes transport-independent
semantic APIs and compatibility conversion. The top-level `peat` crate may
eventually re-export those components as a facade but does not own foundational
types, per ADR-075.

Storage and synchronization implementations consume the schema contract and
enforce it. Operator-facing services expose configuration and status. No
sibling repository defines a competing canonical type.

Protobuf binary is the canonical cross-language encoding for these types. JSON
bindings MUST follow the standard Protobuf JSON mapping; implementation-local
Serde representations are not an independent wire contract.

## Compatibility

The existing sync modes remain compatibility inputs:

| Legacy mode | Synchronization mapping | Other policy claims |
|---|---|---|
| `FullHistory` | Complete causal synchronization | No domain reconstructibility claim without an explicit history source |
| `LatestOnly` | Current-state synchronization | Current state only unless a separate history source is declared |
| `WindowedHistory(n)` | Bounded-window synchronization for `n` seconds | No bounded local-retention claim |

Unknown or undeclared legacy configuration maps to the conservative
`FullHistory`-compatible synchronization behavior and authorizes no history
removal. Existing public Rust paths may remain during migration, but canonical
new policy types flow from `peat-schema`.

## Relationship to Earlier Decisions

- **ADR-016**: local eviction remains valid only when it does not violate the
  effective history and durability policy. "Can re-sync" is not sufficient
  evidence that another durable copy exists.
- **ADR-019**: sync modes remain useful scheduling inputs, but no longer imply
  reconstructibility or local retention.
- **ADR-021**: one living document remains the current-state pattern. Immutable
  finite history segments are an intentional separate document model.
- **ADR-034**: uncoordinated tombstone or TTL garbage collection cannot remove
  history required by this contract before durability and stale-peer safety
  conditions are met.
- **ADR-074**: `peat-schema` is the single source of truth for the wire-visible
  contract.
- **ADR-075**: component crates consume `peat-schema`; they do not depend on the
  top-level facade.

Where earlier language conflicts with this decision, this ADR governs
collection-history, reconstructibility, and durability semantics.

## Consequences

### Positive

- Users can distinguish current-state convergence from historical
  reconstructibility.
- Constrained replicas can evict acknowledged history without requiring every
  node to retain an archive.
- Durability claims become testable rather than inferred from transmission.
- Backends can implement the same contract without sharing a CRDT or transport.
- Invalid combinations fail before field history is silently lost.

### Negative

- Collection owners must declare more than one policy axis.
- History segmentation, catalogs, checkpoints, and stale-writer fencing add
  implementation complexity.
- Producers must handle visible backpressure or rejection.
- Existing collection-name defaults require an explicit migration period.

## Implementation Tracking

- [peat#1085](https://github.com/defenseunicorns/peat/issues/1085): shared schema and semantic APIs
- [peat-mesh#390](https://github.com/defenseunicorns/peat-mesh/issues/390): storage, synchronization, and durability enforcement
- [peat-node#237](https://github.com/defenseunicorns/peat-node/issues/237): operator configuration and status
