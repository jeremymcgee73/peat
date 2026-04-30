# ADR-059: Cross-Transport Document Bridging via TransportManager

**Status**: Proposed
**Date**: 2026-04-29
**Authors**: Kit Plummer
**Organization**: Defense Unicorns
**Relates To**: ADR-029 (TAK Transport Adapter), ADR-032 (Pluggable Transport Abstraction), ADR-038 (Protocol Format Transformation Primitives), ADR-041 (Multi-Transport Embedded Integration), ADR-019 (QoS and Data Prioritization), ADR-042 (Direct UDP Bypass Pathway), ADR-046 (Targeted Message Delivery)

---

## Context

Peat is a multi-transport protocol. Today three concrete transports exist or are imminent: iroh (QUIC + relay), peat-btle (BLE wire format, ADR-041), and TAK (ADR-029). Future transports include LoRa (ADR-052), SBD/satellite (ADR-051), and mavlink (ADR-058).

Each of these transports already has its own translator: `BleTranslator` for BLE (ADR-041), the TAK adapter for CoT (ADR-029). Each one was built one-off, with bespoke wiring between `peat_mesh::Node`'s document store and the transport's wire format. The `BleGateway` struct in `peat-protocol/src/sync/ble_gateway.rs` is the most explicit instance — it owns a `Node` and a `BleTranslator` and exposes `ingest_*` (BLE → iroh) and `observe_*` (iroh → BLE) methods.

**The problem:** as more transports come online, this one-off pattern produces N copies of the observer scaffolding, the loop-prevention logic, and the FFI bridge — diverging over time and forcing every gateway-capable node to import N transport-specific gateway types.

**The need:** a node running multiple transports simultaneously must observe document changes once, fan them out to every transport whose translator can encode them, and prevent echo loops back to the originating transport. Operators must be able to scope individual documents (or whole collections, or specific subscriptions) to a subset of transports — for QoS, security, or bandwidth reasons.

## Decision

Lift the per-transport translator pattern into a generic `Translator` trait owned by `peat-mesh`. Extend `TransportManager` (ADR-032) from a transport selector into a fan-out orchestrator that observes `Node` collections once and routes events through every registered translator.

### `Translator` trait (peat-mesh)

```rust
#[async_trait]
pub trait Translator: Send + Sync {
    /// Stable identifier for this transport, used for origin tagging
    /// and allowed_transports matching. e.g. "ble", "iroh", "tak".
    fn transport_id(&self) -> &str;

    /// Encode a document for outbound transmission on this transport.
    /// Return None if this transport cannot carry the document
    /// (size limits, missing fields, format affinity per ADR-038).
    async fn encode_outbound(
        &self,
        doc: &Document,
        ctx: &TranslationContext,
    ) -> Option<Vec<u8>>;

    /// Decode an inbound wire-format payload into a document.
    /// `Ok(None)` means the payload is well-formed but not a document
    /// this transport carries (e.g. a control frame, a heartbeat) —
    /// normal traffic, no diagnostic. `Err(_)` means the bytes were
    /// malformed for this codec — bump a metric, log a diagnostic;
    /// production fleets need this signal to detect wire-format drift.
    ///
    /// The error type is `anyhow::Error` deliberately. Every codec
    /// owns its own error enum (CoT for TAK, decode errors for BLE,
    /// etc.); a closed enum here would require peat-mesh to import
    /// every codec's error types, inverting the layering the ADR
    /// exists to clean up. Codecs map their errors with `.context()`
    /// for telemetry; the orchestrator only needs "did this fail" +
    /// a stringified chain for logging — opaque error suffices.
    /// `anyhow` is already the peat-mesh `Result` convention.
    async fn decode_inbound(
        &self,
        bytes: &[u8],
        ctx: &TranslationContext,
    ) -> anyhow::Result<Option<Document>>;
}

/// Per-call context the orchestrator hands to translators. Carries the
/// information that today's `BleTranslator` already takes as positional
/// arguments (callsign, cell_id, peripheral_id) and that ADR-029's
/// TAK adapter already plumbs through its config (identity, callsign).
/// Adding fields here is a non-breaking change; translators ignore
/// fields they don't need.
pub struct TranslationContext {
    /// Local node's callsign (operator-visible name).
    pub local_callsign: Option<String>,
    /// Local node's identifier on this transport (peripheral_id for BLE,
    /// EndpointId hex for iroh, callsign for TAK, etc.).
    pub local_transport_id: Option<String>,
    /// Cell / formation the document belongs to, if scoped.
    pub cell_id: Option<String>,
    /// For inbound: the peer that delivered the bytes. For outbound:
    /// `None` if broadcast, or the targeted peer (composes with ADR-046).
    pub peer: Option<String>,
}
```

