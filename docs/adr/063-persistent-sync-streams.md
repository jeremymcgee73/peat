# ADR-063: Persistent Multiplexed Sync Streams for Automerge Round-Throughput

**Status**: Proposed
**Date**: 2026-05-27
**Authors**: Kit Plummer
**Related**: ADR-007 (CRDT-Based Sync Engine — Automerge selection; this ADR records an operational/transport constraint on how that engine runs but does not amend the backend-evaluation framework itself, mirroring ADR-061's relationship to ADR-007), ADR-061 (gossip fan-out topology bounds — same Automerge-sync-engine-constraint shape), ADR-062 (iroh transport consolidation — Phase 2 owns the QUIC endpoint these streams sit on)
**Triggered by**: [peat-mesh#175](https://github.com/defenseunicorns/peat-mesh/issues/175) (sustained-write delivery ratio degrades under shaped-link sweep; lab-validated against rc.24); [peat#935](https://github.com/defenseunicorns/peat/issues/935) (peat-side tracking)

---

## Context

### The symptom

The 7n-dual-c2 telemetry-rate sweep against peat-mesh rc.24, lab scenario `probe-p3-no-loss.scn` (platform-3's two C2 links shaped at 256 kbps + 100 ms one-way delay, all other links at line rate), observed:

| Telemetry rate (per emitter × 2 emitters) | c2-alpha → platform-3 delivery |
|---|---|
| 1 Hz × 400 B   | 100.0% (205 / 205)        |
| 10 Hz × 400 B  | 90.8% (1 744 / 1 920)     |
| 25 Hz × 400 B  | 86.3% (3 989 / 4 622)     |

At 25 Hz × 400 B × 2 emitters the aggregate offered load is ≈ 20 KB/s — well under the link's 64 KB/s aggregate capacity. The plateau is not bandwidth-bound; it is sync-round-cadence-bound. The shaped link's ≈ 200 ms RTT caps a one-round-at-a-time sync coordinator at ≈ 5 sync rounds/sec/peer. At 50 emissions/sec total the coordinator cannot keep up: each round merges multiple writes, but drain rate from the c2-alpha → platform-3 backlog is bounded by `rounds/sec × writes_per_round`, and a 30 s post-emit drain window does not clear the long tail.

The adjacent attachment-convergence benchmark on the same shaped link degrades only ≈ 16% (37.0 s @ 0 Hz → 42.9 s @ 25 Hz background telemetry). The bulk-transfer path is more resilient than the small-write CRDT-sync path, which corroborates "sync-round overhead, not raw bandwidth" as the bottleneck.

### Two stacked gaps in the existing implementation

Tracing from the lab observation through the peat-mesh source reveals two architectural defects layered on top of each other. The persistent-channel surface was scaffolded under Issue #438 Phase 2 (`src/storage/sync_channel.rs` docstring) but never finished end-to-end:

**Gap 1 — `AutomergeBackend` never wires `SyncChannelManager`.**

`peat-mesh/src/sync/automerge_backend.rs:312-315` instantiates an `AutomergeSyncCoordinator` and never calls `set_channel_manager`. The coordinator field `channel_manager: Arc<RwLock<Option<Weak<SyncChannelManager>>>>` (`automerge_sync.rs:537`) stays `None` for the lifetime of the backend. The only call site in the repo that wires it is the standalone `bin/peat-mesh-node.rs:293-297` binary — which is not what consumers use.

Consequence: every consumer that uses the public `AutomergeBackend::new` (peat-protocol, peat-node, peat-sim) runs the legacy fallback at `automerge_sync.rs:1346`. That path executes for every sync message × every peer × every round:

1. `conn.open_bi().await` — open a fresh QUIC bi-stream
2. Write `[2 doc_key_len][N doc_key][1 msg_type][4 payload_len][payload]`
3. `send.finish()` — half-close the send side
4. `recv.stop(0)` — abort the receive side

That is per-message stream allocation and per-message teardown overhead. At 200 ms RTT the QUIC stream-open / stream-close round trip is the binding cost; the document bytes themselves are not.

**Gap 2 — `SyncChannelManager`'s wire format is asymmetric, and there is no persistent accept-side loop.**

Even if `set_channel_manager` is called, the channel cannot round-trip sustained traffic:

- `SyncChannel::send` (`sync_channel.rs:374-399`) writes `[2: doc_key_len=5][5: "batch"][1: marker=0x07][4: batch_len][batch_bytes]`. The literal `"batch"` is hard-coded as a multi-doc batch marker.
- `SyncChannel::receive_loop` (`sync_channel.rs:240-343`) reads `[1: marker][4: len][N: data]` directly — no doc_key prefix. On the first frame the receiver reads `0x00` (high byte of `doc_key_len=5`) as a marker, fails the `marker == SyncMessageType::SyncBatch (0x07)` check, logs `"Unexpected message type"`, and `continue`s. Subsequent iterations consume the `0x05`, then `'b'`, `'a'`, `'t'`, `'c'`, `'h'`, before finally landing on the real `0x07`. By the time it reaches the length field, the loop is reading payload bytes as length bytes and the stream is desynchronized for every subsequent frame.
- The accept side (`mesh_sync_transport.rs:209` → `automerge_sync.rs:2454` `handle_incoming_sync_stream`) uses `receive_sync_payload_from_stream` to read **one** payload and returns. There is no persistent accept-side loop. The matching framing exists (`[2: doc_key_len][N: doc_key][1: msg_type][4: payload_len][payload]`), but it processes a single message per accepted stream.

There are **zero tests** for `SyncChannelManager` outside `pub use` re-exports, which corroborates that the path was never validated end-to-end.

### Why this lives in an ADR

The two gaps together describe an architectural pattern, not a single bug. The decision the implementation needs to make is:

1. **What stream-allocation policy does the Automerge sync coordinator use?** One stream per peer (multiplexed across docs), one stream per (peer, doc), or one stream per message?
2. **What wire framing supports that policy?** Per-message-on-its-own-stream needs no framing (the stream IS the frame); persistent multiplexed needs explicit framing.
3. **What are the symmetric send/receive semantics on both connect and accept sides?**
4. **What is the round-throughput envelope the new policy delivers, expressed as a contract operators can rely on?**

This is the same shape as ADR-061: the CRDT layer's correctness is unchanged; the cost is an operational property of how sync is run. ADR-061 captured the topology envelope; this ADR captures the stream-throughput envelope.

---

## Decision

**Adopt persistent multiplexed sync streams as the sync coordinator's primary transport pattern, with per-message stream allocation retained only as a fallback during channel reconnection.** The persistent-stream surface scaffolded under Issue #438 Phase 2 is brought to completion: wire format is unified, accept-side loop is added, channel manager is wired into `AutomergeBackend::new`, and the round-throughput envelope is contracted.

The decision has four sub-parts.

### D1. One persistent bidirectional stream per peer, multiplexed across docs

For each `(local_endpoint, peer_endpoint)` pair the coordinator maintains exactly one bidirectional QUIC stream that carries sync messages for **all** docs the two peers share. Multiplexing keys are the per-message `doc_key` already present in the legacy framing. This collapses three nested costs to one:

- N peers × M docs × K rounds = N×M×K stream opens → N stream opens (amortized once at peer connect).
- Stream-open RTT no longer binds round throughput. Round throughput is bounded by `1 / RTT` per peer for *correlated* (request → response) rounds, and by the link's serialization bandwidth for *uncorrelated* (push) rounds.
- Concurrent rounds for different docs on the same peer pipeline naturally over the single stream's QUIC byte ordering.

Per-(peer, doc) is rejected because docs are dynamic — `peat-mesh` consumers create docs at runtime (telemetry emitters, hierarchical-aggregation rollups), and per-doc streams would re-introduce open-per-doc cost. Per-peer is the right granularity because peer membership is the stable layer.

One stream per peer also matches what `SyncChannelManager`'s scaffolding already assumes (`channels: HashMap<EndpointId, Arc<SyncChannel>>`).

### D2. Unified wire framing on the persistent stream

The persistent stream uses **the v2 framing already implemented by the legacy fallback path** (`automerge_sync.rs:1353-1377` send, `automerge_sync.rs:1412-1441` receive). One frame per logical sync message:

```text
[2 bytes BE: doc_key_len]
[doc_key_len bytes: doc_key (UTF-8)]
[1 byte: msg_type (DeltaSync | StateSnapshot | Tombstone | TombstoneBatch | SyncBatch | …)]
[4 bytes BE: payload_len]
[payload_len bytes: payload]
```

This is **the same byte layout the receiver already implements** in `receive_sync_payload_from_stream`. The framing decision is "match the legacy fallback exactly, then call the matching function in a loop." Concrete corrections required:

1. **`SyncChannel::send` keeps writing the v2 frame.** The existing implementation already does so; the `"batch"` doc_key for multi-doc batches is a valid framing choice — `"batch"` is just the doc_key string for the multi-doc-batch case, parsed identically to any other doc_key by the receiver.
2. **`SyncChannel::receive_loop` is rewritten to call `receive_sync_payload_from_stream` (or equivalent) in a loop.** The current "read marker / read length / read data" code is replaced wholesale. The cancellation token semantics are preserved.
3. **`handle_incoming_sync_stream` is rewritten to loop over `receive_sync_payload_from_stream` until the stream closes**, rather than returning after one payload. The single-payload variant is retained as a private helper for the legacy fallback path (or removed once the fallback is removed — see D4).

This is the minimum framing change: no new on-the-wire bytes, no protocol version bump, no migration phase. The receiver side already correctly parses every frame the sender already writes; the bug is that one of the two receiver implementations skips the prefix and the other returns after one frame.

### D3. Symmetric persistent loop on both connect and accept sides

The persistent stream is symmetric:

- **Connect side** (initiator) opens the bidirectional stream via `conn.open_bi()`. The receive half spawns a long-lived loop that processes frames until the stream closes or the channel is closed by `SyncChannelManager`.
- **Accept side** (responder) receives the bidirectional stream via `iroh::Connection::accept_bi()` (in `MeshSyncTransport`'s accept loop). The receive half spawns a long-lived loop **on the same stream** that processes frames until the stream closes. The send half is held in the per-peer channel state on the accept side as well, available for the responder to push its own deltas back without opening a separate stream.

Both sides use the same loop body; the only difference is who opened the stream. This means each `(local, peer)` pair has **one** stream that carries traffic in both directions — the responder's outgoing sync messages reuse the inbound stream's send half.

A consequence is that `SyncChannelManager` on the accept side must materialize a `SyncChannel` entry from an inbound stream, not just from an outbound `connect`. The manager's `channels` map becomes the single registry for both directions of any given peer.

### D4. Legacy fallback retained for reconnect window only

The legacy per-message-stream path at `automerge_sync.rs:1346` is **not** deleted. It is retained as the fallback when `SyncChannelManager::send_*` returns "channel reconnecting" — that is, the brief window between detecting a stream error and re-establishing the persistent stream. During reconnect the coordinator still makes progress (slowly), rather than queueing or dropping.

Once the persistent channel reaches `Connected` state again, the coordinator returns to the persistent path. The fallback's existence is invisible to consumers and to the bandwidth envelope; it is a degraded-mode safety valve.

This is deliberately the opposite of "feature flag for opt-in." The persistent path is always preferred; the legacy path is the failure mode.

### D5. `AutomergeBackend::new` wires `SyncChannelManager` unconditionally

`peat-mesh/src/sync/automerge_backend.rs:312-315` is amended to call `set_channel_manager` after coordinator construction, with the manager built from the same `MeshSyncTransport` + coordinator handle the `bin/peat-mesh-node.rs:293-297` site already uses. Consumers see no API change — `AutomergeBackend::new` returns the same type with the same surface; the wire-up is internal.

There is no consumer opt-out. Persistent streams are the default and only supported path. The `set_channel_manager` method becomes a private detail of `AutomergeBackend::new` once `bin/peat-mesh-node.rs` is also migrated to the same constructor path.

---

## Wire format (canonical, for the record)

The framing the persistent stream commits to:

```text
Frame:
  [2 bytes BE: doc_key_len]            // 0 < doc_key_len ≤ 65535
  [doc_key_len bytes: doc_key (UTF-8)] // logical multiplexing key
  [1 byte: msg_type]                   // SyncMessageType variant
  [4 bytes BE: payload_len]            // 0 ≤ payload_len ≤ 2^32 - 1
  [payload_len bytes: payload]         // msg_type-specific encoding
```

`msg_type` values (from `SyncMessageType` in `automerge_sync.rs:90`):

| Hex  | Variant           | Payload                              |
|------|-------------------|--------------------------------------|
| 0x00 | DeltaSync         | Automerge `SyncMessage::encode`      |
| 0x01 | StateSnapshot     | Automerge full-document bytes        |
| 0x02 | WindowedHistory   | windowed-history payload             |
| 0x04 | Tombstone         | ADR-034 tombstone record             |
| 0x05 | TombstoneBatch    | ADR-034 batched tombstones           |
| 0x06 | TombstoneAck      | ADR-034 tombstone acknowledgement    |
| 0x07 | SyncBatch         | `SyncBatch::encode` (multi-doc)      |

Frames are concatenated on the wire; there is no inter-frame delimiter beyond the length-prefix framing itself. A reader processes frames in a loop until the stream closes (graceful close or peer reset).

Endianness: all multi-byte integers are big-endian. This matches the legacy v2 format and is **not** subject to negotiation — there is no protocol version handshake to negotiate it.

The `doc_key` is a UTF-8 string in the range 1–65535 bytes. The reserved string `"batch"` is the multiplexing key for multi-doc batches (`SyncBatch`). All other `doc_key` values are application-defined document identifiers.

The wire format is identical to the legacy fallback's per-stream framing, so a frame written by either path is parseable by either path. This is the property that lets the legacy fallback co-exist as a reconnect-window degraded mode without introducing a versioning gate.

---

## Round-throughput envelope (contract)

With persistent multiplexed streams in place, the round-throughput envelope is:

- **One-way push throughput** (sender pushes deltas, no per-message acknowledgement at the sync layer): bounded by link serialization bandwidth, not RTT. On the 256 kbps shaped link of the UAT, the envelope is ≈ 32 KB/s per stream — orders of magnitude above the 20 KB/s offered load at 25 Hz × 2 emitters.
- **Round-trip sync throughput** (initiator's sync message followed by responder's sync message, both within the Automerge `generate_sync_message`/`receive_sync_message` cycle): bounded by `1 / RTT` per *correlated round* per peer. At 200 ms RTT that is 5 correlated rounds/sec/peer — unchanged from the legacy path, because the correlation is a property of the sync algorithm, not the stream pattern. **But:** uncorrelated rounds (the initiator pushing multiple frames before the responder replies) pipeline freely. In sustained-write workloads the dominant path is uncorrelated; the correlated path matters only at the initial convergence handshake per (peer, doc).
- **Per-doc steady-state cost**: amortizes to zero stream opens, one frame per delta, ≈ 13-byte framing overhead per frame.

The UAT thresholds in peat-mesh#175 are derived from this envelope:

| Rate          | Required delivery ratio | Envelope head-room                         |
|---------------|-------------------------|--------------------------------------------|
| 1 Hz          | ≥ 99.5%                 | 50× under serialization-bandwidth bound    |
| 10 Hz         | ≥ 99.0%                 | 5× under serialization-bandwidth bound     |
| 25 Hz         | ≥ 95.0%                 | 2× under serialization-bandwidth bound     |

And the attachment-convergence side-check (25 Hz background telemetry within 10% of the 0 Hz baseline) is the gate that prevents the persistent stream's accept-side loop from starving the iroh-blobs bulk-transfer path on the same connection.

Out-of-envelope cases the contract does **not** cover:

- More than one persistent stream per peer (e.g., dedicating a stream per priority class). Not in scope; would require a new ADR.
- Sync rounds across more than one peer in parallel from a single doc write. Already handled by the per-peer-stream design — fan-out across peers happens on separate streams concurrently.
- Persistent streams over BLE / serial / radio transports. peat-mesh's `SyncTransport` trait is iroh-shaped today (`conn.open_bi()` is QUIC-specific); BLE / serial would need their own transport-stream pattern. This ADR's envelope applies to the iroh-backed sync coordinator only — consistent with ADR-062's "iroh consolidation in peat-mesh."

---

## Consequences

**Positive:**

- The UAT thresholds (peat-mesh#175 §Pass criteria) become achievable. The sweep at 25 Hz × 2 emitters fits within the envelope by a 2× factor; the 10 Hz target by 5×; the 1 Hz target by 50×.
- The persistent-channel surface that has been scaffolded since Issue #438 Phase 2 (`sync_channel.rs` module docstring) reaches a finished state, removing dead-code maintenance cost.
- Future cross-transport extension (BLE persistent streams, serial framing) has a documented contract to extend from rather than a half-finished implementation to first understand.
- The legacy fallback's "stream-per-message" cost is paid only during reconnect windows, not in steady state. Memory pressure from many short-lived QUIC streams (relevant for peat#873-class symptoms on Android) drops correspondingly.

**Negative / cost:**

- The fix is non-trivial: rewriting `SyncChannel::receive_loop`, adding the accept-side loop to `MeshSyncTransport`/`handle_incoming_sync_stream`, wiring `set_channel_manager` into `AutomergeBackend::new`, plus unit + integration tests for the persistent-channel happy path and the reconnect path. Investigation comment on peat-mesh#175 suggests splitting into two PRs: one for channel correctness + tests, one for the wire-up. That ordering is correct.
- Reconnect logic must be hardened. The current `SyncChannel::reconnect` (`sync_channel.rs:353-365`) is triggered by `send` observing `ChannelState::Reconnecting`. With the accept-side loop in place, the responder must also detect peer stream loss and either spawn a new accept loop on the next inbound `accept_bi()` or surface the loss to `SyncChannelManager` for cleanup. This is a new failure-mode surface.
- Per-peer state grows: one persistent stream + a send mutex + a receive task handle per connected peer. At the default `max_connections=7` (ADR-061 §Topology classification) that is ≈ 7 stream handles per node — bounded and small, but a steady allocation rather than a transient one.
- The "tests are missing for `SyncChannelManager`" gap noted in the investigation must be closed as part of this work. The acceptance criteria for the implementation PR (peat-mesh#175) include unit + integration test coverage for: persistent-channel happy path, reconnect-on-error path, accept-side loop processing multiple frames per stream, and the round-trip throughput threshold under the UAT scenario.
- One bandwidth-amplification interaction with ADR-061: under transitive gossip (relay-on-remote-apply), the persistent stream pattern means the relay's outbound fan-out reuses already-open streams. That is purely beneficial (fewer stream opens) and does not change ADR-061's topology envelope — the *number* of sync rounds per write event is unchanged; the cost per round is lower. ADR-061 §Bandwidth bound per topology continues to hold; the absolute byte cost moves down.

**Neutral / unchanged:**

- CRDT-layer correctness: untouched. `generate_sync_message` / `receive_sync_message` semantics and Automerge convergence guarantees are independent of the stream pattern.
- Public API surface of `AutomergeBackend`: unchanged. The wire-up of `SyncChannelManager` is internal.
- FIPS crypto posture (per ADR-060 §5 and `peat/CLAUDE.md` § "Hard rule: FIPS-approved cryptographic primitives only"): no crypto choice is made by this ADR. The streams run on whatever rustls/aws-lc-rs provider the iroh layer is configured with, identical to the legacy path.
- ADR-007's CRDT-backend selection: unchanged. Automerge remains the selected backend; this ADR documents how the transport beneath it runs.
- ADR-062's iroh consolidation: unchanged. The persistent streams sit on the peat-mesh-owned iroh `Endpoint` per ADR-062 Phase 2. No additional surface needed from peat-mesh's `network::iroh_transport` module.

---

## Implementation status

- **Diagnosis**: complete. Two-gap analysis captured in peat-mesh#175 comment thread and verified against `peat-mesh/src/sync/automerge_backend.rs`, `src/storage/automerge_sync.rs`, `src/storage/sync_channel.rs`, `src/storage/mesh_sync_transport.rs`.
- **ADR**: this document (peat#935).
- **peat-mesh fix**: open against peat-mesh#175. Recommended PR split per the investigation comment:
  1. PR A — `SyncChannelManager` correctness: rewrite `receive_loop`, add accept-side persistent loop, fix wire-format symmetry, add unit + integration tests. No behavior change for current consumers (since none wire the manager today).
  2. PR B — `AutomergeBackend::new` wires `SyncChannelManager`. Behavior change: every consumer gets the persistent stream. UAT (`sweep-telemetry-rate.sh`) runs against this PR; pass criteria are the peat-mesh#175 thresholds (1 Hz ≥ 99.5%, 10 Hz ≥ 99.0%, 25 Hz ≥ 95.0%, attachment convergence within 10% of 0 Hz baseline).
- **peat-protocol consumer-side**: no change required. peat-protocol consumes `AutomergeBackend` via the same path it always has; the persistent-stream upgrade is internal to peat-mesh.
- **Cooldown knob** (`flow_control.rs:70`, 100 ms per-(peer, doc) cooldown): left unchanged. The investigation flagged this as adjacent; it does not bind on live emission streams (each emission uses a fresh `doc_id`) and only matters on backlog catch-up re-sync. If post-fix numbers still plateau, the cooldown is the next knob; otherwise out of scope.

---

## Alternatives considered

### Alt-A — Delete `SyncChannelManager` entirely, optimize the legacy path

Drop the half-finished persistent-channel scaffold. Optimize the legacy "open stream per message" path with techniques like batched `open_bi` (open N streams in advance and queue messages onto them) or HTTP/2-style stream prioritization.

Rejected: the legacy path is fundamentally bounded by stream-open cost on every message. Optimizing around it postpones the round-throughput problem rather than fixing it. The scaffold is already most of the way there; finishing it is less work than re-architecting around the legacy path.

### Alt-B — Per-(peer, doc) persistent stream

One stream per `(peer, doc)` pair instead of per `peer`. Removes the multiplexing key (`doc_key`) from the wire format entirely (the stream identity is the multiplexing key).

Rejected: doc count is dynamic and unbounded in the workloads peat is built for (telemetry emitters create docs at write rate). Per-doc streams re-introduce per-doc stream-open cost, which is the cost the persistent design is supposed to amortize away. Also doesn't compose with `SyncBatch` (multi-doc batched syncs).

### Alt-C — In-flight sync-round pipelining only (no persistent stream)

Keep the legacy per-message-stream path, but allow N sync rounds to be in flight per peer simultaneously. This was hypothesis #1 in peat-mesh#175's "optimization candidates" section.

Rejected: pipelining helps the response-pending case but does nothing for the dominant cost (QUIC stream open + close per message). At 200 ms RTT, a single stream open / close cycle is ~ 100 ms of wall-clock — pipelining 4 rounds still saturates the stream-allocation path, not the bandwidth. Also adds complexity (in-flight-window state, response correlation) without solving the binding cost.

### Alt-D — Move to a completely different transport (HTTP/2 over QUIC, custom framing)

Adopt HTTP/2-over-QUIC or a custom framed sub-protocol with proper stream multiplexing semantics.

Rejected: iroh already provides QUIC stream multiplexing. The persistent stream design in this ADR uses iroh's bidirectional QUIC stream as the primitive — it *is* a multiplexed sub-protocol on QUIC, just one we own. HTTP/2 adds a header table and request/response semantics that don't fit a CRDT push protocol.

---

## References

- peat-mesh#175 — sustained-write delivery plateau (this ADR's parent ticket).
- peat#935 — peat-side tracking for the ADR.
- peat-mesh source paths cited above:
  - `src/sync/automerge_backend.rs:300-339` — `AutomergeBackend::new`, the missing-wire-up site (D5).
  - `src/storage/automerge_sync.rs:537, 583, 615, 626, 693, 1126, 1312-1402, 1404-1450, 2454-2490` — coordinator, `set_channel_manager`, legacy-fallback path, v2 receive function, single-payload accept site.
  - `src/storage/sync_channel.rs:1-78, 116-200, 240-343, 351-410` — module docstring, `SyncChannel::connect`, `receive_loop` (the asymmetric-framing bug), `send` (the canonical framing).
  - `src/storage/mesh_sync_transport.rs:198-237` — accept loop calling the single-payload `handle_incoming_sync_stream`.
  - `src/bin/peat-mesh-node.rs:293-297` — the one site that wires `set_channel_manager`.
- peat-mesh#137 — `fetch_blob` peer iteration penalty. Different code path, similar "sync-tier overhead manifests under load" pattern; not a dependency.
- peat#873 — connection-recycling memory pressure on Android. Persistent streams cut the rate at which stream handles are allocated and reclaimed, which makes #873-class symptoms less likely but does not replace the recycler design.
- ADR-007 — CRDT-Based Sync Engine. Selects Automerge; this ADR records a transport-pattern constraint on how the Automerge sync engine runs but does not amend the backend-evaluation framework. Mirrors ADR-061's relationship to ADR-007.
- ADR-061 — Gossip Fan-Out Topology Bounds. Operational envelope on the same Automerge sync engine; this ADR's persistent-stream pattern reduces the per-round byte cost but does not change ADR-061's topology classes or N-vs-write-rate envelope.
- ADR-062 — Iroh Transport Consolidation. The persistent streams sit on the iroh `Endpoint` peat-mesh now owns post-Phase 2. No new public surface from `peat_mesh::network::iroh_transport` is required.
- `sweep-telemetry-rate.sh` — UAT script in [peat-sim/experiments/7n-dual-c2](https://github.com/defenseunicorns/peat-sim/tree/main/experiments/7n-dual-c2). The pass criteria in this ADR's §Round-throughput envelope are this script's thresholds.
