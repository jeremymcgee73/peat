# ADR-071: Subscription-Based Convergence (Interest-Driven Distribution)

**Status**: Proposed
**Date**: 2026-06-19

## Executive Summary

Distribution today is **sender-enumerated**: the writer chooses a scope
(all peers, a node list, a formation) and `resolve_targets` turns that into an
explicit `target_nodes` list baked into the distribution document; a receiver
acts only if it finds itself in that list. This conflates *membership*
(credentials + connectivity) with *need*, and it cannot reach a node the sender
isn't directly peered with.

This ADR changes the model to **interest-driven convergence**: the writer
publishes *availability* and nothing about recipients. Each node decides
**locally** whether it needs the data — a receiver-evaluated, sender-ignorant
predicate — and pulls it if so. The predicate is **pluggable**: subscription is
the first input; version-gap and capability advertisement are future inputs to
the *same* decision. A separate, explicit **directed-send** path (address a
specific node by name) is retained for the cases that genuinely need push.

## Context

### Current model

`IrohFileDistribution::resolve_targets` (peat-protocol) maps every
`DistributionScope` to the blob store's `known_peers()` — i.e. the set of
peers this node has directly dialed:

- `AllNodes` → all `known_peers`
- `Nodes { ids }` → requested ids filtered to `known_peers`
- `Formation` / `Capable` → unimplemented; fall back to `known_peers`

The result is written into the distribution document as `target_nodes`. The
receive watcher delivers iff `target_nodes.contains(own_short_id)`.

### Problems

1. **Membership ≠ need.** Being in the formation and connected does not mean a
   node wants a given blob. Targeting by `known_peers` distributes to peers
   that may have no interest, and (because `known_peers` is direct-dial only)
   *fails to reach* interested nodes that are reachable only transitively.
2. **The sender must know the recipients.** Enumerating `target_nodes` couples
   the writer to mesh topology and membership it shouldn't have to track. The
   canonical counter-example: a node needs a newer model version *because it
   holds an older one* — a fact only the receiver knows. The writer should only
   have to publish "version N is available."
3. **No path to capability-based need.** Capability advertisement (a future
   Peat-protocol aspect) will also drive who needs what. A membership-scope
   enum has no room for it without another bespoke branch.

The provider-gossip work (separate change in the mesh layer) already supplies
the **"who has it"** half: holdings propagate mesh-wide so any node can locate a
blob beyond its direct peers. What is missing is the **"who needs it"** half,
evaluated correctly.

## Decision

### 1. Need is receiver-evaluated and sender-ignorant

The writer publishes *availability*: the blob, its content hash, and the
**collection/topic it belongs to**. It does **not** enumerate recipients.

Each node independently evaluates a **need predicate** against its own state
and, if satisfied, fetches the blob (locating a holder via provider gossip).
The predicate is gated always by **"I don't already have it."**

### 2. The need predicate is pluggable

Need is a single receiver-side evaluation with multiple inputs, composed over
time. Inputs, in delivery order:

1. **Subscription** (this ADR, Phase 1) — the node has registered durable
   interest in the collection/topic the data is published under.
2. **Version-gap** (Phase 2) — the node holds an older version of a versioned
   artifact than the one published.
3. **Capability** (Phase 3) — the node's advertised capability matches the
   data's capability requirement.

All three feed the same "do I need this?" check. New inputs are added without
disturbing existing ones or the wire format of a distribution.

### 3. Two distribution paths

- **Interest-driven convergence (pull) — primary.** Publish availability under
  a collection/topic; subscribers that lack it converge it. No `target_nodes`.
- **Directed send (push) — explicit.** Address a specific node by name when the
  writer genuinely must target one recipient. This maps to the existing
  node-list scope, which is **retained** as the directed-send primitive.

The membership-broadcast scopes (`AllNodes`, `Formation`) are **subsumed** by
convergence: "everyone who is subscribed" replaces "every peer."

### 4. Durable interest is a subscription, not an ephemeral stream

Interest that drives convergence must persist independent of any live observer:
a node converges the data it cares about whether or not a consumer is currently
watching a change stream. The durable subscription is therefore first-class
node state, registered through the sidecar's gRPC surface and read by the
convergence watcher. An open change-stream is an *observer* of already-converged
state, not the expression of durable interest.

## Architecture

```
writer:  publish(blob, hash, collection)        # availability only
            └─► distribution document gossips mesh-wide (metadata)

each node (receiver-self-select):
    on observing a distribution doc:
        need = NeedEvaluator::needs(doc, local_state)   # pluggable
             = subscribed(doc.collection)               # Phase 1
               [ ∨ version_gap(doc) ∨ capability(doc) ]  # Phase 2/3
        if need && !have(doc.hash):
            fetch(doc.hash)        # holder located via provider gossip
            # acquiring it re-announces holding → next-hop convergence
```

- **Distribution document** gains a `collection` (and, later, version /
  capability descriptors). `target_nodes` is no longer the delivery gate for
  the convergence path; it remains meaningful only for directed send.
- **`NeedEvaluator`** is the receiver-side abstraction — one trait, one
  `needs(...)` decision, with subscription as the first implementation. This is
  the extension point for version-gap and capability.
- **Durable subscription registry** is node-local state (the set of
  collections/topics the node has registered interest in), persisted and read
  by the convergence watcher. Exposed through the sidecar gRPC surface.
- **Provider gossip** (mesh layer, already landed) supplies holder discovery so
  a needing node can fetch from a non-adjacent holder.

### Completion semantics

A distribution is "converged" when every node that *needs* it holds it. Because
need is receiver-evaluated and members may be absent, convergence is **eventual**
(DDIL store-and-forward): an interested node that is offline at publish time
converges when it returns. Completion is therefore not a fixed denominator known
at send time; status is expressed against the set of nodes observed to need it.
Retention bounds how long an unconverged distribution is held.

## Phasing

- **Phase 1 — subscription-based convergence (this ADR's initial delivery).**
  `collection` on the distribution; `NeedEvaluator` with the subscription input;
  durable subscription surface on the sidecar; receive watcher switches from
  `target_nodes` membership to the need predicate. Directed send retained.
- **Phase 2 — version-gap need.** Versioned artifacts; receiver needs a newer
  version because it holds an older one.
- **Phase 3 — capability advertisement.** Capability match as a need input.

## Consequences

**Positive**
- Writers decouple from membership and topology; they publish availability only.
- Interested nodes beyond direct peers converge (with provider gossip).
- One extensible need decision instead of a membership-scope enum that must grow
  a branch per targeting mode.
- Directed send remains available for the cases that genuinely need it.

**Negative / risks**
- Completion semantics shift from a fixed target set to an eventual,
  interest-relative notion; status reporting and retention must adapt.
- Durable interest is new persisted state with its own lifecycle (register /
  deregister / GC).
- Subscription registry consistency: receiver-self-select needs only *local*
  interest, so no interest gossip is required in Phase 1 — but directed send and
  status reporting may later want a gossiped view of who-needs-what; deferred.

## Cross-repo impact

- **peat-protocol**: `NeedEvaluator` abstraction; `collection` on the
  distribution document; receive watcher uses the need predicate.
- **Sidecar gRPC surface**: durable subscription registration; `collection`
  binding on publish; membership-broadcast scopes deprecated in favor of
  convergence; node-list retained for directed send.
- **Mesh layer**: none for Phase 1 (provider gossip already present).

Implementation lands as one issue/PR per repo, linked through a tracking issue.