The trait is async because at least one in-tree codec (the TAK adapter, ADR-029) is already `#[async_trait]`-shaped, and other planned transports (LoRa with bandwidth gating per ADR-052, SBD with satellite-window queueing per ADR-051) will need async hooks. `BleTranslator` is sync today, but its methods slot trivially into `async fn` (`async fn x(&self, ..) { sync_body }`) with no runtime cost on a Tokio executor.

`peat-protocol`'s existing `BleTranslator`, the TAK adapter, and any future transport's codec become `Translator` implementations. `peat-protocol` remains the home for the codecs themselves; `peat-mesh` owns the trait and the orchestrator.

**TAK-shaped sketch (validates the signature against ADR-029):**

```rust
pub struct TakTranslator {
    cot_codec: CotCodec,
    callsign_cache: Arc<RwLock<HashMap<String, String>>>, // node_id → callsign
    config: TakConfig, // identity, server URL, etc.
}

#[async_trait]
impl Translator for TakTranslator {
    fn transport_id(&self) -> &str { "tak" }

    async fn encode_outbound(
        &self,
        doc: &Document,
        ctx: &TranslationContext,
    ) -> Option<Vec<u8>> {
        // encode_outbound's `None` cases are all "decline": size
        // limits, missing fields, format affinity (per ADR-038). It
        // is *not* a silent error sink. When `doc_to_cot` returns Err
        // — i.e. a real codec failure rather than a structural
        // decline — log + bump a per-codec encode-error metric
        // before returning None. The trait's signature can't carry
        // the error directly, so the codec must surface it as
        // telemetry; otherwise wire-format drift on the encode side
        // is invisible to operators.
        match self.cot_codec
            .doc_to_cot(doc, ctx.local_callsign.as_deref())
            .await
        {
            Ok(cot) => Some(cot.into_xml_bytes()),
            Err(e) => {
                tracing::warn!(
                    error = %anyhow::Error::from(e),
                    transport = "tak",
                    "encode_outbound failed; this is a real codec error, not a decline",
                );
                metrics::counter!("translator.encode_error", "transport" => "tak").increment(1);
                None
            }
        }
    }

    async fn decode_inbound(
        &self,
        bytes: &[u8],
        ctx: &TranslationContext,
    ) -> anyhow::Result<Option<Document>> {
        // Malformed CoT bytes surface as Err — production fleets need
        // this signal to detect wire-format drift. Don't swallow with
        // `.ok()?` — that would defeat the contract the trait makes.
        // `.context()` carries codec-specific framing into the
        // anyhow chain for telemetry without requiring peat-mesh to
        // know about CotError.
        let cot = CotEvent::parse(bytes).context("TAK: parse CoT XML")?;

        // Cache callsign opportunistically; cache failure is not a
        // wire-format-drift signal, just log and continue.
        if let Some(cs) = cot.callsign() {
            self.callsign_cache.write().await.insert(cot.uid().to_string(), cs.into());
        }

        // CoT events that don't map to a doc this transport carries
        // (e.g. control/heartbeat events) return Ok(None) — normal
        // traffic, no diagnostic. Only structural decode failures
        // bubble up via Err with codec context.
        self.cot_codec
            .cot_to_doc(&cot, ctx.peer.as_deref())
            .await
            .context("TAK: map CoT to Document")
    }
}
```

The interior mutability on `callsign_cache` (an `Arc<RwLock<...>>`) is the per-translator state the trait deliberately doesn't try to abstract — translators own their state shape; the trait only fixes the encode/decode contract. This matches the way ADR-029 describes TAK adapter internals today.

If this signature does not survive contact with the TAK migration in Slice 1.5, the ADR commits to revising it before Slice 1.5 ships rather than locking Slice 1's BLE refactor into an unstable shape. The pre-Slice-1.5 review gate is captured in the Implementation section.

### iroh's role: substrate, not Translator

Before the orchestration spec: iroh occupies a different layer than other transports in this design. iroh **is the CRDT sync substrate** — it ships Automerge ops between `Node` doc stores. Other transports (BLE, TAK, LoRa, SBD) translate documents to/from foreign wire formats and bridge into the doc store via `Node::publish` and observe via `Node::observe`. iroh does neither of those — it operates at the Automerge layer directly. **iroh is therefore not in the `Translator` set**; it has no `encode_outbound` / `decode_inbound` to implement.

