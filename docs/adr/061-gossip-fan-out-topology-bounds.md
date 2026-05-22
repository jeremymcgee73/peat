# ADR-061: Gossip Fan-Out Topology Bounds for Automerge Sync

**Status**: Proposed
**Date**: 2026-05-22
**Authors**: Kit Plummer
**Amends**: ADR-017 §Layer 2 "Mesh Topology Management" — adds the four topology classes (hub-and-spoke / fully-connected / partial-mesh / singleton) and their bandwidth envelopes under relay-on-remote-apply gossip. The rest of ADR-017 remains valid.
**Related**: ADR-007 (CRDT-Based Sync Engine — Automerge selection; this ADR records an operational constraint on how Automerge sync is run but does not amend the backend-evaluation framework itself), peat#891 (field report), peat#907 (architectural tracking), peat-mesh#151 (origin-tagged change broadcast, rc.16), peat#909 (consumer + integration test)
**Triggered by**: peat-mesh#151 QA review (bandwidth amplification trace, ARCH finding); peat#910 (ADR escalation)

---

## Context

Prior to peat-mesh rc.16 / peat#909, the Automerge propagation task in
`peat-protocol/src/storage/automerge_backend.rs` (Phase 6.5) fired only on
`ChangeOrigin::Local` events — local writes were pushed to all directly-connected
peers, and a doc received from peer X never propagated onward to peer Y.
Hub-and-spoke topologies (alpha as hub, bravo and charlie as spokes pointed only at
alpha) therefore deadlocked at "each spoke sees alpha but never the other spoke"
indefinitely. The `docs/guides/QUICKSTART.md` Scenario 2 / Scenario 4 promise of
"transitive sync via gossip" was undelivered in code.

peat-mesh#151 (rc.16) closed the upstream half by adding an origin-tagged change
broadcast (`subscribe_to_changes_with_origin` returning `DocChange { key,
ChangeOrigin }`). peat#909 closed the consumer half: the propagation task now
fires on **every** `ChangeOrigin::Remote(src)` event and pushes the doc to every
connected peer **except** `src`. With both halves in place, hub-and-spoke
converges transitively and the QUICKSTART promise is delivered.

But the new behavior fires in **all** topologies, not just hub-and-spoke. peat#910
(filed against the architectural fan-out semantics, escalated from peat-mesh#151's
QA review) traced the bandwidth cost in a fully-connected mesh:

> In a fully-connected N-node mesh a single write generates O(N²) sync
> negotiations before the 2-second debounce window closes, versus O(N) in the
> previous release.

The CRDT layer is idempotent under echo — `generate_sync_message` returns `None`
when the peer is already current, so no document bytes are re-sent — but the
sync-state handshake overhead accrues per event. On bandwidth-constrained links
(BLE mesh, tactical radio at 30–230 kbps) and at high write rates (10 Hz
telemetry, hierarchical-aggregation bursts), this overhead is material.

This ADR exists to (1) classify the topologies the new behavior affects, (2)
derive the bandwidth bound per class, (3) define the applicability envelope below
which the new behavior stays within the ecosystem's 20% bandwidth-baseline
criterion, (4) enumerate mitigation options and pick one, and (5) amend ADR-007 /
ADR-017 with the decision.

---

## Topology classification

The deployments peat actually sees fall into four mesh classes. The propagation
task's per-event fan-out cost differs by class.

### Hub-and-spoke

One node is connected to N−1 others; the N−1 are not directly connected to each
other. Convergence between two leaves requires the hub to relay. This is the
QUICKSTART Scenario 2 / Scenario 4 case and the motivating scenario for #891.

**Runtime predicate**: a node is a "hub" for a doc D if it is the only peer with
an open sync session to ≥2 other peers, none of which are connected to each
other. The runtime does not currently compute this — peers know their own
adjacency list, not the peer-graph.

### Fully-connected mesh

Every node has a direct sync session to every other node. Convergence between any
two nodes is one-hop; transitive gossip is redundant. This is the small-cluster
case (≤7 nodes, the default `max_connections` cap from
`AutomergeIrohBackend`).

**Runtime predicate**: a node is in a fully-connected mesh for a doc D if every
peer it learns about via the origin-tagged broadcast (`Remote(src)`) is itself
present in the node's `transport.connected_peers()` list.

### Partial mesh (rings, chains, trees, dense-with-holes)

Some pairs of nodes have direct edges; others depend on relay. Real-world
example: laptop alpha (well-connected) + Pi bravo (well-connected) + LoRa charlie
(narrowband, one edge to bravo only).

**Runtime predicate**: a node is in a partial mesh for a doc D if at least one
`Remote(src)` peer is NOT in `connected_peers()`. Gossip is load-bearing for
those pairs.

### Singleton / two-node

N < 3. Gossip is trivially redundant; only direct sync applies.

---

## Bandwidth bound per topology

The trace from peat#910 (3-node fully-connected mesh, bravo writes), reproduced
here for the record:

