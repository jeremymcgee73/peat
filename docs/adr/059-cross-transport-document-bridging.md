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

### Codec placement (where each `Translator` lives)

Each transport's `Translator` impl lives in **the same crate as that transport**, gated behind a `mesh-translator` Cargo feature so the transport crate can still be consumed standalone — without dragging `peat-mesh` into the dep graph — by callers that only need the radio side. This keeps the rule unambiguous as new transports land:

- BLE codec → `peat-btle` (behind `peat-btle/mesh-translator`). Standalone Bitchat-style consumers compile peat-btle without the feature and never see peat-mesh.
- TAK CoT codec → `peat-transport` (behind `peat-transport/mesh-translator`). Today's home, since the existing TAK Server / Mesh SA radio transports already live there.
- Future LoRa / SBD / mavlink codecs → their own `peat-lora` / `peat-sbd` / `peat-mavlink` crates, each behind the same feature shape.

Two consequences worth naming so future codec PRs don't re-litigate:

1. **The transport crate must remain compilable without the feature.** This is a forcing function: it prevents the codec from leaking peat-mesh types into transport-side public APIs (radio code stays decoupled from CRDT machinery). CI runs both `--no-default-features` and `--features mesh-translator` builds.
2. **Codec naming tracks the wire format, not the transport family.** TAK is an ecosystem with multiple wire formats (CoT XML, CoT protobuf, Mission Package); the codec is `CotTranslator` in `peat-transport/src/tak/cot_translator.rs`, not `TakTranslator`. The `transport_id()` returned to `TransportManager` still names the transport family (`"tak"`), since that's the unit `allowed_transports` filters on — finer-grained codecs within one family register under suffixed IDs (`"tak-mp"`, etc.) only if they need separate origin tagging.

Slice 1.5 (TAK trait-stability gate) lands the rule for `peat-transport`. The `BleTranslator` migration from `peat-protocol` to `peat-btle` is a tracked follow-up — same shape, cross-repo move.

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

---

## Amendment 1 — BLE receive-side wire format and decoded-document callback (Slice 1.b.3)

**Status**: Proposed
**Date**: 2026-05-01
**Authors**: Kit Plummer
**Organization**: Defense Unicorns

### Context

Slice 1.c shipped `BleOutboundFrameDispatcher` in peat-atak-plugin: peat-mesh's per-transport fan-out hands `BleTranslator::encode_outbound` bytes to the dispatcher, which forwards them to `PeatBleManager.broadcastBytes` for GATT write. The outbound side works end-to-end.

The receive side does not. peat-btle's top-level GATT receive dispatch (`peat_mesh::on_ble_data_received*` → `decrypt_document` → leading-byte discrimination at `peat_mesh.rs:2520` between `DeltaDocument::is_delta_document` (0xB2) and the fall-through `PeatDocument::decode` path) has no recognizer for translator-emitted frames. Inside `PeatDocument::decode` the legacy section markers (0xAB EXTENDED, 0xAC EMERGENCY, 0xAD CHAT, 0xAE ENCRYPTED, 0xAF PEER_E2EE, 0xB0 KEY_EXCHANGE) drive typed legacy decoders that produce `Peripheral` / `EmergencyEvent` / `ChatCRDT` directly. **None of these branches invoke `BleTranslator::decode_inbound`**, and the marker space has no recognizer for translator frames. Translator-encoded outbound bytes thus arrive at peers whose receive dispatch silently fails to interpret them: `PeatDocument::decode` reads the first 4 bytes as a `version` field, fails the implicit version check or falls off the end of the counter parse, and `merge_document` returns `None` with no diagnostic.

This was a load-bearing assumption ADR-059's "Inbound transport bridge" pseudocode glossed over: `on bytes from transport T → T.translator.decode_inbound(bytes, ctx)` assumed each transport's wire format already self-identifies translator frames. For BLE, it does not.

The receive-side gap blocks the remaining plugin migration. Until peat-btle dispatches translator frames through `decode_inbound`, every plugin call that switches from a legacy `PeatBleManager.broadcast*`-shaped path to `node.publishDocument(collection, json)` emits bytes onto the wire that no peat-btle peer can decode. Tracked work blocked by this gap: peat-atak-plugin #65 (outbound migration), #66 (inbound observer-stream migration), #69 (Rust-owned read-API), #26 (PeatBleManager removal).

### Decision

#### Wire format: a single top-level translator-frame marker plus a fixed collection code

Introduce one new marker byte in peat-btle's top-level frame-discriminator space — alongside the existing `RELAY_ENVELOPE_MARKER` (0xB1) and `DELTA_DOCUMENT_MARKER` (0xB2):

```
TRANSLATOR_FRAME_MARKER = 0xB6
```

Frame layout, post-decryption:

```
+---------+---------+----------------------------+
| 1 byte  | 1 byte  |   N bytes                  |
| 0xB6    | code    |   postcard payload         |
+---------+---------+----------------------------+
```

`code` is a stable u8 identifier for the BleTranslator collection:

| code   | collection         | typed payload         |
|--------|--------------------|-----------------------|
| `0x01` | `tracks`           | `BlePosition`         |
| `0x02` | `platforms`        | `BlePeripheral`       |
| `0x03` | `alerts`           | `BleEmergencyEvent`   |
| `0x04` | `canned_messages`  | `BleCannedMessage`    |

Codes are **immutable** once assigned. A new collection appends a new code; collection renames register a new code without retiring the old one. The receiver maps `code` to the configured collection name (`BleTranslator` already supports operator-renamable collection strings via `TranslationConfig.{tracks,platforms,alerts,canned_messages}_collection`); the wire-side identifier stays a u8 so renames are local to a deployment and don't drift across the fleet.

**Encode/decode contract for the trait boundary.** The full wire bytes — marker + code + postcard payload — are produced by `BleTranslator::encode_outbound` and consumed by peat-btle's receive dispatch. The framing **does not** cross the FFI boundary as a host-side responsibility (see "Outbound framing" under Implementation for the rationale). On the decode side, the receive dispatch reads the marker and code in order to *route at all*, so by the time it invokes `BleTranslator::decode_inbound` the marker and code are already-stripped: `decode_inbound` takes the payload-only bytes with `ctx.collection` populated from the code lookup. This is asymmetric in shape (encode emits framed, decode takes payload-only) but symmetric in ownership (peat-btle owns both ends of the wire format), which is the load-bearing property.