This carve-out keeps the orchestration coherent: every translator's role is "this foreign wire format ↔ Document"; iroh has no foreign wire format because Automerge is the encoding. Sync-delivered docs (i.e., docs that arrived via iroh's CRDT merge from a remote peer) appear on the local `Node` as new state with no local-transport origin — their `ChangeEvent.origin` is `None`. That is correct: from this node's perspective, no local transport delivered them, so all eligible local transports should fan out.

### `TransportManager` fan-out orchestration

The orchestration is **observer-driven**: `TransportManager` subscribes to every collection on `Node` via `Node::observe`, and every event the observer emits drives a non-blocking fan-out into per-transport bounded queues. There is no separate "publish-driven" fan-out path. This collapses the model to one code path that handles three cases uniformly: locally-published docs, transport-ingested docs, and sync-delivered docs.

**Origin-on-event** replaces an in-memory map. `Node`'s publish API is extended:

```rust
impl Node {
    /// Existing convenience; equivalent to publish_with_origin(.., None).
    pub async fn publish(
        &self,
        collection: &str,
        doc: Document,
    ) -> anyhow::Result<DocumentId>;

    /// Publish with a per-call origin hint that travels with the
    /// resulting ChangeEvent. The origin is *not* stored on the
    /// document and *not* serialized for CRDT sync — it is metadata
    /// attached to the ChangeEvent stream on this node only.
    pub async fn publish_with_origin(
        &self,
        collection: &str,
        doc: Document,
        origin: Option<String>,
    ) -> anyhow::Result<DocumentId>;
}

pub struct ChangeEvent {
    /// Existing variant fields (Updated / Initial / Removed).
    pub kind: ChangeEventKind,
    /// Per-event origin hint. Set to the `origin` argument of the
    /// publish_with_origin call that produced the event when the
    /// event was triggered by a local publish; `None` for events
    /// triggered by Automerge sync from a remote peer or by
    /// Node::publish without an origin hint.
    pub origin: Option<String>,
}
```

`origin` is metadata on the event, not on the document. It rides the local `ChangeEvent` stream only — Automerge sync to remote peers does not see it. This forecloses the multi-hop replication failure flagged earlier: gateway A's `publish_with_origin(.., Some("ble"))` produces a local event with `origin=Some("ble")` (A's BLE fan-out is skipped), but the doc travels to gateway B via Automerge with no origin trace. B's local observer fires with `origin=None` (sync, not a local publish_with_origin call) and broadcasts to all of B's eligible transports — including B's BLE peers, who are a different physical segment from A's.

```text
TransportManager state:
    translators: HashMap<TransportId, Arc<dyn Translator>>
    outbound_queues: HashMap<TransportId, mpsc::Sender<FanoutItem>>
    qos_drop_policy: Arc<dyn QosDropPolicy>   // ADR-019

initialization (once per Node):
    for each collection registered with the doc registry:
        let stream = node.observe(collection, &Query::All).await?;
        spawn observer task:
            while let Some(event) = stream.recv().await:
                self.fanout(&event);

inbound transport bridge (one per non-iroh transport):
    on bytes from transport T:
        match T.translator.decode_inbound(bytes, ctx).await {
            Ok(Some(doc)) =>
                node.publish_with_origin(collection, doc, Some(T.transport_id())).await;
            Ok(None) => { /* control frame; ignored */ }
            Err(e)   => { /* log + bump per-transport parse-error metric */ }
        }

fanout(event):
    let doc = event.document;     // Updated / Initial only in Slice 1.
                                  // Removed is deferred to Slice 2 — see
                                  // "Delete propagation" below.
    for each (transport_id, sender) in state.outbound_queues:
        if event.origin == Some(transport_id) { continue }              // single-node loop prevention
        if let Some(allowed) = &doc.allowed_transports {
            if !allowed.contains(&transport_id) { continue }            // operator scoping
        }
        match sender.try_send(FanoutItem { doc: doc.clone(), ctx: ctx.clone() }) {
            Ok(()) => {}
            Err(TrySendError::Full(item)) =>
                state.qos_drop_policy.handle_full(&transport_id, item),
            Err(TrySendError::Closed(_)) => {
                // Bump a per-transport metric so a drain-task panic is
                // distinguishable from intentional unregister in
                // production. On first Closed for a transport_id,
                // synchronously remove it from outbound_queues so
                // subsequent events surface "no such transport" rather
                // than repeatedly hitting Closed silently.
                metrics::counter!(
                    "transport.fanout_dropped_closed",
                    "transport" => transport_id.clone()
                ).increment(1);
                state.outbound_queues.remove(&transport_id);
                tracing::warn!(
                    transport = %transport_id,
                    "fan-out channel closed unexpectedly; \
                     transport removed from active set"
                );
            }
        }

per-transport drain task (one per registered transport,
                          started at register_translator time):
    while let Some(item) = receiver.recv().await:
        let translator = state.translators[&transport_id];
        match translator.encode_outbound(&item.doc, &item.ctx).await {
            Some(bytes) => {
                if let Err(e) = state.transports[&transport_id]
                    .send_outbound(bytes).await
                {
                    // log + per-transport send-failure metric;
                    // transport's own retry/backoff applies
                }
            }
            None => {
                // codec declined (size limits, missing fields,
                // format affinity per ADR-038). Bump a
                // per-(transport, decline-reason) metric so
                // operators can see when scoping starts dropping
                // traffic that ought to be carried.
            }
        }
```