| Step | Event                            | Sender → Receiver(s)               |
|------|----------------------------------|------------------------------------|
| 1    | Local write on bravo             | bravo → alpha, charlie             |
| 2    | `Remote(bravo)` on alpha         | alpha → charlie                    |
| 3    | `Remote(bravo)` on charlie       | charlie → alpha                    |
| 4    | `Remote(alpha)` on charlie       | charlie → bravo                    |
| 5    | `Remote(charlie)` on alpha       | alpha → bravo                      |
| 6    | `Remote(charlie)` on bravo       | bravo → alpha (debounce-suppressed)|
| 7    | `Remote(alpha)` on bravo         | bravo → charlie (debounce-suppressed)|

Pre-fix: 2 syncs. Post-fix: 5 wire-bound syncs (1, 2, 3, 4, 5) + 2
debounce-suppressed (6, 7).

Worst-case sync negotiations per write event, by topology:

| Topology               | Pre-fix | Post-fix          | Notes |
|------------------------|---------|-------------------|-------|
| Singleton / two-node   | 0 / 1   | 0 / 1             | No change. |
| Hub-and-spoke (N)      | 1       | N − 1             | Hub relays once per spoke; previously did not relay at all. |
| Fully-connected (N)    | N − 1   | O(N²) per ref-trace | Each receive event fans out N − 1 more; per-(doc, peer) debounce caps intra-window re-fires per source but not the cross-source amplification. |
| Partial mesh (general) | O(\|E\|)| O(\|E\| · D)      | D = mean graph diameter; each relay hop fires another fan-out at the next hop. |