**Why a fixed-byte code, not a length-prefixed string.** ADR-059's main wire-format-stability concern (§"Wire-format codec contract") is *transport IDs* (`"ble"`, `"iroh"`, `"tak"`) — those round-trip across nodes and registration order varies. Collection identifiers within `BleTranslator` are different: peat-btle is the single source of truth for what BLE-translator collections exist (the closed set is `tracks` / `platforms` / `alerts` / `canned_messages`, plus future additions made in peat-btle itself), so a peat-btle-internal numbering is stable by construction. The 1-byte saving over a length-prefixed string matters at the BLE MTU (~150 B sustained per ADR-041), and the constraint that codes are immutable substitutes for the registration-order portability the string form would buy.

**Why 0xB6, specifically.** Three reservations sit between the existing top-level marker family and the app-layer `DocumentType` registry range, leaving 0xB6 as the first free byte for translator frames:

| Byte | Owner | Status |
|------|-------|--------|
| 0xB1 | `RELAY_ENVELOPE_MARKER` (peat-btle/src/document.rs) | shipped |
| 0xB2 | `DELTA_DOCUMENT_MARKER` (peat-btle/src/document.rs) | shipped |
| 0xB3 | `IDENTITY_ATTESTATION_MARKER` (peat-btle ADR-001 Phase 1) | reserved, not yet emitted |
| 0xB4 | `REVOCATION_MARKER` (peat-btle ADR-001 Phase 3) | reserved, not yet emitted |
| 0xB5 | `KEY_ROTATION_MARKER` (peat-btle ADR-001 Phase 4) | reserved, not yet emitted |
| **0xB6** | **`TRANSLATOR_FRAME_MARKER` (this amendment)** | **first free top-level marker** |
| 0xB7–0xBF | reserved for future translator-frame variants (this amendment) | future |
| 0xC0–0xCF | app-layer `DocumentType` registry (peat-btle/src/registry.rs) | shipped |

An earlier draft of this amendment claimed 0xB3 as the translator marker. QA review surfaced the collision with peat-btle's `ADR-001-trust-architecture.md` "Wire Format Summary" table (lines 524–526), which assigns 0xB3 / 0xB4 / 0xB5 to the trust phases. Although the ADR-001 markers are not yet emitted as Rust constants (the existing `IdentityAttestation::encode/decode` writes a 108-byte raw payload with no leading marker), the reservations are committed by the prior ADR — so the new entrant moves. 0xB6 keeps translator frames inside the top-level discriminator family alongside `is_delta_document`, leaves the trust phases free to roll out per ADR-001, and stays disjoint from the registry's `DocumentType` range so registry consumers don't mis-imply translator frames as `DocumentType` instances.

#### Receive-side dispatch

`peat_mesh::on_ble_data_received_anonymous` (and its identified-peer siblings at `peat_mesh.rs:2245` and `:2432`) gain a 0xB6 branch on the decrypted payload's first byte, alongside the existing `DeltaDocument::is_delta_document` check at `peat_mesh.rs:2520`. The branch:

```text
on decrypted[0] == 0xB6:
    if decrypted.len() < 2:                     // truncated frame
        bump metric ble.translator_frame_truncated
        return None
    let code = decrypted[1];
    let payload = &decrypted[2..];
    let collection = match self.translator.code_to_collection(code) {
        Some(c) => c,                           // configured collection name
        None => {                               // unknown code: forward-compat
            bump metric ble.translator_unknown_code{code}
            return None                         // silently drop — newer peer
        }
    };

    let ctx = TranslationContext {
        collection: Some(collection.into()),
        local_callsign: self.config.local_callsign.clone(),
        local_transport_id: Some(self.peripheral_id_hex()),
        local_wire_id: Some(self.peer_peripheral_id_hex(identifier)),
        cell_id: self.config.cell_id.clone(),
        peer: Some(identifier.into()),
    };

    match self.translator.decode_inbound(payload, &ctx).await {
        Ok(Some(doc)) => match self.decoded_document_callback.read() {
            Some(cb) => cb.on_document(collection, doc, ctx.peer.as_deref()),
            None => {                           // 1.b.3-ships-before-1.b.4 window
                bump metric ble.translator_no_callback{collection}
                // Drop. Frame was decoded successfully but no peat-ffi
                // consumer is wired yet. Operator-visible via the
                // counter so the gap is observable, not silent.
            }
        }
        Ok(None) => {                           // codec declined; not for us
            bump metric ble.translator_decline{collection}
        }
        Err(e) => {                             // wire-format-drift signal
            bump metric ble.translator_decode_error{collection}
            log::warn!(...);
        }
    }
```

`local_wire_id` is the hex-encoded peripheral_id of the peer that delivered the bytes, mirroring the contract `BleTranslator::decode_inbound` already requires for the `tracks` collection (`peat-btle/src/translator.rs:850–860`). The receive dispatch is the correct layer to stamp this — connection identity is known here and nowhere upstream — and stamping it explicitly avoids the silent `peripheral_id = 0` collapse the trait contract was hardened against.