Key properties:

- **One fan-out path, three cases handled uniformly.** Local publish with origin → event carries that origin → originator's transport skipped, others fan out. Local publish without origin → event has `origin=None` → all eligible transports fan out. Sync-delivered doc → event has `origin=None` → all eligible transports fan out. There is no special case for "this doc came in from a transport vs Automerge sync."
- **The origin lifetime problem is resolved at the Node API level.** Round-4's publish-call-lifecycle scopeguard was unsound under async observer firing. With `origin` carried on the event itself, there is no time-bounded window: the event's origin is set when the event is constructed and read when the observer dispatches; both happen on whichever task handles the event, and the field's lifetime is the event's lifetime. No HashMap, no scopeguard, no TTL.
- **The fan-out loop is non-blocking.** `try_send` per channel completes in microseconds; a slow `encode_outbound` on the TAK or LoRa channel does not delay `send_outbound` on BLE or iroh. Each transport's encode and send happen on its own task, isolated.
- **The QoS-aware drop policy lives on the channel boundary.** When `try_send` returns `Full`, the policy (per ADR-019) decides whether to evict the lowest-priority queued item, drop the new arrival, or coalesce. Whichever path it takes, the observer task never blocks. This is the load-bearing path the QoS mitigation has to be on — encoding before checking queue depth would defeat it.
- **Ordering within a transport is preserved.** The mpsc channel is FIFO; the drain task encodes and sends in the order items were pushed. Cross-transport ordering is unspecified (and necessarily so — transports run independently).

### Per-transport channel sizing