The CRDT-layer no-op (`generate_sync_message → None` after the peer's
`sync_state` knows the receiver's heads) caps the *byte cost* of the redundant
syncs in the fully-connected case to near-zero document bytes, but the
sync-state handshake itself is several hundred bytes per peer per event. At 10
events/second (telemetry burst) on a 6-node mesh the steady-state overhead is
~30 sync-state handshakes/second/peer = ~10 KB/s/peer of handshake noise.

---

## Applicability envelope

Define the bandwidth envelope below which the rc.16+#909 behavior stays within
the ecosystem's 20% bandwidth-baseline criterion:

- **Hub-and-spoke**, any N: always within envelope. Gossip is the intent; the
  amplification factor is 1× because every relay is load-bearing.
- **Fully-connected mesh**, **N ≤ 4**, write rate **≤ 2 Hz** per node, link
  bandwidth ≥ 256 kbps: within envelope. Overhead ≤ 8 syncs/s/peer × ~256 bytes
  ≈ 2 KB/s — well under 20% of 256 kbps.
- **Fully-connected mesh**, **N ≤ 7** (the default max_connections cap), write
  rate **≤ 0.5 Hz** per node: within envelope by the same calculation.
- **Fully-connected mesh**, **N > 7 OR write rate > 2 Hz**: **out of
  envelope**. Mitigation required (see below).
- **BLE-class links** (peat-btle mesh, link bandwidth 30–230 kbps): the
  20% baseline budget is 6–46 kbps = 750 B/s – 5.75 KB/s. At ~256 bytes
  per sync-state handshake and ~(N−1) handshakes per write event on a
  fully-connected N-node mesh, the bound is:
  - N = 3, write rate ≤ 0.5 Hz, link ≥ 30 kbps: borderline within
    envelope (≈ 65% of the 750 B/s budget).
  - N = 3, write rate ≤ 1 Hz, link ≥ 60 kbps: within envelope.
  - N ≥ 4 fully-connected at any write rate on a 30 kbps link: **out of
    envelope** — handshake overhead exceeds the 20% baseline before any
    application data flows.
  - Any partial-mesh BLE deployment with relay hops: case-by-case;
    multi-hop on narrowband is almost always out of envelope and should
    use the existing peat-btle `mesh-translator` path, not transitive
    Automerge gossip.
- **Partial mesh** (non-BLE), mean diameter ≤ 3: within envelope.
- **Partial mesh** (non-BLE), mean diameter > 3: out of envelope — multi-hop
  gossip compounds amplification at each hop.

These bounds assume the per-(doc, peer) 2-second debounce in the propagation
task and the bounded 4096-entry LRU added in peat#909.

---

## Mitigation options

Four options were considered for the out-of-envelope cases.

### Option A — Accept and document

Keep the current rc.16+#909 behavior. Document the envelope in the developer
guide. Operators with deployments outside the envelope tune `max_connections`,
write rate, or topology to fit.

- Pro: zero code change, no new failure modes, ships today.
- Con: cliff for operators who don't know they're outside the envelope; the
  failure mode is "everything works but is slower" which is hard to diagnose.

### Option B — Topology-aware gossip (runtime detection)

The propagation task computes the fully-connected predicate per event: if every
peer it learns about via prior `Remote(src)` events is in
`connected_peers()`, treat as fully-connected and suppress the `Remote(_)`
fan-out (the source's neighbors already received the doc directly).

- Pro: zero operator burden; correct behavior emerges from observable state.
- Con: requires a per-doc adjacency map at the consumer; the predicate is
  stable only if topology is stable (churn breaks the inference); cross-host
  observability lag means an "almost fully connected" mesh may oscillate
  between the two regimes. New failure modes around the predicate's
  consistency.

### Option C — Opt-in / opt-out via config

Add a peat-mesh config flag (default-on for hub-and-spoke, default-off for
"dense" deployments). Operators choose at startup.

- Pro: explicit, predictable.
- Con: operator burden; the choice is wrong if the topology changes mid-life
  (e.g., a fully-connected mesh partitions and one side becomes a hub).
  Defeats the QUICKSTART "just works" property.

### Option D — Smarter suppression via peer-graph hints

Each node periodically gossips its `connected_peers()` list as an Automerge
doc. The propagation task consults the peer-graph: when alpha receives a doc
from bravo, alpha pushes to charlie only if charlie is NOT directly connected
to bravo.

- Pro: optimal — no redundant pushes in fully-connected, full gossip in
  hub-and-spoke, correct decay in partial mesh.
- Con: substantial implementation cost; peer-graph CRDT becomes a new
  hot-path doc; latency between graph change and propagation-task decision
  could leave gaps; new failure mode around peer-graph poisoning.

---

## Decision

**Option A (accept and document)** for the rc.16+#909 release line, with a
deferred path to **Option B (runtime detection)** for the next minor version.

Reasoning:

1. The current behavior is **correct** for every topology — convergence is
   guaranteed at the CRDT layer regardless of fan-out. The cost is bandwidth
   overhead, not data loss.
2. The motivating field report (peat#891) is hub-and-spoke; the envelope
   covers that case exactly.
3. The deployments currently in production are within the envelope (default
   `max_connections=7`, telemetry rate ≤ 1 Hz per node, ≥ 1 Mbps LAN links).
   No live deployment exceeds the envelope today.
4. Option B has the right shape but requires a per-doc adjacency map and
   careful handling of churn / observability lag. That's worth a separate
   slice; rushing it into the same release as the gossip fix would couple
   two non-trivial changes.

The applicability envelope is a contract: deployments outside the envelope must
either (a) wait for Option B, (b) provide their own topology-aware suppression
in a peat-mesh fork, or (c) accept the documented bandwidth overhead.

---

## Implementation status

- **rc.16 / peat-mesh#151**: shipped. Origin-tagged change broadcast lands the
  primitive Option B would build on (the consumer already knows source
  attribution per event).
- **peat#909**: shipped. Propagation task consumes the origin-tagged broadcast,
  filters echoes, debounces per (doc, peer). The per-(doc, peer) debounce + 4096-
  entry LRU bound the steady-state memory; the bandwidth bound is the one
  this ADR characterizes.
- **QUICKSTART**: covered. Scenario 2 / Scenario 4 hub-and-spoke promise is
  delivered; envelope is well within "few nodes, low write rate."
- **Developer Guide update**: landed in this PR. The envelope is now in
  `docs/guides/developer/DEVELOPER_GUIDE.md` §6.4.1 "Transitive gossip and
  topology envelope" as operator-facing guidance, with the full row matrix
  including BLE-class links and the three out-of-envelope mitigation
  options (reduce `max_connections`, reduce write rate, choose a partial
  topology). This was a peat#911 QA prerequisite — Option A's rationale
  in the §Decision section depends on operators being informed, and a
  pending guide update would have been a hard prerequisite for shipping
  to deployments outside the LAN envelope.
- **Option B implementation**: deferred. Tracked as a follow-up of peat#910.

---

## References

- peat#891 — field report (hub-and-spoke deadlock)
- peat#907 — tracking issue for the architectural fix
- peat#909 — consumer half + integration test + bounded LRU
- peat#910 — this ADR's parent ticket; "do not propose a fix here, escalate
  to ADR review" is the QA direction this document honors
- peat-mesh#151 — upstream PR (origin-tagged change broadcast, rc.16)
- ADR-007 — Automerge sync engine: ADR-007 selected Automerge as the
  CRDT backend but did not specify a propagation policy at the section
  level (the behavior lives in `peat-protocol/src/storage/automerge_backend.rs`
  Phase 6.5, not in any ADR-007 section). ADR-061 records an
  operational constraint on how that Automerge sync engine is run; it
  does not amend the backend-evaluation framework itself, hence the
  "Related" rather than "Amends" relationship.
- ADR-017 — P2P mesh management & discovery: this ADR amends §Layer 2
  "Mesh Topology Management" (the section actually named in ADR-017)
  to cover the four topology classes and their envelopes. The rest of
  ADR-017 (§Layer 1 Discovery Strategies, §Layer 3 Data Flow Control,
  Architecture, Implementation Design) remains in force.