The dispatch is **non-blocking on the GATT receive task**: the receive side already owns a Tokio runtime (peat-btle's `gatt::service` spawns per-connection tasks). `decode_inbound` is `async`; decode happens on the receive task, and the decoded-document callback is invoked synchronously from that task. If a callback consumer (peat-ffi) wants async ingest into `Node::publish_with_origin`, it spawns its own task — the callback contract is "non-blocking, non-panicking, returns quickly," matching the existing `OutboundFrameCallback` shape established by Slice 1.b.

#### Decoded-document callback

A new peat-btle public API mirroring the outbound callback shape:

```rust
/// Receives Documents decoded from inbound BLE translator frames.
///
/// Invoked by peat-btle's GATT receive dispatch after a 0xB6 frame is
/// successfully routed through `BleTranslator::decode_inbound`. The
/// callback is expected to be non-blocking and panic-free; consumers
/// that need async publish should spawn their own task.
///
/// `collection` is the BleTranslator collection name (e.g. "tracks"),
/// suitable for direct use as the first argument to
/// `Node::publish_with_origin`. `peer` carries the BLE peer's
/// identifier for diagnostics and `target_nodes` checks (ADR-046).
#[cfg(feature = "mesh-translator")]
pub trait DecodedDocumentCallback: Send + Sync + 'static {
    fn on_document(
        &self,
        collection: &str,
        doc: peat_mesh::sync::Document,
        peer: Option<&str>,
    );
}

#[cfg(feature = "mesh-translator")]
impl PeatMesh {
    pub fn set_decoded_document_callback(
        &self,
        cb: Arc<dyn DecodedDocumentCallback>,
    );
}
```

**Storage shape and the no-callback window.** The field is `Option<Arc<dyn DecodedDocumentCallback>>` (behind whatever interior-mutability primitive `PeatMesh` already uses for similar setters), initialized to `None` at construction. Slice 1.b.3 ships peat-btle with the trait, the setter, and the receive-dispatch branch; Slice 1.b.4 — landing in a *later* peat-ffi release after a peat-btle release with 1.b.3 — installs the callback. **The release window between 1.b.3 shipping and 1.b.4 wiring is real**: a deployment running peat-btle-with-1.b.3 plus a peat-ffi version that predates 1.b.4 will receive 0xB6 frames, decode them successfully, and have no consumer to hand them to.

Specified behavior in that window: drop the decoded `Document` and bump `ble.translator_no_callback{collection}`. The frame is not panicked-on, not error-logged at warning level (a missing consumer in the release-skew window is expected, not a fault), and not retried. The counter makes the gap observable — operators see exactly how many frames are being decoded into the void during the rollout — and lets peat-ffi integration tests assert the behavior is "no-op-drop with telemetry," not panic, not error, not silent. peat-ffi's 1.b.4 implementation can register the callback before opening any GATT connection, which closes the window from the consumer side once 1.b.4 is deployed.

peat-ffi's responsibility is to install a callback that calls `Node::publish_with_origin(collection, doc, Some("ble"))`, threading the per-event origin so the local fan-out skips re-emitting onto BLE. peat-atak-plugin and wearos-tak-civ never see this callback directly — they continue to consume doc-store observer streams via their existing FFI.

**Why a separate callback rather than reusing `OutboundFrameCallback`.** The two flow in opposite directions. `OutboundFrameCallback` lifts a Document-shaped event from peat-mesh's fan-out into a transport-specific frame for the host to write. `DecodedDocumentCallback` lifts a transport-specific frame from the radio into a Document for peat-mesh to ingest. Conflating them onto one trait would force every consumer to discriminate direction at the call site; keeping them separate matches the underlying control flow and the existing `OutboundFrameCallback` shape.

#### Backwards compatibility with mixed fleets

The 0xB6 marker is purely additive. Three classes of peer matter:

1. **Legacy senders** (M5Stack on pre-#70 firmware, wearos-tak-civ pre-update, peat-atak-plugin pre-adoption) emit only `PeatDocument`-shaped frames with the existing 0xAB/0xAC/0xAD/0xAE/0xAF section markers. New peat-btle receivers MUST keep dispatching these — the existing legacy decoders stay in place and the 0xB6 branch is added alongside, not in place of them.

2. **Legacy receivers** (same fleet, before peat-btle ships #70) see 0xB6-prefixed bytes as something `PeatDocument::decode` was never designed to handle. There is no version-range check at `peat-btle/src/document.rs:531`; any `version` u32 is accepted, and silent-drop on a 0xB6-prefixed payload depends entirely on `GCounter::decode` rejecting `data[8..12]` as `num_entries > MAX_COUNTER_ENTRIES` (= 256, `peat-btle/src/sync/crdt.rs:55`). The structural hazard: **any translator-encoded payload whose wire bytes 8–11 (the bytes `PeatDocument::decode` reads as `num_entries`) form a value ≤ 256 will be accepted as a valid-but-garbage `PeatDocument` and merge a junk GCounter into the receiver's state.** This is reachable for plausible inputs, not a theoretical edge case.

   Concrete instance: the actual `BlePosition` struct (`peat-btle/src/translator.rs:80`) is `{ latitude: f32, longitude: f32, altitude: Option<f32>, accuracy: Option<f32> }`. A translator-encoded position broadcast at the (0, 0) origin with `altitude = None` and `accuracy = None` produces a 10-byte postcard payload of all-zero bytes (two zero-f32s plus two `None` tags), and the wire frame `[0xB6, code, 0x00 × 10]` puts `[lon_byte2, lon_byte3, altitude_tag, accuracy_tag] = [0x00, 0x00, 0x00, 0x00]` at the `num_entries` offset → `num_entries = 0` → slides through the bound check → `PeatDocument::decode` returns `Some(garbage_doc)` with an empty counter and no sections → `merge_document` pollutes the receiver's GCounter. Any tactical device broadcasting from the (0, 0) origin would trigger this on every peer running pre-#70 peat-btle, and any input where bytes 8–11 happen to fall ≤ 256 has the same effect.

   **Therefore #70 itself adds an explicit reserved-marker rejection check** in `on_ble_data_received_anonymous` (and identified-peer siblings) **alongside** the new 0xB6 dispatch branch. The check fires *before* fall-through to `merge_document` / `PeatDocument::decode`:

   ```text
   on first_byte in 0xB6..=0xBF:
       0xB6 → translator dispatch (specified above)
       0xB7..=0xBF → reserved future translator markers; silent drop
                     bump metric ble.reserved_marker_drop{marker}
                     return None (do not call merge_document)
   ```

   The 0xB7–0xBF range is reserved for future translator-frame variants (multi-frame, fragmented, etc.) so adding them later doesn't require touching legacy-rejection code on the receive path. The check is a 4-line addition, ships in the same peat-btle release as the 0xB6 branch, and is a deployment prerequisite: **operators MUST roll out the peat-btle release containing this check to every BLE peer in the deployment before enabling 0xB6 emission on any peer.** Senders without this rollout discipline pollute every receiver's GCounter probabilistically and (for `lat = 0.0`) deterministically — there is no graceful pre-#70 silent-drop guarantee.

   Pre-#70 peers that the operator cannot upgrade (e.g. M5Stack devices that have aged out of firmware support) make the deployment a 0xB6-disabled deployment until they are retired. The dual-emit-disable knob (Implementation step 4 below) lets operators with such peers run 100% legacy emit and accept the no-cross-transport-translator-frames cost as the explicit alternative to the GCounter-pollution hazard.

3. **New peers, both directions** speak both. **Dual-emit is the default on the sender side** until peat-btle telemetry reports zero legacy-section-marker traffic across a deployment: the outbound dispatcher writes the 0xB6 translator frame *and* the corresponding legacy frame for the same logical doc.

   **The legacy emit is gated by `BleTranslator::encode_outbound`'s success.** When `encode_outbound` returns `None` — codec declined, `target_nodes` filter excluded all reachable BLE peers (per ADR-059 §"Composes with — ADR-046"), `allowed_transports` excluded BLE (Slice 2+), or any other decline reason — the legacy emit is **also suppressed**. The dual-emit trigger is "I have a translator frame to send via BLE, also emit the matching legacy frame," not "translate and broadcast independently." This means every ADR-046 / ADR-059-Slice-2 / encode-time scoping rule applied to the translator frame is automatically applied to the legacy frame as well; the legacy path does not become a leak channel for target-restricted documents during the transition window.

   Loop prevention is unaffected because both emit paths originate the same `publish_with_origin(.., Some("ble"))` call upstream — Automerge merge on the receiver collapses both deliveries to one doc-store update.

   Dual-emit costs roughly 2× wire bytes per logical doc during the transition window. The cost is intentional: it's the price of not breaking interop with the existing ATAK/M5Stack/watch fleet. **The disable knob ships in Slice 1.b.3 itself** (per Implementation step 4 below) — operators with fleet-uniform deployments may disable it from day one, accepting the legacy-peer-blackhole risk explicitly. Slice 1.b.5's separate scope is the eventual removal of the legacy-emit *code path*, not the per-deployment toggle.

A receiver that sees both a 0xB6 frame and the matching legacy frame for the same logical doc ingests both — each receive path calls `Node::publish_with_origin(.., Some("ble"))` independently, and Automerge merges both into the doc store. Whichever publish lands second produces a no-op CRDT merge on identical state; per ADR-059 §"Automerge no-op-merge suppression," no-op merges do not fire `ChangeEvent`s. The local observer therefore fires **once per logical doc**, not twice — downstream observer-stream consumers (UI redraw, alert tones, analytics counters, audit logs in peat-atak-plugin and wearos-tak-civ) see exactly one event regardless of how many BLE delivery paths brought the doc in. The single fan-out emission carries `origin = Some("ble")`, so the BLE channel skips on origin match and no echo is produced.

#### Loop-prevention invariant under dual delivery

The fan-out spec already covers this case (see "Concurrent ingest of the same logical doc"), but worth restating explicitly: a doc that arrives once via 0xB6 and once via legacy 0xAB/etc. produces two separate `publish_with_origin(.., Some("ble"))` calls on the receiver. The first publish (whichever lands first) drives a real state change → the local observer fires once → fan-out emits once with `origin = Some("ble")` → BLE channel skips on origin match → iroh/TAK/LoRa transports receive one copy. The second publish merges no-op on identical CRDT state and **does not fire the observer** per ADR-059 §"Automerge no-op-merge suppression." Net: one observer fire, one cross-transport emission, no BLE echo, no doubled downstream effects on observer-stream consumers.

The no-op-merge-suppression dependency here is the **same contract** ADR-059 already load-bearingly relies on for multi-hop loop prevention (gateway A→B→A round-trips). This amendment inherits that guarantee; it does not introduce a new dependency. Slice 1's A→B→A regression fixture (#55) and this amendment's dual-delivery fixture (Slice 1.b.3, below) exercise the contract from two different stress angles.

### Consequences

#### Positive

- **Closes the receive-side gap that gates all remaining plugin migration.** peat-atak-plugin #65, #66, #69, and #26 unblock as soon as peat-btle ships this and peat-ffi wires the callback. Today they are all parked behind "BLE peers can't decode the bytes my plugin would emit."
- **Wire-format addition is single-byte-discriminator, fixed-cost.** No new envelope around existing legacy frames; no length-prefix on collections; no per-link negotiation. 2 bytes of overhead per translator frame against a ~150 B MTU.
- **Mixed-fleet transition is operationally safe by default.** Dual-emit means a partial rollout never leaves any peer holding stale state because of a wire-format mismatch. Legacy-only peers see legacy frames, new peers see both, iroh-only peers see Automerge ops as before.

#### Negative

- **Dual-emit doubles BLE airtime per logical doc during the transition.** Real cost in ADR-019 QoS terms — high-burst scenarios may see preemption fire earlier on the BLE channel. Mitigation: dual-emit is per-deployment configurable from day one (knob ships in Slice 1.b.3, see Implementation step 4); deployments certain of fleet uniformity may disable it immediately, accepting the legacy-peer-blackhole risk explicitly.

- **0xB6 emission is gated on a fleet-wide peat-btle release rollout.** Pre-#70 peat-btle's `PeatDocument::decode` does not provide a guaranteed silent-drop on 0xB6-prefixed payloads (see §"Backwards compatibility… 2"); operators must roll out the peat-btle release containing the reserved-marker rejection check to every BLE peer **before** enabling 0xB6 emission anywhere. This is a real operational constraint, not just a soft preference: skipping the rollout corrupts the GCounter on every legacy receiver that sees a translator frame whose `data[8..12]` happens to fall under `MAX_COUNTER_ENTRIES`.
- **Two parallel decoders on the receive side until legacy retires.** The 0xAB/0xAC/0xAD/etc. section dispatch inside `PeatDocument::decode` and the new top-level 0xB6 branch both produce app-visible state; a future migration to retire legacy decoders is an additional cleanup slice (out of scope for this amendment). Until then, peat-btle carries both.
- **Code-table is a peat-btle-internal stable identifier list.** Adding a translator collection to BleTranslator now requires (a) a new code in the table and (b) coordinated rollout so peers ship the recognizer before any sender emits the new code. Same constraint as any wire-format extension.

#### Composes with

- **ADR-019 (QoS):** dual-emit lives on the channel boundary; QoS preempts both copies under load identically.
- **ADR-046 (targeted delivery):** `target_nodes` is checked codec-side in `encode_outbound` (ADR-059 §"Composes with — ADR-046"). Receive-side dispatch is target-agnostic; if the doc arrived, it was for someone reachable. The `peer` argument on the decoded-document callback is available for codec-internal `target_nodes` filtering should a future Slice need it.
- **ADR-041 (multi-transport embedded):** 0xB6 slots into the existing top-level marker family ADR-041 establishes for peat-btle wire framing. Choice of byte and dispatch-table extension match conventions already in `peat-btle/src/document.rs` and `peat-btle/src/sync/delta_document.rs`.

### Implementation

**Slice 1.b.3 — peat-btle receive-side translator-frame routing.** Lands in peat-btle behind the existing `mesh-translator` Cargo feature. Surface area:

1. `peat-btle/src/document.rs`: add `pub const TRANSLATOR_FRAME_MARKER: u8 = 0xB6;` alongside the existing marker constants.
2. `peat-btle/src/translator.rs`: add `code_to_collection(&self, code: u8) -> Option<&str>` and `collection_to_code(&self, name: &str) -> Option<u8>` methods on `BleTranslator`, reading from `TranslationConfig`. Expose `pub const COLLECTION_CODE_TRACKS: u8 = 0x01;` etc. — public so the in-tree dual-emit gating logic and tests can reference the same canonical values, **not** for host-side outbound use (per Step 5, the host owns no per-translator-frame logic).
3. `peat-btle/src/lib.rs` (and the `uniffi_bindings.rs` surface): add `DecodedDocumentCallback` trait and `set_decoded_document_callback` on the public `PeatMesh`-bearing type, both `#[cfg(feature = "mesh-translator")]`. For UniFFI, expose as `Box<dyn DecodedDocumentCallback>` per the existing callback pattern.
4. `peat-btle/src/peat_mesh.rs`: extend `on_ble_data_received_anonymous` (and identified-peer siblings) with **two** first-byte branches before the existing `DeltaDocument::is_delta_document` check:

   - `first_byte == 0xB6`: translator dispatch. Invokes `decode_inbound` and the callback as specified in §"Receive-side dispatch."
   - `first_byte ∈ 0xB7..=0xBF`: reserved-future-translator-marker silent-drop. Returns `None` without falling through to `merge_document`. Reserves the range so that future translator-frame variants (multi-frame, fragmented, etc.) don't require touching legacy-rejection logic on the receive path.

   Surface these counters at the receive layer:

   - `ble.translator_frame_truncated`
   - `ble.translator_unknown_code{code}`
   - `ble.translator_decode_error{collection}`
   - `ble.reserved_marker_drop{marker}` (0xB7–0xBF)
   - `ble.legacy_section_marker_recv{marker}` (0xAB / 0xAC / 0xAD / 0xAE / 0xAF — counts inbound legacy frames per section type)
   - `ble.legacy_section_marker_send{marker}` (same set, counted at the dual-emit dispatch site on the outbound path)

   The two `legacy_section_marker_*` counters drive the Slice 1.b.5 retirement decision. They are listed alongside the translator-side counters because the retirement criterion ("zero legacy traffic across the deployment") is operationally undecidable without them — without these counters the criterion has no data to consult and "deferred" becomes "indefinite."

   **Dual-emit toggle.** Add `enable_legacy_emit: bool` (default `true`) to `PeatMesh`'s outbound-path config and a setter `PeatMesh::set_enable_legacy_emit(bool)` so operators can disable the legacy emit per deployment from day one. When `false`, only the 0xB6 frame is emitted; the legacy-frame builder and `broadcastBytes` call for the legacy path are short-circuited. The toggle is **not** the same thing as Slice 1.b.5: 1.b.5 removes the legacy-emit code path entirely once telemetry confirms zero legacy traffic across all reachable deployments. The toggle is the operator-facing surface; the code-path removal is the eventual cleanup.
5. **Outbound framing lives in `BleTranslator::encode_outbound`, not in the host.** `encode_outbound` returns the fully-framed bytes — `[TRANSLATOR_FRAME_MARKER, collection_code, postcard_payload]` — for every collection it carries. peat-ffi's `OutboundFrameCallback` ships those bytes unchanged to the host, and `BleOutboundFrameDispatcher` (peat-atak-plugin) and the iOS equivalent forward them to `PeatBleManager.broadcastBytes` (or its iOS analogue) without inspection. The host owns no per-translator-frame logic.

   **Why this and not host-side prefixing.** The receive side already owns its decoding inside peat-btle: marker recognition, code lookup, ctx population, `decode_inbound` invocation. Mirroring the encode side keeps the wire-format identity in **one place** — `peat-btle/src/translator.rs` is the sole owner of "what bytes go on the BLE wire for a translator frame" in both directions. An earlier draft of this amendment placed the prefix in each platform binding (Android JNI, iOS UniFFI). QA review surfaced that as an asymmetric multi-binding correctness hazard: every host owning prepend logic with no compiler-enforced consistency, and silent failure modes on the receive side (forgotten prepend → unknown frame → silent drop; double-prepend → `code = 0xB6` → "unknown code" → silent drop). The encryption-envelope precedent does not transfer because session keys are host-managed; a fixed-byte protocol prefix has no host dependency, so the symmetry argument wins.

   **Implementation note.** `encode_outbound` looks up `collection_code` via the same `BleTranslator` config that supports operator-renamable collection names. If the collection code is unknown (operator added a collection without a code-table entry), `encode_outbound` returns `None` — the same decline path it already uses for unrecognized collections. Adding a new translator collection therefore requires (a) extending the typed-struct surface, (b) appending to the code table, (c) shipping a peat-btle release. Senders cannot accidentally emit an un-decodable frame.

6. **Regression test fixtures.** Slice 1.b.3 ships three fixtures, each locking a different invariant:

   - `peat-btle/tests/translator_frame_dual_delivery.rs` — a doc round-trips through both 0xB6 and the matching legacy frame on the same node; assert (a) one fan-out emission with `origin = Some("ble")`, (b) **observer-fire-count == 1** on the doc-store stream (locks the no-op-merge-suppression contract under dual-emit, directly addressing the QA concern that Automerge state convergence alone doesn't guarantee single observer fire to downstream consumers), (c) idempotent doc-store state. Composes with the Slice 1 A→B→A round-trip fixture (#55) — same contract, different exercise.
   - `peat-btle/tests/reserved_marker_silent_drop.rs` — feed `[0xB7, 0x00, ...]` (and one frame per byte 0xB8–0xBF) directly to the receive dispatch; assert (a) `merge_document` is never called, (b) `ble.reserved_marker_drop{marker}` increments, (c) doc-store state is unchanged. Documents the post-#70 silent-drop guarantee explicitly so the §"Backwards compatibility… 2" claim is proof, not assertion.
   - `peat-btle/tests/dual_emit_target_nodes_gating.rs` — publish a doc with `target_nodes = ["other-node"]` (no local-reachable target) and `allowed_transports = ["ble"]`; assert that **neither** the 0xB6 frame nor the legacy frame is emitted. Locks the §"Backwards compatibility… 3" guarantee that the legacy emit inherits encode-time scoping rather than becoming a leak channel.

**Slice 1.b.4 — peat-ffi callback wiring.** Lands in peat-ffi after Slice 1.b.3 ships in a peat-btle release. Installs a `DecodedDocumentCallback` impl that calls `Node::publish_with_origin(collection, doc, Some("ble"))`. Registered at `PeatNode` construction; teardown is symmetric with the existing `OutboundFrameCallback` lifecycle.

**Slice 1.b.5 — legacy-emit code-path removal (deferred).** Removes the dual-emit branch from the outbound dispatcher and retires the legacy-frame builder code. Out of scope for #70 itself.

**Concrete entry criterion** (the round-2 motivation for adding the legacy-marker counters was specifically to keep "deferred" from drifting to "indefinite"; an unquantified criterion would reproduce that failure mode at the exit gate):

- **30 consecutive days of zero `ble.legacy_section_marker_recv{*}` and `ble.legacy_section_marker_send{*}` traffic** across every reachable deployment reporting telemetry to the central counter store.
- 30 days covers two full sprint cycles, surfaces missed deployments that report telemetry only after a node restart, and is operationally cheap (the counters are zero-cost when zero).
- "Reachable" excludes deployments that have explicitly opted out of telemetry; those deployments retain dual-emit indefinitely or follow a per-deployment retirement path the operator manages, which is the right answer for air-gapped fleets.
- If the criterion's duration or scope needs adjustment based on Slice 1.b.3 operational data, the adjustment is captured as an addendum to this amendment, not a verbal handshake on the Slice 1.b.5 PR.

The day-one operator-facing toggle (`enable_legacy_emit`) is the Slice 1.b.3 surface that lets individual deployments opt out of dual-emit ahead of the global retirement; Slice 1.b.5 is the global cleanup that removes the toggle and the dead branch together once the entry criterion above is met.

**Trait stability.** This amendment does not alter the `Translator` trait. `decode_inbound`'s contract is unchanged; the new work is entirely on the peat-btle side of the trait boundary.

### Alternatives Considered

- **Length-prefixed collection name on the wire.** Rejected: ~10–15 B per frame versus 2 B with a fixed code, and the portability argument that justifies length-prefixed strings for *transport IDs* (registration order varies per node) doesn't apply to BLE-internal collection identifiers (peat-btle is the source of truth).

- **Reuse the 0xC0–0xCF app-layer `DocumentType` registry.** Rejected: the registry is for `DocumentType`-trait types (`peat-btle/src/registry.rs`), which are a different abstraction layer from peat-mesh `Document`s. Conflating them would either require dual-implementing each translator collection as both a `DocumentType` and a `Translator` collection, or muddying both abstractions' contracts. Cleaner to keep the marker spaces disjoint.

- **Rip out legacy decoders (0xAB/0xAC/0xAD/0xAE) and replace with translator-only frames in one cut-over.** Rejected: the legacy fleet (M5Stack on existing firmware, wearos-tak-civ pre-update) cannot be re-flashed atomically, and a hard cut-over would silently blackhole every legacy peer's traffic to new peers. Dual-emit-then-retire is slower but operationally recoverable.

- **Encode the collection as a leading varint inside the postcard payload itself, no marker byte.** Rejected: peat-btle's leading-byte dispatch is the canonical layering boundary for "what kind of frame is this." Embedding the discriminator inside the postcard payload would force the receive side to *attempt* postcard decode before knowing whether the bytes are a translator frame — turning dispatch into try/catch instead of switch. Top-level marker dispatch is the pattern every other peat-btle frame type already uses.

- **Add the 0xB6 branch as a section marker inside `PeatDocument::decode`'s while-loop (alongside 0xAB/0xAC/0xAD).** Rejected: those markers are *sections within a single PeatDocument envelope* (version + node_id + counter + sections). Translator frames have no PeatDocument header — they are independent top-level frames. Embedding them as PeatDocument sections would either require synthesising a fake header (wasteful and confusing) or inverting the section-vs-envelope relationship.

- **Host-side prefixing of marker + code on outbound (each platform binding owns the prepend).** Rejected during QA review of an earlier draft. The receive side decodes inside peat-btle; mirroring the encode side keeps the wire format owned in one crate. Host-side prefixing would put correctness-critical, no-compile-time-enforcement logic in every binding (Android JNI, iOS UniFFI, future hosts), with two silent-drop failure modes on the receive side: forgotten prepend produces an unknown frame format, double-prepend produces `code = 0xB6` lookup failure. Single source of truth in `peat-btle/src/translator.rs::encode_outbound` removes the hazard entirely.

---

## Amendment 2 — Decoded-document callback wiring is plugin-side, not peat-ffi-side (Slice 1.b.4 design correction)

**Status**: Proposed
**Date**: 2026-05-02
**Authors**: Kit Plummer
**Organization**: Defense Unicorns

### Context

Amendment 1 §"Decoded-document callback" specified that Slice 1.b.4 would land in peat-ffi:

> **Slice 1.b.4 — peat-ffi callback wiring.** Lands in peat-ffi after Slice 1.b.3 ships in a peat-btle release. Installs a `DecodedDocumentCallback` impl that calls `Node::publish_with_origin(collection, doc, Some("ble"))`. Registered at `PeatNode` construction; teardown is symmetric with the existing `OutboundFrameCallback` lifecycle.

Implementation-time investigation against peat-btle 0.3.0 (the release that shipped the Slice 1.b.3 receive-dispatch path) surfaced that this wiring does not fit the actual data flow:

- **`peat_btle::PeatMesh` is the only type that exposes `set_decoded_document_callback`** — that's where Slice 1.b.3 wired the receive dispatch and the trait setter.
- **peat-ffi never owns a `peat_btle::PeatMesh` instance.** peat-ffi's BLE story is built on `peat_btle::BluetoothLETransport<A: BleAdapter>` (a transport-trait wrapper around an adapter) plus `peat_protocol::transport::btle::PeatBleTransport` (a peat-protocol-side wrapper). Neither owns or exposes `PeatMesh`. peat-ffi's `PeatNode` accordingly has no handle from which to call `set_decoded_document_callback`.
- **peat-atak-plugin owns `PeatMesh` directly** via peat-btle's UniFFI bindings (`PeatBleManager.kt::getMesh() = peatBtle?.mesh`). Other host bindings (wearos-tak-civ, future iOS) follow the same pattern: the host instantiates `peat_btle::PeatMesh` via UniFFI, the host calls `on_ble_data_received_*` directly from the GATT-layer callbacks the host already owns.
- **The Slice 1.b.3 trait `crate::DecodedDocumentCallback` is Rust-only.** It deliberately does *not* carry `#[uniffi::export(callback_interface)]`, so no Kotlin/Swift host can implement it. Amendment 1 §"Decoded-document callback" mentioned a UniFFI export ("For UniFFI, expose as `Box<dyn DecodedDocumentCallback>` per the existing callback pattern") but the trait shape (taking `peat_mesh::sync::Document`, which has no UniFFI binding today) made that unactionable in 0.3.0 without further design.

The combination means Slice 1.b.4 as Amendment 1 wrote it cannot be implemented without either restructuring peat-ffi to own a `PeatMesh` (a non-trivial FFI-boundary change) or routing decoded documents through a different surface.

### Decision

**Slice 1.b.4 lands in the host (peat-atak-plugin and equivalents), not in peat-ffi.** The data flow is:

```text
BLE GATT receive
   ↓  (host's UniFFI/GATT callback)
peat_btle::PeatMesh::on_ble_data_received_*
   ↓  (Slice 1.b.3 receive dispatch — already shipped in 0.3.0)
BleTranslator::decode_inbound_sync(payload, ctx)
   ↓  (Ok(Some(doc)))
PeatMesh.decoded_document_callback (host-installed, UniFFI-exported)
   ↓  (host's Kotlin/Swift impl serializes Document → JSON, calls peat-ffi)
peat-ffi::publishDocumentJni(collection, json, origin="ble")
   ↓
peat_mesh::Node::publish_with_origin(collection, doc, Some("ble"))
   ↓  (cross-transport fan-out)
iroh / TAK / LoRa
```

The host is the natural bridge because it is the only component that owns handles to **both** `peat_btle::PeatMesh` (via UniFFI) and peat-ffi's `PeatNode` (via JNI). peat-ffi sees only the resulting `publishDocument` call with `origin="ble"`; it does not need to know that the doc came from a translator frame.

#### UniFFI callback export (peat-btle 0.3.1)

peat-btle gains a UniFFI-exported callback trait alongside the existing Rust-only one:

```rust
/// UniFFI-exported counterpart of `crate::DecodedDocumentCallback`. The
/// `peat_mesh::sync::Document` payload is serialized to JSON before
/// invocation so the host (Kotlin / Swift) can forward it through
/// peat-ffi's existing `publishDocument`-shaped FFI without needing a
/// UniFFI binding for `Document` itself. The Rust-side trait remains
/// for in-process Rust consumers (peat-sim direct integration tests,
/// future Rust-native hosts); they receive the typed `Document`.
#[cfg(all(feature = "mesh-translator", feature = "uniffi"))]
#[uniffi::export(callback_interface)]
pub trait DecodedDocumentJsonCallback: Send + Sync {
    /// `collection` is the BleTranslator collection name. `doc_json` is
    /// the serde-JSON serialization of the decoded `Document`. `peer`
    /// is the BLE peer identifier from the receive context.
    fn on_document(&self, collection: String, doc_json: String, peer: Option<String>);
}

#[cfg(all(feature = "mesh-translator", feature = "uniffi"))]
impl PeatMesh {
    pub fn set_decoded_document_json_callback(
        &self,
        cb: Box<dyn DecodedDocumentJsonCallback>,
    );
}
```

The receive dispatch (`try_handle_translator_marker`) gains an additional invocation alongside the existing Rust-trait callback path: when a 0xB6 frame decodes successfully, BOTH the Rust-trait callback (if installed) AND the UniFFI JSON callback (if installed) fire. Each is independent — same as today's `OutboundFrameCallback` Rust-vs-UniFFI parallel paths. The release-skew window (Slice 1.b.3 shipping before 1.b.4 wires the host) still emits `PeatEvent::TranslatorNoCallback` if **neither** callback is installed; if at least one is installed, that branch is suppressed.

#### Host implementation (peat-atak-plugin)

The plugin gains a `BleDecodedDocumentBridge` (or equivalent) class that:

1. Implements `uniffi.peat_btle.DecodedDocumentJsonCallback`.
2. On `on_document(collection, docJson, peer)`, calls `peatNode.publishDocument(collection, docJson, "ble")` — the existing peat-ffi JNI surface, with the `origin="ble"` argument variant.
3. Is registered on `peatBtle.mesh.setDecodedDocumentJsonCallback(this)` at plugin startup, after both the peat-btle `PeatMesh` and the peat-ffi `PeatNode` are constructed.

If peat-ffi's existing `publishDocument` JNI does not yet accept an `origin` parameter, that's a one-line peat-ffi addition (extending an existing method with a new variant or optional arg).

#### Why not restructure peat-ffi to own `PeatMesh`?

Restructuring peat-ffi to own a `peat_btle::PeatMesh` instance — e.g., constructing one inside `PeatNode::new` and exposing a getter — is a possible alternative but rejected because:

- **Doubles the `PeatMesh` lifecycle.** The plugin already constructs and owns one via UniFFI for its existing BLE management (`PeatBleManager.peatBtle.mesh`). A second instance inside peat-ffi would either need to be coordinated with the plugin's instance (lifecycle, identity, peer state) or replace it (which means peat-ffi owns the BLE stack, not the plugin — a much larger architectural change).
- **Breaks the existing FFI/UniFFI boundary.** Today the plugin uses peat-btle's UniFFI for low-level BLE and peat-ffi's JNI for peat-mesh-level operations. Putting peat-btle's `PeatMesh` inside peat-ffi forces peat-ffi to bridge UniFFI types into JNI, which UniFFI is not designed for.
- **Provides no benefit** the plugin-side wiring doesn't. Both paths end at the same `publish_with_origin` call.

### Consequences

#### Positive

- **Implementable against the existing FFI/UniFFI split.** No restructuring of peat-ffi or its BLE adapter chain. Slice 1.b.4 becomes (1) a peat-btle 0.3.1 release adding the UniFFI callback export and (2) a peat-atak-plugin PR registering it.
- **One PeatMesh instance per host.** The plugin's existing `peatBtle.mesh` stays the single owner; the new callback is just one more handler attached to it.
- **Host owns the bridge logic, in line with the existing pattern.** `BleOutboundFrameDispatcher` (Slice 1.c) already lives in the plugin for symmetric reasons — the host is the bridge between peat-btle UniFFI and peat-ffi JNI. Receive-side bridging follows the same shape.

#### Negative

- **Slice 1.b.4 now requires another peat-btle release** (0.3.1) to add the UniFFI callback export. peat-btle 0.3.0 already shipped the Rust-trait variant, so this is purely additive — no breaking change, no operator-rollout sequencing concerns beyond the standard "plugin must run a peat-btle version that ships the UniFFI export."
- **JSON serialization on the receive hot path.** Each decoded `Document` is serialized to JSON before crossing the UniFFI boundary. Mitigation: the existing `OutboundFrameCallback` path already serializes outbound frames similarly, and BLE wire throughput (≤ ~10 frames/sec sustained per ADR-041) is the dominant cost — JSON serialization is in the noise. Future Rust-native hosts (peat-sim direct-integration tests) skip the UniFFI path and use the Rust-trait callback for the typed `Document` directly.
- **Two parallel callback fields on `PeatMesh`** during the 0.3.x line: `decoded_document_callback` (Rust trait) and a new UniFFI-exported one. Same shape as `OutboundFrameCallback`'s parallel paths — accepted as part of UniFFI integration cost. Slice 2.x may consolidate once iOS / wearos-tak-civ confirm they need the JSON form too.

#### Composes with

- **Amendment 1 §"Decoded-document callback" / §"Storage shape and the no-callback window"** — unchanged for the Rust-trait variant. The new UniFFI callback adds a parallel path; the no-callback `PeatEvent::TranslatorNoCallback` event fires only when **both** callbacks are absent.
- **Slice 1.c outbound dispatcher in peat-atak-plugin** — symmetric: outbound is plugin-side (host implements `OutboundFrameCallback`, dispatcher routes bytes to `PeatBleManager.broadcastBytes`), inbound is plugin-side (host implements `DecodedDocumentJsonCallback`, bridge routes JSON to peat-ffi `publishDocument`).
- **#65 / #66 / #69 / #26 unblock conditions** unchanged in spirit: those plugin migrations resume once Slice 1.b.4 ships in a peat-btle release the plugin can pin (0.3.1) and the plugin's `BleDecodedDocumentBridge` is in place.

### Implementation

**Slice 1.b.4 — peat-btle 0.3.1.** Lands in peat-btle behind both `mesh-translator` and `uniffi` features:

1. `peat-btle/src/lib.rs`: add `pub trait DecodedDocumentJsonCallback` with `#[uniffi::export(callback_interface)]`.
2. `peat-btle/src/peat_mesh.rs`: add `decoded_document_json_callback: RwLock<Option<Box<dyn DecodedDocumentJsonCallback>>>` field (gated by both features), `set_decoded_document_json_callback` method, initialize to `None` in all constructors. Extend `try_handle_translator_marker` to invoke the JSON callback alongside the Rust-trait callback when `Ok(Some(doc))`. Suppress `PeatEvent::TranslatorNoCallback` only when **both** callback slots are empty.
3. `peat-btle/src/uniffi_bindings.rs`: expose `set_decoded_document_json_callback` on the UniFFI `PeatMesh` wrapper.
4. CHANGELOG: 0.3.1 entry covering the additive UniFFI export.
5. Release.

**Slice 1.b.4 — peat-atak-plugin.** Lands in peat-atak-plugin once peat-btle 0.3.1 is on crates.io:

1. New `BleDecodedDocumentBridge.kt` implementing `uniffi.peat_btle.DecodedDocumentJsonCallback`. On each `on_document(collection, docJson, peer)`, call `peatNode.publishDocument(collection, docJson, "ble")`.
2. Wire registration into the plugin's startup sequence (`PeatPluginLifecycle` or equivalent), after both `PeatBleManager` and `PeatNode` are constructed.
3. peat-ffi: if `publishDocument` doesn't yet accept an `origin` parameter, add a `publishDocumentWithOrigin(collection, json, origin)` JNI method (one-line wrapper around `Node::publish_with_origin`). Verify before assuming.
4. Integration test: send a 0xB6 frame to peat-btle's `PeatMesh.on_ble_data_received_anonymous`, assert the decoded doc lands in the iroh-side doc store with `origin = Some("ble")`.

**Slice 1.b.5 — peat-ffi RustNative-host wiring (deferred, optional).** If peat-ffi ever owns a `PeatMesh` instance (e.g., for a Rust-only headless build), it can install the Rust-trait `DecodedDocumentCallback` directly. Out of scope for #70 and not required for the plugin migration — the UniFFI path covers every host that exists today.

**Slicing rename.** The original Amendment 1 "Slice 1.b.5 — host-side legacy-emit retirement (deferred)" remains as written — it is the legacy-emit code-path removal once telemetry confirms zero legacy traffic. To avoid number reuse, refer to the optional Rust-native peat-ffi wiring (paragraph above) as **Slice 1.b.6** if it ever ships.

### Alternatives Considered

- **Restructure peat-ffi to own `peat_btle::PeatMesh`.** Rejected: doubles the lifecycle (plugin already owns one), breaks the FFI/UniFFI boundary, provides no behavioral benefit over plugin-side bridging.
- **Expose `peat_mesh::sync::Document` via UniFFI instead of JSON-string callback.** Rejected: requires UniFFI bindings for `peat_mesh::sync::Document`, `peat_mesh::sync::Field`, and the entire schema graph — peat-mesh has no UniFFI dependency today and adding one for a single callback's payload type is disproportionate. JSON-string is what every other peat-ffi `publishDocument`-family method already uses; consistent.
- **Add a peat-ffi `setDecodedDocumentCallback` JNI surface that the plugin calls with an opaque PeatMesh handle.** Rejected: UniFFI handles aren't trivially passed across JNI, and even if they were, peat-ffi would still need to install the callback on a `PeatMesh` it doesn't own — the indirection buys nothing.

### Supersedes

- Amendment 1 §"Decoded-document callback" final paragraph claim that Slice 1.b.4 "Lands in peat-ffi after Slice 1.b.3 ships in a peat-btle release. Installs a `DecodedDocumentCallback` impl that calls `Node::publish_with_origin(collection, doc, Some("ble"))`. Registered at `PeatNode` construction." The wiring lands in the host (peat-atak-plugin and equivalents); the Rust-trait callback in peat-btle 0.3.0 is preserved as an in-process Rust-consumer surface but is not what the plugin uses.