Per-transport channel buffer depth is **part of the QoS calibration, not an implementation detail**. A single default across all transports would be wrong: BLE/LoRa/SBD have radically different throughput, MTU, and burst tolerance from iroh QUIC. Using one number would either over-buffer cheap transports (delaying QoS preemption past ADR-019's intent — high-priority traffic queues behind low-priority backlog instead of preempting it) or under-buffer fast ones (spurious drops on bursts the transport could absorb).

Channel depth is configurable per transport at `register_translator` time:

```rust
impl TransportManager {
    pub fn register_translator(
        &self,
        translator: Arc<dyn Translator>,
        config: TranslatorRegistrationConfig,
    ) -> anyhow::Result<()>;
}

pub struct TranslatorRegistrationConfig {
    /// mpsc channel depth. Sized per transport-family characteristics
    /// (see defaults below); operators may override.
    pub outbound_buffer_depth: usize,
    // … other registration-time parameters as needed (QoS policy
    // override, drain-task priority, etc.)
}
```

**Default depths and rationale** for the four in-scope transports:

| Transport | Default depth | Rationale |
|---|---|---|
| `ble`     | 32  | Per-link MTU ~150 B + ~5–10 frames/sec sustained throughput; 32 absorbs ~3 s of burst before QoS preemption kicks in. Larger depths would let stale low-priority traffic delay emergency frames. |
| `lora`    | 16  | Duty-cycle limited (per ADR-052); a deeper queue is meaningless because the radio can't drain it. QoS preemption needs to fire early to keep the queue's contents fresh. |
| `sbd`     | 8   | Satellite windows are minutes apart (per ADR-051); only the highest-priority items survive each window. Shallow depth forces operators/QoS to pick winners up front rather than queue indefinitely. |
| `tak`     | 256 | TCP-backed; high throughput, low per-message cost. Depth here protects against TAK-server hiccups without losing traffic. |

Defaults are documented in the `TranslatorRegistrationConfig` impl, not hard-coded inside `TransportManager`. Operators can override per deployment — a tactical edge node bandwidth-throttled for OPSEC will want lower depths than a cloud bridge.

### Concurrent ingest of the same logical doc

If the same Automerge document arrives via two transports simultaneously on the same node (e.g. a peer reachable on both BLE and iroh sends the same chat through both), each transport's bridge calls `publish_with_origin` independently. Automerge merges both into the doc store. The local observer fires twice — once per publish call, with each event carrying its respective origin. First fan-out skips BLE (origin=`"ble"`); second fan-out skips iroh (origin=`"iroh"`). The doc is sent to TAK/LoRa once each. No echo to the originator on either transport — both events naturally exclude their respective origin without any cross-event coordination.

Duplicate-encode cost on the receiver side varies by transport:

- **Automerge-aware destinations (other peat nodes over iroh)** — duplicate ops merge idempotently in the destination's CRDT; cost is bounded.
- **TAK clients** — CoT semantics replace state by UID; duplicates are idempotent at the receiver but the bytes were already paid for on the wire.
- **Bandwidth-constrained transports (LoRa per ADR-052, SBD per ADR-051)** — duplicate-encode cost is operationally significant against duty-cycle / satellite-window budgets. **For these transports, per-transport channel coalescing is required, not optional**: `TranslatorRegistrationConfig` defaults enable coalescing for `lora` and `sbd` registrations (drain task collapses successive `FanoutItem`s for the same `doc_id` before encoding). Operators on bandwidth-budget-tight deployments may enable coalescing for `ble` as well; the default leaves it off for `ble` because BLE's local-segment latency makes coalescing visible as added jitter.

### Per-document transport scoping

One new field on the document:

- `allowed_transports: Option<Vec<String>>` — `None` (default) means any transport whose translator can encode the doc may carry it; `Some(["iroh"])` restricts to iroh, gateways skip even if their translator returns `Some(bytes)`.

(The fan-out orchestration section above describes how `Node::publish_with_origin` and `ChangeEvent.origin` carry the loop-prevention origin tag on the local event stream. That tag is **not** a document field — see "Schema impact" below for the rationale.)

**Validation rules** (resolves QA-flagged ambiguity in the original draft):

- `Some(vec![])` is **rejected at publish time** as a malformed value. There is no "block all transports" sentinel — a document not intended to leave the local node should not be published into a synced collection at all. This eliminates the silent-block footgun where an empty list looks like default behavior.
- Every entry in `allowed_transports` is validated against the set of `Translator::transport_id()` values registered with `TransportManager` at publish time. Unknown IDs are **rejected** (not silently dropped, not warned-and-included). This prevents typos like `"ireoh"` from collapsing into "block all transports" — the publish call returns an error and the operator sees the failure immediately.

**Per-collection registry defaults validate at *both* registration time and publish time.** Collection registration with a `default_allowed_transports` containing an unknown ID fails at registration — the operator sees the typo immediately. But because collection registration and transport registration are independent lifecycle events (a transport may be unregistered later, or a node may register a collection whose default refers to a transport that node doesn't run), publish-time validation re-runs against the currently-registered transport set. A collection-defaulted ID that is no longer registered at publish time produces an explicit publish error, never a silent fall-through. Operators choose whether to handle the error by retrying without the missing transport in the allow-list, or by treating it as a configuration drift to remediate.

### Per-collection defaults

Collections registered in the document registry (per ADR-021) accept an optional `default_allowed_transports` setting that gets stamped onto every document published into the collection unless the publisher overrides it. Operators can declare "the `commands` collection is iroh-only" once at registration time.

### Per-subscription filtering

Extend the existing `Query` enum with a new variant:

```rust
Query::AllowedTransport(String)
```

Matches documents where `allowed_transports.is_none()` or `allowed_transports.unwrap().contains(t)`. Subscribers can ask "give me chat docs that BLE peers will see" without inspecting the field themselves.

### Invariants

A transport without a `Translator` implementation may participate as a transport for its own peer set, but **cannot bridge** documents to or from any other transport. Such transports are isolated. This is a forcing function: every transport added to the system must declare its codec to the protocol layer, or accept island-mode.

**Transport ID uniqueness.** `TransportManager::register_translator` rejects any translator whose `Translator::transport_id()` collides with an already-registered translator. Without this, two registrations claiming `"ble"` would produce ambiguous origin tagging, ambiguous `allowed_transports` matching, and ambiguous wire-format identity — all correctness bugs, not stylistic ones. Registration returns an explicit error; the caller decides whether to retry under a different ID or fail node startup.

**Automerge no-op-merge suppression.** The multi-hop loop-prevention story (gateway A publishes via BLE with `origin=Some("ble")` → Automerge syncs to gateway B over iroh → B fans out to its transports) depends on iroh's CRDT round-trip back to A *not* firing a `ChangeEvent` on A's idempotent merge of state A already has. If `Node` were to emit a `ChangeEvent` on no-op merges, A would re-observe its own doc with `event.origin = None` (no local publish_with_origin produced it) and re-fan-out to BLE — exactly the loop the per-event origin design exists to prevent. The orchestrator therefore relies on `Node`'s `ChangeEvent` stream suppressing no-op merges. peat-mesh ADR-0007 (Consumer Interface Adapters) and the existing `AutomergeIrohBackend` change-detection path in `peat-protocol/src/sync/automerge.rs` already implement this — events fire on actual state change, not on every sync round-trip. Slice 1 implementation **must include a regression test fixture exercising the A→B→A round-trip** to lock the contract; if a future backend change breaks no-op suppression, the orchestrator's fallback is a content-hash dedup at the fan-out boundary (compare event's resulting doc digest against a small per-`doc_id` recently-emitted set).

### Schema impact

Only **one** new document field is introduced. Origin tagging for loop prevention rides on `ChangeEvent.origin` (a per-event API addition, not a document field) and `Node::publish_with_origin` (a publish-API addition); see fan-out orchestration above for the contract. Neither is in cap-schema or on the wire.

Per ADR-012 (Schema Definition / Protocol Extensibility):

- `allowed_transports: Option<Vec<String>>` is a new top-level optional field. Existing documents lacking it are interpreted as `None` (broadcast to any capable transport), preserving today's behavior. No existing document needs migration.
- **cap-schema versioning:** minor bump. Backward compatible — older readers tolerating unknown fields continue to function, and the omit-when-`None` encoding rule (below) means existing docs stay byte-identical on the wire.

**Wire-format codec contract** (matters for BLE per ADR-041, LoRa per ADR-052, SBD per ADR-051 — every byte counts against the MTU):

- A codec **MAY elide** the `allowed_transports` field entirely from the wire payload when its value is `None`. The receiving translator reconstructs `None` for absent fields. This guarantees no wire-size cost for documents that don't use the new field.
- **Transport IDs on the wire are stable strings** (`"ble"`, `"iroh"`, `"tak"`, etc. — the same strings returned by `Translator::transport_id()`). They must round-trip identically across nodes, so per-node enum indices are explicitly **not** allowed for cross-node payloads — registration order differs from node to node and a per-node index decodes to a different transport ID on each receiver, which would defeat the entire scoping mechanism. Codecs that need wire-size optimization may use **session-scoped string interning** with the symbol table negotiated up front per BLE/LoRa link, but the canonical form is the registered string.
- When `allowed_transports` is present, a codec **MAY** decline to encode the document at all if its own transport ID is not in the list. The codec returns `None` from `encode_outbound`, and `TransportManager` skips the send. This is the wire-level expression of the publish-time validation: scoping is honored equally by the orchestrator's allow-list check *and* by the codec's encode contract.

## Consequences

### Positive

- **One observer scaffolding instead of N.** New transports add a `Translator` impl + register with `TransportManager`. No new fan-out code per transport.
- **Loop prevention is uniform.** A per-event `origin: Option<String>` field on `ChangeEvent` — set by `Node::publish_with_origin` and never serialized to the wire — replaces the per-doc `ble_origin: true` flag introduced by `BleGateway` (PR #802; see Supersedes). Because the origin rides the local event stream only and is dropped at every node boundary, it stays correct across multi-hop gateway topologies. iroh is the CRDT substrate (not a Translator), so sync-delivered docs land on remote nodes with `event.origin = None` and fan out to all eligible local transports — exactly the cross-transport visibility this ADR exists to deliver.
- **Per-doc / per-collection / per-subscription scoping is composable.** Operators control bandwidth, security, and reach without modifying transport code.
- **`peat-protocol` stops owning gateway behavior.** It owns codecs (translators) only. `peat-mesh` owns the orchestrator. Layering aligns with the dependency graph.
- **`BleGateway` becomes deprecated.** Its `ingest_*` / `observe_*` methods are absorbed into the generic `TransportManager` flow + `BleTranslator` impl.

### Negative

- **Migration cost.** `BleGateway`'s callers in peat-ffi and peat-atak-plugin (PR #802, #803, #804) need to be re-pointed at `TransportManager`. The shape is similar, but it's churn.
- **Document-schema addition.** `allowed_transports` is a new top-level optional field. Existing documents lacking it are interpreted as `None`, preserving today's broadcast-to-all-capable behavior. Codecs must tolerate the field's presence in the wire-encode path; codecs that elide `None` produce byte-identical output for documents that don't use it.
- **Observer task is the single ingress to fan-out.** If the observer task itself stalls (e.g. holding a lock the doc store also wants), every transport's outbound flow stalls with it. The orchestration above prevents the *known* sources of stall — encode/send happen on per-transport drain tasks, not the observer task — but `Node::observe`'s implementation must remain non-blocking on the observer-task side or this guarantee weakens. Mitigation: enforce in `Node::observe`'s contract that the stream is a non-blocking receiver and dispatch into per-transport channels is a `try_send` with QoS-aware overflow handling.
- **QoS-aware drop policy is load-bearing, not optional.** The orthogonality claim below — `allowed_transports` controls *eligibility*, QoS controls *which-of-the-eligible-survive-under-load* — depends on the QoS policy being on the channel boundary as specified above. A non-QoS-aware drop policy (e.g. tail-drop) would discard high-priority ADR-019 traffic indistinguishably from chatter under load, undoing ADR-019 in exactly the conditions it exists for.

### Composes with

- **ADR-019 (QoS):** transport selection is orthogonal to QoS priority. A high-priority message still respects `allowed_transports`, but within allowed transports QoS determines preemption.
- **ADR-042 (UDP bypass):** `bypass_sync` shortcuts CRDT for direct delivery; `ChangeEvent.origin` prevents same-node echo on the bridging path. They address different concerns and can coexist on the same document.
- **ADR-046 (targeted delivery):** `target_nodes` is *whom*, `allowed_transports` is *over which transport*. Both apply; gateways that can encode the doc skip the doc unless they can reach a target node over an allowed transport. **Composition mechanism for Slice 1:** `target_nodes` filtering lives in `encode_outbound` (the codec returns `None` when none of `target_nodes` is reachable on its transport — the codec already has a peer-reachability view via `TranslationContext`). This deliberately keeps the orchestration pseudocode `target_nodes`-agnostic in Slice 1, where the feature isn't shipped yet. **If a Slice 2+ codec finds the codec-side filter leaks too much transport state into the encode path** (e.g. a codec needs to enumerate its peer set inside `encode_outbound`, which feels like an inversion of concerns), the orchestrator can be extended with a per-target enqueue path: each `(transport_id, target_node)` produces its own `FanoutItem` with `ctx.peer` populated, and the codec just encodes against the named target. The trait does not need to change for either path; the per-target extension is a `TransportManager` orchestration change, picked up by codecs as a free upgrade.

## Implementation

Phased, independently shippable:

1. **Slice 1:** `Translator` trait + `TranslationContext` (peat-mesh); `Node::publish_with_origin` + `ChangeEvent.origin` (peat-mesh API additions); `TransportManager` observer-driven fan-out with per-transport bounded mpsc channels (depths configurable at registration time, defaults per transport family — see "Per-transport channel sizing"); QoS-aware drop policy on overflow; refactor `BleTranslator` to impl `Translator`; deprecate `BleGateway`. **Platform binding coverage:** BlueZ-on-Linux and aarch64 (Jetson Orin Nano, Raspberry Pi) hosts run native Rust and consume `TransportManager` and `Translator` directly via in-process API — no FFI binding layer is involved on those targets, and host-language divergence applies only to Android (JNI) and iOS (UniFFI). **Generic `OutboundFrameCallback` therefore ships in two binding flavors only:** JNI for Android (plugin dispatches by `transport_id`) and UniFFI for iOS, so neither cross-language host retains a per-transport `BleGateway`-shaped FFI defeating the deprecation. The native Rust hosts use `TransportManager::register_translator` directly. Slice 1 fans out `Updated` and `Initial` `ChangeEvent` variants only; `Removed` is deferred to Slice 2 (see Delete propagation below). Delivers cross-transport BLE↔iroh peripheral propagation end-to-end (M5Stack sees iroh-only peers, today's gap).

2. **Slice 1.5 — TAK trait-stability gate:** before any other Slice 2 work begins, port the TAK adapter (ADR-029) onto the Slice 1 `Translator` trait. If the migration requires changing the trait shape, the change is made here and Slice 1 implementations update in lockstep — Slice 2 work does not begin until the trait survives at least two real codec implementations (BLE + TAK). This addresses the QA-flagged risk that a BLE-only refactor could lock in a signature that doesn't survive contact with the second codec.

3. **Slice 2:** `allowed_transports` field, `Query::AllowedTransport` variant, collection-registry default, publish-time validation (rejects unknown transport IDs and `Some(vec![])`). **Delete propagation (Removed-event fan-out) ships in Slice 2** — see contract below. Purely additive on the existing surface — `None` everywhere preserves Slice 1 behavior; codecs that don't yet implement delete-encoding fall through harmlessly. **`Query` must be marked `#[non_exhaustive]` in Slice 1** so that adding `AllowedTransport(String)` in Slice 2 is genuinely additive rather than a compile-time break for every consumer that exhaustively matches on `Query` (peat-protocol, peat-atak-plugin, peat-sim, peat-ffi). If `Query` cannot be made `#[non_exhaustive]` (downstream constraints), call this out as a Slice 2 cross-repo migration list.

### Delete propagation (Slice 2 scope)

Slice 1's fan-out spec handles `Updated` and `Initial` `ChangeEvent` variants. `Removed` events — emitted when a doc is explicitly deleted or its TTL expires per ADR-016 — are **deferred to Slice 2**, not silently dropped. Doing this in Slice 1 would force the orchestrator's BLE refactor to lock in a delete contract before two real codecs (BLE + TAK in Slice 1.5) have validated it; doing it never would leave a known divergence path between iroh CRDT segments (where Automerge tombstones propagate) and BLE/TAK/LoRa segments (where deletes silently don't), exactly the cross-transport divergence ADR-059 exists to prevent.

**Slice 1 ship-readiness mitigation for the delete-propagation gap.** The window between Slice 1 and Slice 2 is a real operational concern: any doc deleted or TTL-expired during Slice 1 propagates over iroh but not over BLE/TAK/LoRa, leaving stale state on M5Stack screens and TAK clients indefinitely. To avoid shipping Slice 1 with this divergence as a silent failure mode:

1. **`Removed` events emit a `transport.delete_dropped{transport_id}` metric** in the Slice 1 observer dispatcher — operators see, per transport, exactly how many deletes the bridge is failing to propagate.
2. **The doc registry's collection registration grows an opt-in `requires_delete_propagation: bool` flag** at registration time. Collections that set it (e.g. emergency alerts, ephemeral chat with TTL) are **rejected at registration** during Slice 1 unless every transport listed in their `default_allowed_transports` (or the registered set if `default_allowed_transports` is `None`) advertises delete support — which in Slice 1 is none of them. Operators registering such collections see the constraint at the right layer and either wait for Slice 2 or scope the collection to iroh-only via `default_allowed_transports = Some(vec!["iroh"])` (iroh's CRDT layer carries deletes natively).
3. **Slice 2 targets one minor release after Slice 1.** This is the schedule intent, not a hard release-gate — unenforced "must" language drifts to aspiration once implementing PRs land. The operational guarantee against the divergence window is the Slice 1 ship-readiness mitigation already specified above (`transport.delete_dropped` metric per transport plus the `requires_delete_propagation` collection-registration gate that rejects delete-sensitive collections unless they're scoped iroh-only). If Slice 2 slips, those two artifacts make the gap visible and recoverable; they are the binding contract, not the calendar.

The Slice 2 contract:

- The `Translator` trait gains an `async fn encode_delete(&self, doc_id: &DocumentId, ctx: &TranslationContext) -> Option<Vec<u8>>` method. `None` is "decline" (codec has no delete representation in its wire format — same semantics as `encode_outbound` declines); `Some(bytes)` is the codec's wire-format tombstone or delete frame.
- `TransportManager`'s observer task extends to handle `Removed` events: same scoping rules as `Updated` (origin skip + `allowed_transports` filter), then call `Translator::encode_delete` instead of `encode_outbound`.
- Per-codec implementation: `BleTranslator` already has the wire framing for marker-byte 0xC1+ delete frames per ADR-041 — Slice 2 adds the `encode_delete` method that emits one. TAK uses CoT's existing event-staleness mechanism (set `stale=now` to mark deleted). LoRa/SBD codecs decide per-codec.
- Codecs without a delete representation return `None` from `encode_delete`. Operators who care about cross-transport delete consistency on a constrained transport must either (a) use `allowed_transports` to exclude that transport from collections that need delete propagation, or (b) accept that deletes don't reach those peers until they reconnect through a transport that does carry deletes.

Slice 2's delete contract may evolve based on Slice 1.5 TAK migration findings — same trait-stability discipline as the rest of the trait surface.

## Alternatives Considered

- **Sibling gateways (one-off per transport).** Rejected: forces N copies of observer scaffolding, JNI bridge, loop prevention. Diverges over time.
- **Generic `Gateway<Translator, Sink>` trait, peat-protocol-owned.** Rejected: keeps the layering oddity (gateway logic below the layer it bridges to) and duplicates what `TransportManager` already exists to do.
- **Routing policy in `Translator::encode_outbound` only (no `allowed_transports` field).** Rejected: forces every operator scope to be encoded into translator behavior, no per-document override possible.

## Supersedes

- The one-off `BleGateway` design in `peat-protocol/src/sync/ble_gateway.rs` (introduced as part of PR #802, never formalized in an ADR). `BleGateway`'s functionality is fully absorbed by `TransportManager` + `BleTranslator`-as-`Translator`. The struct is deprecated and removed in Slice 1.
