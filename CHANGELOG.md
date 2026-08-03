# Changelog

All notable changes to the Peat workspace are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog covers the crates published to crates.io from this workspace:

- `peat-protocol` — public facade; depends on `peat-schema` and `peat-mesh`
- `peat-schema` — wire format (Protobuf) definitions

Sub-crates that stay internal (`peat-transport`, `peat-persistence`, `peat-ffi`, `examples/*`) share the workspace version but are not published and are not documented here.

## [Unreleased]

### Fixed

- **FFI peer authentication uses one formation-auth protocol**
  ([#1045](https://github.com/defenseunicorns/peat/issues/1045)) —
  all `peat-protocol` and `peat-ffi` dial and accept paths now use peat-mesh's
  canonical versioned handshake, so two nodes created through the exported FFI
  API authenticate on both sides and synchronize documents.

### Removed

- Remove the incompatible
  `peat_protocol::network::formation_handshake::{perform_initiator_handshake, perform_responder_handshake}`
  API. Custom Iroh transport owners migrate to
  `peat_mesh::storage::{respond_to_formation_auth, accept_formation_auth}`.

### Pinned

- Advance `peat-mesh` to `>=0.9.0-rc.59, <0.9.1` for the public canonical
  acceptor API, and align peat-protocol's test-only Iroh pin to `1.0.2`.
  Default feature selection is unchanged.

## [0.9.0-rc.32] - 2026-08-03

### Fixed

- **scan() skips partially-synced documents** ([#1050](https://github.com/defenseunicorns/peat/pull/1050)) —
  `TypedCollection::scan()` propagated deserialization errors from partially-synced
  documents, aborting the entire scan during incremental CRDT sync on degraded links.
  Undesodable documents are now skipped with a debug trace, matching the pattern in
  `observe()`. (`peat-protocol`)

### Pinned

- `peat-mesh` remains `>=0.9.0-rc.45, <0.9.1`; default feature selection is unchanged.

## [0.9.0-rc.31] - 2026-07-18

### Changed

- **Retire the migrated TAK transport implementation** ([#1026](https://github.com/defenseunicorns/peat/pull/1026)) —
  removes the obsolete transport module after its maintained implementation moved
  to the standalone integration repository. (`peat-transport`)

### Fixed

- **Three-node mesh convergence coverage** ([#1027](https://github.com/defenseunicorns/peat/pull/1027)) —
  re-enables the multi-node convergence test and updates map iteration for the
  Rust 1.97 Clippy baseline. (`peat-protocol`)

### Changed — `peat-ffi`

- **Feature-gated Dart compatibility adapter** ([#1031](https://github.com/defenseunicorns/peat/pull/1031)) —
  `peat-ffi` 0.2.12 keeps the existing adapter enabled by default while allowing
  native wrappers to disable it and provide a version-matched adapter without
  duplicate optimized-link symbols.

### Pinned

- `peat-mesh` remains `>=0.9.0-rc.45, <0.9.1`; default feature selection is unchanged.

## [0.9.0-rc.30] - 2026-07-08

### Added

- **Kinematics and PositionError on Track/NodeState** ([#1023](https://github.com/defenseunicorns/peat/pull/1023)) —
  `peat-schema` adds `Kinematics` and `PositionError` messages to `common.proto`,
  wired into `Track` and `NodeState`. Existing `velocity`, `cep_m`, and
  `vertical_error_m` fields are deprecated in favour of the new structured types.
  `peat-protocol` model constructors updated to include the new fields.
  (peat-schema, peat-protocol)

### Fixed

- **peat-protocol NodeState constructor** — missing `kinematics` and
  `position_error` fields in `NodeState` initializer caused compile failure
  against peat-schema 0.9.0-rc.29. (peat-protocol)

### Changed — `peat-ffi`

- **ADR-074 schema alignment** ([#1022](https://github.com/defenseunicorns/peat/pull/1022)) —
  FFI types shrunk to proto-backed fields only. `peat-ffi` bumped to 0.2.11.

## [0.9.0-rc.29] - 2026-07-05

### Added

- **mDNS-discovered peer dialing (Android)** ([#1007](https://github.com/defenseunicorns/peat/pull/1007)) —
  the Automerge connection manager
  now consumes the peat-controlled `_peat._udp` browse
  (`transport.peat_mdns_events()`) alongside iroh's `MdnsAddressLookup`,
  converts each discovered peer to a dialable `PeerInfo` (via peat-mesh's new
  `From<discovery::PeerInfo>` bridge, peat-mesh#268) and dials its advertised
  concrete addresses with `connect_peer`. Fixes the Android
  "discovered-but-no-dial" gap: iroh swarm-discovery emits nothing on the
  wildcard-bound interface there, so nodes advertised but never connected and
  the peer count stayed at 0. Runs alongside the iroh path on desktop
  (`get_connection` dedup prevents a double-dial); a non-hex node_id is skipped.
  (peat-protocol)

### Added — `peat-ffi`

- **Deterministic formation iroh identity for `createNode`** ([#1006](https://github.com/defenseunicorns/peat/pull/1006)) —
  a canonical
  formation iroh identity is derived for `createNode`, with an explicit
  `node_id` for a stable endpoint identity across restarts. Emits an identity
  base64 fallback log on non-Android too.
- **`bindAddress` threaded through `createNode`** ([#1006](https://github.com/defenseunicorns/peat/pull/1006)) —
  the `createNode` JNI
  entrypoints accept a `bindAddress` so the iroh endpoint binds to the detected
  LAN IP on Android instead of the wildcard interface.
- **mDNS reconnect watchdog** ([#1007](https://github.com/defenseunicorns/peat/pull/1007)) —
  dial-side formation auth now routes through
  peat-mesh's `respond_to_formation_auth`, with a watchdog that re-dials peers
  on mDNS reconnect.
- **UniFFI-exported blob-transfer surface + `blob_fetch_start`** ([#1017](https://github.com/defenseunicorns/peat/pull/1017), peat#1013) —
  `enable_blob_transfer`, `blob_add_peer`, `blob_put`, `blob_exists_locally`,
  `blob_endpoint_id`, `blob_bound_addr` (previously JNI-only) now carry
  `#[uniffi::export]`, plus new `blob_add_peer_id` (wraps peat-mesh's
  `add_peer_from_hex_id`) and the async, poll-based
  `blob_fetch_start(hash, size, peer_id: Option<String>)` ->
  `Arc<BlobFetchHandle>`. `peer_id = None` runs the mesh-sync `fetch_blob`
  path; `Some(id)` runs peat-mesh's new direct-peer `fetch_blob_from_peer`
  path — one entrypoint, both delivery modes. `BlobFetchHandle::dispose()`
  aborts the underlying task for real mid-transfer cancellation.
  `enable_blob_transfer`'s `bind_addr` param changes from
  `Option<SocketAddr>` to `Option<String>` (`SocketAddr` isn't
  UniFFI-liftable); the JNI wrapper now passes the raw string through
  instead of pre-parsing, so a malformed non-empty bind address surfaces as
  an error instead of silently falling back to an ephemeral bind.

### Pinned

- `peat-mesh` floor raised to `0.9.0-rc.45` (peat-mesh#274 —
  `NetworkedIrohBlobStore::fetch_blob_from_peer`, the direct-peer blob-fetch
  path `blob_fetch_start` consumes). Default features unchanged
  (`peat-mesh` continues to build with `default-features = false`, per-crate
  opt-in).

## [0.9.0-rc.28] - 2026-06-24

### Added — `peat-ffi`

- **Persistent peer roster** (#1000) — `RosterStore` (JSON on disk) tracks known
  group members across restarts. Exposed via six new UniFFI surface methods:
  `roster_remember`, `roster_upsert`, `roster_remove`, `roster_get`,
  `roster_list`, `roster_list_by_group`. Also adds the `RosterEntry` UniFFI
  Record. Stored as plain JSON (non-secret reachability hints only; no FIPS
  concern).
- **Per-peer reconnect supervisor** (#1000) — dial state machine
  (`Idle → Connecting → Connected → Backoff`) with exponential backoff (2 s
  base, 5 min cap) plus deterministic per-peer jitter to de-correlate
  thundering-herd re-dials. Three new UniFFI surface methods:
  `reconnect_known_peers` (gentle, honours backoff), `wake_reconnect` (clears
  backoffs — call on foreground / network-up events), `on_peer_observed` (hint
  that a specific roster member is reachable now). Concurrent in-flight dials
  are bounded at `MAX_CONCURRENT_RECONNECT_DIALS = 8`.
- **Cross-transport reconnect dedup** (#1000) — the supervisor's connected set
  is the union of iroh peers and any roster member with a live link on any
  transport (BLE, etc.), so a peer already reachable over BLE is not also dialed
  over iroh/relay.
- **Origin-tagged `DocumentChange`** (#1000) — new required field
  `origin: ChangeOrigin` on the UniFFI `DocumentChange` Record. `ChangeOrigin`
  is a new UniFFI enum with variants `Local` (own publish) and
  `Remote { peer_id }` (sync from a peer). Enables consumers to notify only on
  remote changes. **Coordinated binding regen required** — paired with
  peat-flutter#13.
- **Four Dart C-ABI shims** (#1000) — hand-rolled `#[no_mangle] extern "C"`
  functions reformatting flat `FFIBuffer` arrays for `roster_remember`,
  `reconnect_known_peers`, `wake_reconnect`, and `on_peer_observed`. Sit
  alongside the existing shims in `dart_ffi.rs`.
- **Contributor workflow guardrails** (#1000) — `CLAUDE.md` and `SKILL.md` now
  require fork contributors to sync to upstream `main` before opening a PR, run
  the verification checklist, and pass the consumer-reference grep gate.

### Pinned

- `peat-mesh` `>=0.9.0-rc.43, <0.9.1` (unchanged from rc.27).

## [0.9.0-rc.27] - 2026-06-20

### Fixed — `peat-protocol`

- **`relay-n0-hosted` feature now forwards to `peat-mesh`** (#995). After the
  ADR-062 relocation moved `IrohTransport` endpoint construction (`presets::Empty`
  vs `presets::N0`) into peat-mesh, `peat-protocol`'s `relay-n0-hosted` feature
  was left an orphaned no-op (`= []`) — enabling it had no effect on the relay
  posture. It now forwards to `peat-mesh/relay-n0-hosted`, so the documented
  "flips every `IrohTransport` constructor to the n0-hosted preset" behavior
  takes effect again. Still OFF by default (tactical/edge builds must not phone
  home through n0 infrastructure).

### Added — `peat-ffi`

- **Non-blocking `PeatNode::connect_peer_nowait`** (#995) — a fire-and-forget
  variant of `connect_peer` that spawns the dial + formation handshake + sync
  trigger on the runtime and returns immediately, so UI consumers don't freeze
  on the synchronous FFI call for the full dial duration. Background failures
  are surfaced via `tracing` on every platform. Ships in the Maven AAR 0.1.3 cut.
- **`relay-n0-hosted` feature passthrough** (#995) — forwards to the
  `peat-mesh`/`peat-protocol` feature of the same name. OFF by default.

### Pinned

- `peat-mesh` `>=0.9.0-rc.43, <0.9.1` (unchanged from rc.26).

## [0.9.0-rc.26] - 2026-06-19

### Changed — `peat-protocol`

- **Distribution / file-transfer implementation relocated to peat-mesh**
  (#993, peat#992). `peat-protocol::storage::{file_distribution,
  model_distribution}` now re-export from `peat_mesh::storage::…` — peat-mesh is
  the canonical iroh consumer (ADR-060 §5), so the transport-specific impl no
  longer lives in the protocol layer. Public surface unchanged. Requires
  peat-mesh rc.43. (peat-protocol's direct `iroh` dep is not yet dropped —
  `network`/`transport`/`mesh_sync_transport` still use it.)

### Added — `peat-protocol`

- **ADR-071: subscription-based convergence seam** (#991). Receiver-evaluated,
  sender-ignorant "need" (the `NeedEvaluator` abstraction, `collection` on the
  distribution document, the `should_deliver`/`can_skip_permanently` gate),
  opt-in via `IrohFileDistribution::with_need_evaluator` (default preserves
  directed `target_nodes` delivery).

### Added — `peat-ffi`

- BLE-relay sync surface: CRDT-KV + Automerge counter + node-layer publish
  (#983), runtime n0-relay toggle on `TransportConfigFFI` (#989), and
  build/packaging fixes (#985, #987, #988). Workspace `peat-mesh` floor raised
  to rc.42 (#990).

## [0.9.0-rc.24] - 2026-06-07

**Proto package namespace `cap.*` → `peat.*` ([ADR-069](docs/adr/069-proto-package-namespace-peat.md), [peat#972](https://github.com/defenseunicorns/peat/issues/972)).** All 20 schema protos move from `package cap.<domain>.v1;` (a fossil of the project's original academic name, Capability Aggregation Protocol) to `package peat.<domain>.v1;`, matching the project name and the existing `peat.sidecar.v1` service namespace. The `peat-schema` Rust module tree is flattened, so the canonical path is `peat_schema::<domain>::v1::<Type>` (the pre-existing flat re-export means consumer imports are **unchanged**). **Breaking** only at the descriptor / reflection / fully-qualified-name layer (`cap.node.v1.Node` → `peat.node.v1.Node`) — affects buf, grpcurl reflection, and any FQN-keyed registry. **Not** breaking: proto3 binary wire, persisted Automerge/postcard documents (the package is not encoded in messages), gRPC method paths (these protos define messages only), or proto3 JSON keys. Pre-1.0 clean break, consistent with the ADR-066/068 precedent. The `spec/proto/` reference tree + `draft-peat-protocol-*.md` update is a deferred spec-doc follow-up.

## [0.9.0-rc.23] - 2026-06-06

**ADR-068 Node base-unit vocabulary, Phase 1 ([peat#969](https://github.com/defenseunicorns/peat/pull/969), epic [peat#968](https://github.com/defenseunicorns/peat/issues/968)).** The schema and protocol converge on **Node** as the single name for the base mesh participant, eliminating the overloaded "Platform" ([ADR-068](docs/adr/068-node-base-unit-vocabulary.md), which amends ADR-066's Platform row). Renames across the wire surface: `platform_id(s)` → `node_id(s)`, `platform_type` → `node_type`, `target_platform(s)` → `target_node(s)`, `source_platform(s)` → `source_node(s)`, `DEVICE_TYPE_SENSOR_PLATFORM` → `DEVICE_TYPE_SENSOR_NODE`, and the FFI/storage surface (`PlatformInfo`/`PlatformStatus` → `NodeInfo`/`NodeStatus`, collection `"platforms"` → `"nodes"`). **Wire-breaking** (proto field *numbers* are preserved, so the binary format stays tag-compatible; field *names*, JSON keys, and the collection name break) — a pre-1.0 clean break consistent with the ADR-066 precedent in rc.22. The role-label prose "Strike/Support platform" and OS/hardware "platform" contexts are intentionally preserved as a class-of-vehicle / computing-platform carve-out. peat-mesh adopted the matching internal `HierarchyLevel::Platform` → `Node` rename in `0.9.0-rc.35` (Phase 3). Downstream lockstep: peat-node Phase 4 renames its sidecar `Platform`/`PutPlatform`/`GetPlatforms` surface and the `"platforms"` collection to match; peat-atak-plugin (Phase 5) must regenerate its FFI bindings.

## [0.9.0-rc.22] - 2026-06-02

**ADR-066 hierarchy vocabulary rename ([peat#957](https://github.com/defenseunicorns/peat/pull/957), peat#904 Phases 1+2).** The schema and protocol adopt the abstract, non-military hierarchy vocabulary defined in [ADR-066](docs/adr/066-hierarchy-vocabulary.md): the four aggregation tiers are now **Cell → Cohort → Federation → Coalition** (replacing Squad/Platoon/Company and adding a fourth top tier). This is a **wire-breaking change** to the renamed proto fields, messages, and enum values — peers and persisted documents produced before rc.22 are not compatible across the rename. peat-mesh adopted the matching internal `HierarchyLevel` rename + Coalition tier in `0.9.0-rc.31` (Phase 3).

### Changed — `peat-schema` (wire format)

- **`cell.proto`:** `Cell.platoon_id` (field 5) renamed to `Cell.cohort_id`. Same field number, same `optional string` type and LWW-Register semantics — only the name changed.
- **Hierarchy summary messages renamed:** `SquadSummary` → `CellSummary`, `PlatoonSummary` → `CohortSummary`, `CompanySummary` → `FederationSummary`, plus a new `CoalitionSummary` for the fourth tier. Their leaf identifier fields rename accordingly (`squad_id` → `cell_id`, `platoon_id` → `cohort_id`).
- **`command.proto` `CommandScope` enum:** `SQUAD = 2` → `CELL = 2`, `PLATOON = 3` → `COHORT = 3`, added `FEDERATION = 5` and `COALITION = 6`. `target_ids` now carries node/cell/cohort/federation/coalition ids.
- **Type registry / ontology:** `TypeDescriptor` field metadata and the ontology concepts updated (`platoon`/`company` → `cohort`/`federation`/`coalition`); type IDs (`peat.cell.v1.*`, `peat.node.v1.*`) and hyphenated collection names are unchanged.

### Changed — `peat-protocol`

- Internal coordinator, leader-election, routing, discovery, hierarchy-aggregation, and command types renamed `squad_id` → `cell_id` and the analogous tier fields throughout, tracking the schema rename. No new public capability beyond the rename.
- **`automerge` dependency bumped `0.7.1` → `0.9.0`** to stay ABI-compatible with peat-mesh `0.9.0-rc.31` (which moved to automerge 0.9). Required because `peat-protocol`'s `automerge-backend` feature shares the `Automerge` type with peat-mesh's storage layer across the re-export boundary; a version split produces `mismatched types` against every `Automerge`-typed call site. No `peat-protocol` source changes were needed beyond the bump — the 0.9 read/write API surface peat-protocol uses (`ReadDoc`/`Transactable` `get`/`put`/`put_object`) is source-compatible.

### Migration

Consumers that round-trip the renamed fields must update to the new names (e.g. `platoon_id` → `cohort_id` on `CellState`). Because field numbers are preserved, Protobuf binary payloads decode unchanged; JSON/named-field access and any hardcoded `CommandScope` enum names must be updated.

## [0.9.0-rc.21] - 2026-05-30

**ADR-065 auth-handshake version + capability negotiation ([peat#952](https://github.com/defenseunicorns/peat/pull/952)) + descriptor-driven proto3-zero JSON on `TypeDescriptor` ([peat#954](https://github.com/defenseunicorns/peat/pull/954), closes [peat#953](https://github.com/defenseunicorns/peat/issues/953)).** rc.21 fixes the long-standing authenticator timestamp-mismatch bug that produced ~10⁻⁶ flaky auth failures across the wall-clock-second boundary, and at the same time anchors the wire-format so future protocol revisions can roll across a heterogeneous mesh without producing the same hard rollout cliff this release carries (see the operational note below). Independently, `TypeDescriptor` now drives proto3-zero JSON per registered type — unblocking consumer-side proto3 defaulting (peat-cli, future SDKs) without the per-collection hardcoded table.

### Added — ADR-065 auth-handshake version + capability negotiation

rc.21 ships [ADR-065](docs/adr/065-auth-handshake-version-negotiation.md) — a version token + advertise-only capability set in the challenge-response handshake. Future protocol revisions can now ship to a heterogeneous mesh without producing the same hard rollout cliff this rc.21 release does (see below).

- **`Challenge.protocol_version: uint32`** (proto field `5`) and **`Challenge.capabilities: repeated string`** (proto field `6`) — the challenger advertises which protocol version it speaks and which capability strings it can offer.
- **`SignedChallengeResponse.protocol_version: uint32`** (proto field `7`) and **`SignedChallengeResponse.capabilities: repeated string`** (proto field `8`) — the responder sets `protocol_version = min(challenge.protocol_version, CURRENT_PROTOCOL_VERSION)`, signs the byte construction for the negotiated version, and writes `protocol_version` so the verifier knows which construction to reconstruct.
- **Per-version signed-message byte construction.** v0 (pre-rc.21 peers; field absent on the wire and reads as `0`): `nonce || challenger_id || response.timestamp.seconds`. v1 (rc.21+): same prefix `|| response.protocol_version` (u32 little-endian, 4 bytes). The v1 suffix binds the negotiated version into the signature so a MITM cannot strip the field or rewrite it across protocol bumps.
- **`peat_protocol::security::CURRENT_PROTOCOL_VERSION: u32 = 1`** and **`peat_protocol::security::INCOMPATIBLE_PROTOCOL_VERSION_PREFIX: &str`** are re-exported as stable consumer-facing API surface. Consumers can match against the prefix on a `SecurityError::AuthenticationFailed` to distinguish version-incompatibility from a genuine signature/forgery failure; a typed `SecurityError::IncompatibleProtocolVersion` variant is queued as a peat-mesh follow-up.
- **v1 capability semantics are advertise-only.** `Challenge.capabilities` and `SignedChallengeResponse.capabilities` carry sorted opaque ASCII identifiers consumers can read for soft-policy feature flagging. The capabilities are not part of the v1 signed bytes; ADR-065 §"Capabilities (v1 semantics)" tracks the v2 path for binding them.

### Added — `TypeDescriptor::proto3_zero()` descriptor-driven proto3-zero JSON

[peat#954](https://github.com/defenseunicorns/peat/pull/954), closes [peat#953](https://github.com/defenseunicorns/peat/issues/953). `peat_schema::type_registry::TypeDescriptor` gains a `proto3_zero()` method that returns the canonical proto3 wire-zero `serde_json::Value` for the registered type, driven by the prost-generated `Default` impl + the existing `serde::Serialize` derive. Replaces the per-collection hardcoded defaults table downstream consumers (peat-cli, future SDKs) previously had to maintain.

- **`peat_schema::type_registry::Proto3ZeroFn`** type alias (`fn() -> Value`) and **`peat_schema::type_registry::default_proto3_zero`** fallback (returns `{}`). Public surface — external consumers building descriptors via `TypeDescriptor::new()` get the safe empty-object fallback under the merge-on-top-of-defaults pattern.
- **`TypeDescriptor::proto3_zero_fn: Proto3ZeroFn`** field + **`TypeDescriptor::proto3_zero(&self) -> Value`** accessor. Additive on `TypeDescriptor` — `TypeDescriptor::new(...)` defaults `proto3_zero_fn` to `default_proto3_zero`, so external crates that construct descriptors via the public `new()` API continue to compile unchanged.
- **All five `descriptors::*()` builders** (Capability, NodeConfig, NodeState, CellConfig, CellState) wire `proto3_zero_fn` to a local `fn` calling `serde_json::to_value(<MessageType>::default())`. The zero shape IS the serialised default of the prost-generated struct — cannot drift from the proto3 field list. Resolves the `FieldFormat::JsonString` ambiguity (real strings default to `""`, optional messages default to `null`) per-type rather than per-format-hint.
- **Four regression tests** pin the surface: universal JSON-object shape across every registered descriptor, deserialise-cleanly round-trip via `desc.validate_json` (the round-trip property the API exists to provide), spot-check on the Capability descriptor that the zero matches the prost `Default` field-by-field, and the public `new()` fallback returning `{}`.

Downstream-consumer migration (separate PR in peat-node) replaces `peat-cli`'s ~50-line hardcoded `proto3_defaults_for(collection)` table with a single `desc.proto3_zero()` lookup driven by the registry. Sibling: [peat-node#112](https://github.com/defenseunicorns/peat-node/issues/112).

### Operational note — auth wire-incompatibility with pre-rc.21 peers

The rc.21 release also carries [peat#952](https://github.com/defenseunicorns/peat/pull/952)'s auth-timestamp fix, which changes which timestamp is covered by the Ed25519 signature in the challenge-response handshake: signer and verifier now use `response.timestamp.seconds` instead of the pre-rc.21 mix where the signer used `challenge.timestamp.seconds` and the verifier used `response.timestamp.seconds` (the bug rc.21 fixes).

**Consequence:** pre-rc.21 nodes and post-rc.21 nodes cannot authenticate with each other across this one cliff. A mesh that rolls upgrades incrementally rather than simultaneously will see mutual-auth failures between the old-code and new-code segments of the mesh for the duration of the rollout window.

**This is the LAST staggered-upgrade cliff in the auth path.** ADR-065's version negotiation makes future protocol revisions (rc.21 → rc.22+ and beyond) interoperable across mixed-version meshes: a v1 peer talking to a future v2 peer will negotiate down to v1, and a future v2 peer will know — from the signed `response.protocol_version` byte — which byte construction the v1 peer used. The rc.20 → rc.21 cliff exists because pre-rc.21 wire format has no version field at all (it reads as `0` via the prost default, but the v0 byte construction is the new, fixed construction; pre-rc.21 nodes don't know to send it). After rc.21, this class of operational risk is eliminated.

**Operator action:** upgrade all nodes in a mesh together for the rc.20 → rc.21 boundary. The pre-rc.21 protocol was already producing flaky auth failures at a ~10⁻⁶-per-attempt rate from this same bug; this is therefore not a regression from a stable baseline — but it IS a hard incompatibility that needs the deploy to be coordinated this one time.

Spec reference: `docs/spec/005-security.md` §5.3, `docs/whitepaper/10b-spec-appendix.md` §5.3, `peat-schema/proto/security.proto::SignedChallengeResponse.signature` doc-comment, [ADR-065](docs/adr/065-auth-handshake-version-negotiation.md).

## [0.9.0-rc.20] - 2026-05-30

**peat-mesh floor advance to rc.29 — `subscribe_to_observer_changes` now fires on tombstone-driven deletes.** Single-fix peat-mesh release ([peat-mesh#203](https://github.com/defenseunicorns/peat-mesh/pull/203)) closes [peat-mesh#202](https://github.com/defenseunicorns/peat-mesh/issues/202). Pre-rc.29 the CDC channel was insert/update-complete but delete-blind from any peer; rc.29 closes the documented "fires for ALL document changes" contract by routing the receive-side `apply_tombstone` through `delete_with_origin(.., Remote(peer))` and updating local `AutomergeStore::delete` to fire the same broadcast pipeline as `put`. peat-cli's `peat observe`-with-`peat delete` cross-CLI flow (peat-node ADR-001 Open Question §7) is unblocked.

### Changed

- **`peat-mesh` workspace floor** advanced from `>=0.9.0-rc.28, <0.9.1` to `>=0.9.0-rc.29, <0.9.1`. Spans peat-mesh rc.29's full surface change:
  - **`AutomergeStore::delete` broadcast contract** ([peat-mesh#203](https://github.com/defenseunicorns/peat-mesh/pull/203), closes [peat-mesh#202](https://github.com/defenseunicorns/peat-mesh/issues/202)). `delete(key)` now fires `observer_tx` (CDC) + `change_tx` (local sync-out trigger) + `gossip_tx` (origin-tagged), matching the existing `put` / `put_with_origin` channel-gating matrix. A CDC consumer subscribed to `subscribe_to_observer_changes` now sees the delete via `store.get(key)` returning `Ok(None)` on the event — the "Some → insert/update, None → delete" detection pattern peat-cli's `cli/observe.rs:94-102` already implements.
  - **New `AutomergeStore::delete_with_origin(key, ChangeOrigin)`**. Mirrors `put_with_origin`: `Remote(peer)` suppresses `change_tx` per the peat-mesh#115 ping-pong invariant while observer/gossip fire with peer attribution.
  - **`AutomergeSyncCoordinator::apply_tombstone`** routes the post-tombstone document removal through `delete_with_origin(.., Remote(peer))` — CDC consumers now see remote-driven deletes; transitive-gossip drivers see the peer attribution.

  No peat-side code change required to consume — the workspace continues to compile against rc.28's surface (the rc.29 changes are additive to `AutomergeStore::delete`'s broadcast behaviour and additive on `delete_with_origin`). The floor advance forces downstream peat consumers (peat-node, peat-sim) to pick up the corrected CDC contract on `cargo update`.

### Behavioural delta worth noting for operators

- **TTL eviction** now wakes the sync-coordinator's local-only outbound pusher per evicted document, because each `store.delete` call fires `change_tx`. The doc is already gone by the time the pusher runs (nothing to push) — the side-effect is the wakeup, not propagation. peat-mesh#203 added inline comments at both `ttl_manager.rs` call sites naming this explicitly. Consumers tracking a wakeup rate that aligns with TTL expiry cadence will find the in-place explanation.

### Impact on peat workspace consumers

- **peat-cli `peat observe`** (peat-node): unblocked. The `cli/observe.rs` workaround "render deletes only on locally-observed races" can come out; the channel fires on cross-peer tombstone propagation now.
- **peat-sim:** picks up the corrected CDC contract transitively. No source change required.
- **peat-ffi:** unchanged — no JNI / UniFFI surface additions in this bump.

### Cross-repo follow-up

- **peat-node** pins peat-mesh `=0.9.0-rc.27` (exact). Two-step bump pending: first to `=0.9.0-rc.28` (the rc.28 delta/Lamport/persistence/wire-up surface), then to `=0.9.0-rc.29` (the rc.29 observer-fires-on-delete contract fix). Both can be a single PR if the consumer-side migration is concurrent. Separate.
- **peat-sim** pins peat-mesh `=0.9.0-rc.26` + peat-protocol `=0.9.0-rc.17` (both exact). Both pins need advancing to consume the rc.28-rc.29 train + the peat rc.18-rc.20 train. Separate.

## [0.9.0-rc.19] - 2026-05-29

**peat-mesh floor advance to rc.28 — `peat-cli` round-trip-edit + tombstone-authorship unblocker.** Rolls the peat-mesh trail forward to incorporate four landed PRs ([peat-mesh#193](https://github.com/defenseunicorns/peat-mesh/pull/193), [peat-mesh#194](https://github.com/defenseunicorns/peat-mesh/pull/194), [peat-mesh#197](https://github.com/defenseunicorns/peat-mesh/pull/197), [peat-mesh#198](https://github.com/defenseunicorns/peat-mesh/pull/198)) that close [peat-mesh#187](https://github.com/defenseunicorns/peat-mesh/issues/187), [peat-mesh#192](https://github.com/defenseunicorns/peat-mesh/issues/192), [peat-mesh#195](https://github.com/defenseunicorns/peat-mesh/issues/195), and [peat-mesh#196](https://github.com/defenseunicorns/peat-mesh/issues/196). Adds the Automerge delta primitive + node-local Lamport clock + cross-restart persistence + sync-receive wire-up that `peat-cli` (peat-node) needs for the round-trip-edit pattern (`peat update --from <PATH>`) and tombstone authorship (`peat delete`).

### Changed

- **`peat-mesh` workspace floor** advanced from `>=0.9.0-rc.27, <0.9.1` to `>=0.9.0-rc.28, <0.9.1`. Spans peat-mesh rc.28's full surface addition:
  - **`AutomergeStore::diff` / `apply_delta` / `apply_delta_with_origin`** + **`AutomergeDelta`** value type with `to_bytes` / `from_bytes` framing. Surfaces Automerge's delta primitive at the storage layer so consumers can apply minimal changes to a stored document without recreating it. Preserves ADR-021's "create once, evolve through deltas" invariant.
  - **`AutomergeBackend::next_lamport` / `current_lamport` / `observe_lamport`** — node-local Lamport clock for consumer-authored operations. Replaces the prior `peat-cli delete` wall-clock proxy at the right surface with strict per-node monotonicity under concurrent writes. `u64::MAX - 1` cap inside `observe_atomic` prevents hostile/buggy peer Lamports from saturating the atomic and wrapping `next_lamport` to 0.
  - **`AutomergeStore::read_lamport_highwater` / `write_lamport_highwater`** + **`AutomergeBackend::flush_lamport_highwater`** — cross-restart Lamport persistence via a new `metadata` redb table with monotonic-write semantics; periodic + clean-shutdown flush paths. Resumed seed is `max(persisted_highwater, SystemTime nanos, 1)` — resists wall-clock regression on tactical hardware (battery-less RTC, intermittent NTP, manual time-set).
  - **`AutomergeBackend::shutdown_and_release`** — async teardown awaiting iroh Router shutdown, idempotent via `AtomicBool` CAS guard. Enables deterministic same-process drop-and-reopen for hot-restart flows (config reload, daemon-rotate, mobile pause/resume) and integration tests. Previously the iroh Router's background I/O briefly outlived synchronous `Drop` and held the redb file lock past return.
  - **`AutomergeSyncCoordinator::set_lamport_clock`** + automatic receive-side Lamport observation — inbound `Tombstone.lamport` values from `handle_incoming_tombstone` / `handle_incoming_tombstone_batch` flow through the node-local clock automatically (batch path observes the max in a single CAS). Completes the cross-node half of Lamport partial-order semantics. Opt-in for standalone coordinator consumers.

  No peat-side code change required to consume — the workspace continues to compile against rc.27's surface (the rc.28 additions are purely additive). The floor advance forces downstream peat consumers (peat-node, peat-sim) to pick up the new surface on `cargo update`.

### Impact on peat workspace consumers

- **peat-cli (peat-node):** the motivating consumer. `peat update --from <PATH>` round-trip-edit and `peat delete` tombstone authorship can now drop their interim workarounds and consume the peat-mesh APIs directly:
  - `peat update --from <PATH>` uses `AutomergeStore::diff(current, proposed)` + `apply_delta(key, &delta)` instead of full-document replacement via `put()`.
  - `peat delete` uses `AutomergeBackend::next_lamport()` for tombstone authorship instead of `SystemTime::now()` nanos as a Lamport proxy. Inbound tombstone Lamports flow through `observe_lamport` automatically via the coordinator wire-up.
- **peat-sim:** picks up the persistence + receive-side wire-up transitively. No source change required; the perf/correctness wins ride along on `cargo update`.
- **peat-ffi:** unchanged — no JNI / UniFFI surface additions in this bump.

### Cross-repo follow-up

- **peat-node** pins peat-mesh `=0.9.0-rc.27` (exact). Needs a fresh bump PR to advance to `=0.9.0-rc.28` (or `>=0.9.0-rc.28, <0.9.1`) to consume the new surface in source. Separate.
- **peat-sim** pins peat-mesh `=0.9.0-rc.26` + peat-protocol `=0.9.0-rc.17` (both exact). Both pins need advancing for peat-sim to consume the rc.28 + rc.18 train. Separate.

## [0.9.0-rc.18] - 2026-05-29

**Real `CapabilityMatcher` for `DeploymentDirective` Capability-scope targeting (closes [peat#773](https://github.com/defenseunicorns/peat/issues/773)) + peat-mesh floor advance to rc.27.** Closes the last open p0-blocker in the peat workspace and rolls the peat-mesh trail forward to incorporate the peat-mesh#175 closure follow-throughs (rc.26 in-CI UAT, rc.27 `DocumentStore::get` keyed-lookup overrides on both backends).

### Added

- **`peat_protocol::distribution::CapabilityMatcher`** ([peat#942](https://github.com/defenseunicorns/peat/pull/942), closes [peat#773](https://github.com/defenseunicorns/peat/issues/773)). Stateless evaluator that returns `true` iff every constraint in a `CapabilityFilter` is satisfied by the candidate platform's `CapabilityAdvertisement`. All-must-match semantics. Replaces the pre-#773 placeholder in `DeploymentDirective::targets_node` (`scope::Capability` arm) that returned `true` whenever `required_capabilities` happened to be empty — including filters that set only hardware bounds or custom k/v constraints (`Capability { min_gpu_memory_mb: Some(8_192) }` was admitted by every node regardless of GPU pre-#773).
  - Evaluates **hardware bounds** (`min_gpu_memory_mb`, `min_memory_mb`, `min_storage_mb`) against the new `HardwareSpec` payload. Missing fields are a non-match (conservative; rationale on the `HardwareSpec` doc-comment).
  - Evaluates **custom k/v constraints** against `HardwareSpec.custom`.
  - Evaluates **required capability strings** by direct case-insensitive comparison against `CapabilityInfo.capability_type` and via a SensorType vocabulary bridge (a filter `ELECTRO_OPTICAL` matches an advert advertising the canonical `EO` code).
- **`peat_protocol::cot::HardwareSpec`** ([peat#942](https://github.com/defenseunicorns/peat/pull/942)). Optional hardware-introspection payload attached to `CapabilityAdvertisement` via a new `hardware: Option<HardwareSpec>` field. Backwards-compatible: missing field serialises and deserialises as `None`.
- **`DeploymentDirective::targets(adv)`** — full-evaluation alternative to the ID-only `targets_node(node_id)`. Identity, formation membership (with documented fall-through; see ADR-064), and capability matching via `CapabilityMatcher`. Callers with an `CapabilityAdvertisement` should switch from `targets_node` to `targets`.
- **`CapabilityFilter::is_unconstrained()`** — accessor used by `targets_node` to admit the trivial no-constraint case without evaluating the filter.
- **ADR-064 — Deployment Formation Fall-Through for Unassigned Platforms** ([docs/adr/064-deployment-formation-fallthrough.md](docs/adr/064-deployment-formation-fallthrough.md)). Records the optimistic posture for `targets(adv)` under `DeploymentScope::Formation(fid)` when `adv.formation_id` is `None` — falls through to the directive's issuer formation. The reviewer of [peat#942](https://github.com/defenseunicorns/peat/pull/942) flagged the policy as ADR-territory; ADR-064 anchors the decision with full rationale, three alternatives considered (conservative `None` → non-match, cell-level constraint, explicit transitional flag), implications, and a status-flip gate on lab validation.

### Changed

- **`DeploymentDirective::targets_node`** under `DeploymentScope::Capability` is now conservative — returns `true` only if `filter.is_unconstrained()`. Pre-#773 the arm vacuously admitted filters that set only hardware bounds or custom constraints. Callers that need to evaluate those constraints should switch to `targets(adv)`. Behaviour for `Broadcast`, `Formation`, and `Nodes` scopes is unchanged.
- **`peat-mesh` workspace floor** advanced from `>=0.9.0-rc.25, <0.9.1` to `>=0.9.0-rc.27, <0.9.1`. Spans:
  - **peat-mesh rc.26** ([peat-mesh#184](https://github.com/defenseunicorns/peat-mesh/pull/184)) — in-CI behavioural UAT for peat-mesh#175 delivery-ratio thresholds. Test-only; no API surface change.
  - **peat-mesh rc.27** ([peat-mesh#188](https://github.com/defenseunicorns/peat-mesh/pull/188), [peat-mesh#189](https://github.com/defenseunicorns/peat-mesh/pull/189), [peat-mesh#190](https://github.com/defenseunicorns/peat-mesh/pull/190)) — `AutomergeBackend::get` and `InMemoryBackend::get` keyed-lookup overrides (O(N)→O(1); closes [peat-mesh#186](https://github.com/defenseunicorns/peat-mesh/issues/186)). UAT file-header doc-comment scope clarification. Deletion semantics on both overrides match the trait-default exactly.

  Pure perf improvement on the underlying mesh substrate — no peat-side adaptation required; the bumped floor forces downstream consumers to pick up the perf wins on `cargo update`.
- **`peat-protocol` internal `peat-schema` exact pin** advanced from `=0.9.0-rc.17` to `=0.9.0-rc.18` to match the workspace version.

### Impact on peat workspace consumers

- **peat-node, peat-sim:** can now consume the matcher by bumping their `peat-protocol` exact pin to `=0.9.0-rc.18`. Separate bump PRs needed in each repo; peat-node bump unlocks downstream peat-sim re-validation. peat-mesh rc.27's `DocumentStore::get` perf overrides ride along transitively.
- **peat-ffi** stays at 0.2.5 — no JNI / UniFFI surface additions in this bump.

### Cross-repo follow-up

- **peat-node** pins peat-mesh `=0.9.0-rc.27` (exact) + peat-protocol `>=0.9.0-rc.17`. The peat-protocol range admits rc.18 transitively, but a fresh bump PR is needed to pick up the matcher in source. Separate.
- **peat-sim** pins peat-mesh `=0.9.0-rc.26` + peat-protocol `=0.9.0-rc.17` (both exact). Both pins need advancing for peat-sim to consume rc.18. Separate.

## [0.9.0-rc.17] - 2026-05-27

**Bump peat-mesh floor to 0.9.0-rc.25** — picks up the ADR-063 persistent multiplexed sync streams landed in [peat-mesh#176](https://github.com/defenseunicorns/peat-mesh/pull/176), [peat-mesh#178](https://github.com/defenseunicorns/peat-mesh/pull/178), and [peat-mesh#180](https://github.com/defenseunicorns/peat-mesh/pull/180), closing [peat-mesh#175](https://github.com/defenseunicorns/peat-mesh/issues/175). Architecture decision in [ADR-063](docs/adr/063-persistent-sync-streams.md) (merged via peat#936).

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.16` → `0.9.0-rc.17`. `peat-ffi` stays at `0.2.5` — no JNI / UniFFI surface additions; the peat-mesh wire-up fix is entirely behind `AutomergeBackend::new`'s existing API surface.

### Changed

- **`peat-mesh` floor**: `>=0.9.0-rc.24, <0.9.1` → `>=0.9.0-rc.25, <0.9.1`. Before rc.25 every consumer of `AutomergeBackend::new` (peat-protocol, peat-node, peat-sim) ran the legacy per-message-stream fallback because the `Arc<SyncChannelManager>` was dropped at construction scope exit and the coordinator's `Weak` dangled. rc.25 fixes the wire-up — `AutomergeBackend` now owns the strong reference as a field — and adds the writer-task + bounded-mpsc send path that closes the per-peer mutex contention surfaced by the rc.25 lab UAT.

### Consumer-visible behaviour change

peat-protocol's `AutomergeIrohBackend` consumers (`peat_protocol::sync::automerge::*`) automatically take the persistent multiplexed path post-bump — no API change at the peat-protocol surface. The observable effect is delivery convergence through sustained-write workloads where rc.24's legacy fallback cliff'd. peat-mesh#175 UAT (`sweep-telemetry-rate.sh` on the shaped 256 kbps / 100 ms link, platform-3-only emitter, 3 trials per rate):

| Rate  | rc.24 (legacy fallback) | rc.25 (persistent path) | peat-mesh#175 threshold |
|-------|-------------------------|-------------------------|-------------------------|
| 1 Hz  | 100% / 100%             | 100% / 100%             | ≥ 99.5% ✓               |
| 10 Hz | 94.6% / 94.6%           | 98.9% / 100%            | ≥ 99.0% ✓               |
| 25 Hz | 85.3% / 85.5%           | 98.8% / 100%            | ≥ 95.0% ✓               |

### Cross-repo follow-up

- **peat-node**: pins `peat-mesh = "=0.9.0-rc.24"` (exact) + `peat-protocol = ">=0.9.0-rc.16, <0.9.1"`. Needs a separate bump PR to `=0.9.0-rc.25` and `>=0.9.0-rc.17` respectively, then release `0.3.5`.
- **peat-sim**: pins `peat-mesh = "=0.9.0-rc.24"` (exact) + `peat-protocol = "=0.9.0-rc.16"` (exact). Needs both exact pins bumped; peat-sim is a lab harness, not a published crate, so the bump lands as a single source-tree commit.

## [0.9.0-rc.16] - 2026-05-26

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.15` → `0.9.0-rc.16`. `peat-ffi` stays at `0.2.5` — no JNI surface additions; the `formation_handshake` migration is a pure signature swap (`&Connection` → `&dyn QuicMeshConnection`) with no UniFFI surface impact.
>
> **ADR-062 follow-up closure milestone.** Closes [peat#932](https://github.com/defenseunicorns/peat/issues/932) — `formation_handshake.rs` now takes `&dyn QuicMeshConnection` from `peat_mesh::network` instead of the raw `iroh::endpoint::Connection`. peat-protocol's reach into iroh-specific surface area shrinks from "any `Connection` method" to "exactly four trait methods" (`open_bi`, `accept_bi`, `close_reason`, `remote_endpoint_id`). A future iroh `Connection` method addition no longer widens peat-protocol's reachable surface by default — widening the trait requires a deliberate peat-mesh PR. This closes the ADR-062-Phase-2-follow-up gap the QA reviewer of [peat-mesh#166](https://github.com/defenseunicorns/peat-mesh/pull/166) flagged: "transport-agnosticism enforced only at the import path, not the API shape."
>
> Validated end-to-end on `rpi-ci` + `rpi-ci2` + laptop (192.168.228.0/24) against the published `peat-mesh 0.9.0-rc.24`: all four QUICKSTART scenarios converge cleanly, transitive gossip working across heterogeneous hardware (x86_64 laptop + aarch64 Pi 5s).

### Changed

- **Workspace `peat-mesh` floor bumped to `0.9.0-rc.24`** (was `>=0.9.0-rc.22, <0.9.1` → `>=0.9.0-rc.24, <0.9.1`). rc.23 ([peat-mesh#171](https://github.com/defenseunicorns/peat-mesh/pull/171)) shipped the `parse_close_reason` structured-variant refactor (peat-mesh#164) — internal to peat-mesh, no consumer change. rc.24 ([peat-mesh#173](https://github.com/defenseunicorns/peat-mesh/pull/173)) introduces the narrow `peat_mesh::network::QuicMeshConnection` trait and removes the `pub use iroh::endpoint::Connection` re-export at `peat_mesh::network::Connection`. Pinning below rc.24 would fail to resolve `QuicMeshConnection`; staying at rc.22/rc.23 would keep the old `Connection` import path alive but block the SKILL.md transport-agnosticism-at-API-shape gate this release closes.
- **`peat-protocol/src/network/formation_handshake.rs` signatures**: `perform_initiator_handshake` and `perform_responder_handshake` take `connection: &dyn QuicMeshConnection` instead of `&Connection`. Method bodies unchanged — the two methods used (`open_bi`, `accept_bi`) are dispatched through the trait object. The two callsites in `peat-protocol/src/sync/automerge.rs` (lines 1681, 2433) pass `&conn` where `conn: iroh::endpoint::Connection`; Rust's deref-coerce handles `&Connection → &dyn QuicMeshConnection` automatically through the `impl QuicMeshConnection for Connection` upstream, so no callsite changes needed.
- **`peat-protocol/src/network.rs` re-export list**: `Connection` dropped, `QuicMeshConnection` added — `pub use peat_mesh::network::{DiscoveryEvent, EndpointId, QuicMeshConnection};`.

### Validation

- `cargo test --workspace --features automerge-backend --lib`: **1226 passed**; 0 failed.
- 4 iroh-touching integration tests (`automerge_iroh_sync_e2e`, `tombstone_sync_e2e`, `startup_optimization_e2e`, `iroh_minimal_connection`): **12 passed**.
- `cargo clippy --workspace --features automerge-backend --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- SKILL.md verification gate `grep -rln "use iroh\|use iroh_blobs\|iroh::Endpoint" peat-protocol/src/`: zero matches outside the re-export shim layer.
- **QUICKSTART 4 scenarios validated on rpi-ci + rpi-ci2 + laptop** (192.168.228.0/24) against the **unpatched workspace consuming published `peat-mesh 0.9.0-rc.24` from crates.io** — not a `[patch.crates-io]` override.
- All 22 CI checks on [#933](https://github.com/defenseunicorns/peat/pull/933) green.

## [0.9.0-rc.15] - 2026-05-25

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.14` → `0.9.0-rc.15`. `peat-ffi` stays at `0.2.5` — no JNI surface additions in this release; the only peat-ffi change is the internal `IrohMeshTransport::new` constructor call switching from `peat_protocol::transport::iroh::IrohMeshTransport` (deleted) to `peat_mesh::transport::iroh_mesh::IrohMeshTransport` (canonical, via the `peat_protocol::transport::*` re-export shim). The `create_node` UniFFI signature is byte-identical; the Kotlin/Swift binding surface is unchanged.
>
> **ADR-062 Phase 2 closure milestone**. The structural-violation half of ADR-062 — `peat-protocol` owning an iroh-backed `MeshTransport` impl in violation of `peat/SKILL.md`'s transport-agnosticism rule — is closed here. peat-mesh is now the sole iroh consumer in the ecosystem: it owns `IrohTransport` (moved in rc.18 / peat#918), the iroh-backed `MeshTransport` impl (`IrohMeshTransport`, peat-mesh rc.21 / [peat-mesh#162](https://github.com/defenseunicorns/peat-mesh/pull/162)), and the re-export surface peat-protocol's surviving consumers reach for (`Connection`, `EndpointId`, `DiscoveryEvent` — peat-mesh rc.21 / rc.22). `peat-protocol/src/transport/iroh.rs` (918 lines) is deleted; `iroh`, `iroh-blobs`, `iroh-mdns-address-lookup` are no longer in `peat-protocol/Cargo.toml`'s `[dependencies]`. The SKILL.md grep gate `grep -rln "use iroh\|use iroh_blobs\|iroh::Endpoint" peat-protocol/src/` now returns three files, all in the re-export shim layer (`network.rs`, `lib.rs`, `storage/mod.rs`, `transport/mod.rs`) — zero violations outside the shim. Validated end-to-end on `rpi-ci` + `rpi-ci2` + laptop (192.168.228.0/24) against the published `peat-mesh 0.9.0-rc.22`: all four QUICKSTART scenarios converge cleanly, transitive gossip working across heterogeneous hardware (x86_64 laptop + aarch64 Pi 5s). Closes [#926](https://github.com/defenseunicorns/peat/issues/926).

### Changed

- **Workspace `peat-mesh` floor bumped to `0.9.0-rc.22`** (was `>=0.9.0-rc.20, <0.9.1` → `>=0.9.0-rc.22, <0.9.1`). rc.21 ships the iroh-backed `MeshTransport` impl (`peat_mesh::transport::iroh_mesh::IrohMeshTransport`) and re-exports `iroh::EndpointId` from `peat_mesh::network`. rc.22 adds the `Connection` (consumed by `peat-protocol/src/network/formation_handshake.rs`) and `DiscoveryEvent` (consumed by `peat-protocol/src/sync/automerge.rs`'s mDNS event handler) re-exports the rc.21 surface accidentally missed. Pinning below rc.22 would either fail to resolve the new `IrohMeshTransport` import (`rc.20`) or fail to resolve the `Connection`/`DiscoveryEvent` re-exports the consumer-side `use` statements now reach for (`rc.21`). No transport semantics or wire-shape change.
- **`peat-ffi` `IrohMeshTransport::new` constructor call**: takes `Vec<PeerInfo>` directly (peat-mesh's new constructor) instead of constructing an `Arc<RwLock<PeerConfig>>` wrapper (peat-protocol's old constructor). The `formation` and `local` fields of `PeerConfig` were never consulted by the transport layer — only `config.peers: Vec<PeerInfo>` was — so this is a functional no-op. `peat-ffi` constructs with an empty static-peer list at startup; runtime additions go through `iroh_mesh_transport.set_static_peers(...)` (the inline-registration method peat-mesh rc.21 added during QA on [peat-mesh#162](https://github.com/defenseunicorns/peat-mesh/pull/162)).
- **11 `iroh::EndpointId` import sites in `peat-protocol/src/`** rewired to `peat_mesh::network::EndpointId`. Same underlying type via peat-mesh's re-export. Sites: `network.rs`, `discovery/peer.rs`, `hierarchy/router.rs` (qualified-path), `network/formation_handshake.rs`, `storage/automerge_backend.rs`, `sync/automerge.rs` (5 qualified-path call sites + 2 `EndpointId::from_bytes` constructors). Plus `iroh::endpoint::Connection` in `formation_handshake.rs` → `peat_mesh::network::Connection`, and `iroh_mdns_address_lookup::DiscoveryEvent` in `sync/automerge.rs` → `peat_mesh::network::DiscoveryEvent`.

### Added

- **Linux-host Swift UniFFI binding-generation CI job** (`.github/workflows/android.yml`'s new `generate-swift-bindings` job). Mirrors the existing `Generate Kotlin bindings` job: runs `uniffi-bindgen generate --library libpeat_ffi.so --language swift --out-dir bindings/swift` on `ubuntu-latest`, pins the 3 expected output artifacts (`peat_ffi.swift`, `peat_ffiFFI.h`, `peat_ffiFFI.modulemap`), uploads the bindings as a workflow artifact. Catches UniFFI metadata regressions for Swift at PR time. Closes the ADR-062 §"Cross-compiled FFI binding validation" iOS gate at the generation tier; Swift-compile validation is deferred until iOS becomes an active consumer downstream (would need a macOS runner — separate cost decision).

### Removed

- **`peat-protocol/src/transport/iroh.rs`** (918 lines, the parallel `IrohMeshTransport` implementation). Its canonical home is `peat_mesh::transport::iroh_mesh::IrohMeshTransport`. Consumers continuing to import via `peat_protocol::transport::IrohMeshTransport` keep working via the `pub use peat_mesh::transport::*` re-export at `peat-protocol/src/transport/mod.rs`. The `iroh` submodule path (`peat_protocol::transport::iroh::IrohMeshTransport`) is gone — direct importers should switch their `use` lines to the new path.
- **`peat-protocol/tests/dual_active_simultaneous.rs`** (416 lines, 3 tests: `test_iroh_and_ble_simultaneously_active`, `test_iroh_lifecycle_doesnt_affect_ble`, `test_simultaneous_routing_decisions`). The tests were migrated byte-for-byte to `peat-mesh/tests/dual_active_simultaneous.rs` (lines 164, 248, 311) in peat-mesh rc.21 ([peat-mesh#162](https://github.com/defenseunicorns/peat-mesh/pull/162), merge commit `5909b20`). The peat-mesh location is canonical for transport-tier integration tests as of ADR-062 Phase 2.
- **`peat-protocol/Cargo.toml`'s direct dependencies on `iroh`, `iroh-blobs`, `iroh-mdns-address-lookup`**. All three reach peat-protocol transitively via `peat-mesh` per ADR-062 acceptance gate #2. The iroh feature configuration (`default-features = false`, `tls-aws-lc-rs`, `metrics`, `fast-apple-datapath`, `portmapper`) lives in `peat-mesh/Cargo.toml` — peat-mesh is the canonical iroh consumer per ADR-060 §5 (FIPS posture: aws-lc-rs, not ring). `iroh` returns as a `[dev-dependencies]` entry — four integration tests under `peat-protocol/tests/` (`iroh_minimal_connection`, `startup_optimization_e2e`, `automerge_iroh_sync_e2e`, `tombstone_sync_e2e`) still use direct `iroh::Endpoint::builder` / `iroh::TransportAddr` scaffolding. ADR-062's `[dependencies]` gate is literal; `[dev-dependencies]` is out of scope. Same FIPS-parity feature set (`tls-aws-lc-rs`) as peat-mesh's runtime activation.

### Validation

- `cargo test --workspace --features automerge-backend --lib`: **1226 passed; 0 failed**.
- `cargo test -p peat-protocol --features automerge-backend --test {automerge_iroh_sync_e2e,tombstone_sync_e2e,startup_optimization_e2e,iroh_minimal_connection}`: all pass.
- `cargo clippy --workspace --features automerge-backend --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- **QUICKSTART 4 scenarios validated on rpi-ci + rpi-ci2 + laptop** (192.168.228.0/24) against the **unpatched workspace consuming published `peat-mesh 0.9.0-rc.22` from crates.io** — not a `[patch.crates-io]` override. Transitive gossip working across heterogeneous hardware (x86_64 laptop + aarch64 Pi 5s).
- SKILL.md verification gate `grep -rln "use iroh\|use iroh_blobs\|iroh::Endpoint" peat-protocol/src/`: zero matches outside the re-export shim layer.
- All 22 CI checks on [#930](https://github.com/defenseunicorns/peat/pull/930) green, including the new `Generate Swift bindings` job, `Build Android (aarch64/armv7/x86_64)` × 3, `Generate Kotlin bindings`, `Package Android AAR`, `peat-ffi Android Surface Test`, `peat-ffi JVM JNI Tests`, `E2E Tests`, `Documentation`, `Supply Chain`, `Security Audit`, and `Peat QA Review`.

## [0.9.0-rc.14] - 2026-05-24

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.13` → `0.9.0-rc.14`. `peat-ffi` bumps `0.2.5` → carries the peat#925 Android JNI surface additions; consumers of the AAR must declare `external fun setAndroidContextJni(context: Any)` only if they exercise iroh DNS-based discovery (relay, pkarr) — mDNS-only consumers see no surface change. **FIPS posture milestone**: the active rustls crypto provider for every iroh QUIC handshake in the workspace is now `aws-lc-rs`; the `ring` symbol surface remains transitively linked via `noq-proto` / `rcgen` / `rustls-webpki` and is tracked under [#923](https://github.com/defenseunicorns/peat/issues/923) for upstream removal. Closes [#923](https://github.com/defenseunicorns/peat/issues/923) (consumer side) and [#925](https://github.com/defenseunicorns/peat/issues/925) (Android JNI register-natives abort). Validated end-to-end on `rpi-ci` + `rpi-ci2` + laptop (192.168.228.0/24) against `peat-mesh 0.9.0-rc.20`: all four QUICKSTART scenarios converge cleanly, transitive gossip working across heterogeneous hardware (x86_64 laptop + aarch64 Pi 5s), no `noq_udp: sendmsg` warnings, no connection-recycle disruptions under `PEAT_CONNECTION_RECYCLE_SECS=0`.

### Added

- **`peat-ffi` JNI surface: `setAndroidContextJni(context: Any)` + `verifyAndroidContextJni(): Boolean`** ([#925](https://github.com/defenseunicorns/peat/issues/925)). The iroh 1.0.0-rc.0 cascade introduced `ndk-context` as a transitive dep via `swarm-discovery` / `iroh-mdns-address-lookup` / `iroh-dns` → `hickory-resolver`. `JNI_OnLoad` now calls `ndk_context::initialize_android_context(vm, null)` — enough for the iroh discovery subtree that only needs the JVM for thread attachment. Consumers exercising iroh DNS-based discovery (relay, pkarr, non-mDNS peer lookups) must call `PeatJni.setAndroidContextJni(applicationContext)` from `Application.onCreate()` before the first `createNodeJni`. The Rust side promotes the JNI local-ref to a process-lifetime `GlobalRef`, pins it under a `Mutex<Option<GlobalRef>>`, and performs an atomic `release_android_context()` + `initialize_android_context(vm, ctx)` pair. An `AtomicBool IROH_STARTED` flag (set via `Release` on first successful `createNodeJni`/`createNodeWithConfigJni`) makes post-iroh-start invocations a logged no-op instead of a SIGABRT — surfaced in logcat as `I PeatFFI : setAndroidContextJni: ignoring — iroh already started`. `verifyAndroidContextJni` is a surface-tier test hook; production code should not consult it.
- **`peat-ffi` Android panic hook in `JNI_OnLoad`**. Forwards Rust panics from any worker thread to logcat under the `PeatFFI` tag (`PANIC in thread '<name>' at <file>:<line>:<col>: <message>`) before chaining to the default handler. Idempotent via `OnceLock`. Closes the visibility gap where the default Rust panic handler wrote to stderr — which Android's logcat never captures — and made every panic in the iroh / quinn / rustls / aws-lc-rs / redb subtree look like a silent SIGABRT with stripped Rust frames. This permanent diagnostic addition surfaced the original `ndk-context` panic that drove the [#925](https://github.com/defenseunicorns/peat/issues/925) follow-on chain.
- **Surface-tier instrumented Android tests** in `peat-ffi/android/src/androidTest`: `a_setAndroidContextJni_wiresContextThroughToNdkContext` (happy path, runs first under `@FixMethodOrder(NAME_ASCENDING)` so `IROH_STARTED` hasn't fired yet) and `z_setAndroidContextJni_isNoOpAfterIrohStart` (rejection path, runs after createNode-invoking tests). Both gate on the Galaxy Tab A9+ (`SM-X210`) self-hosted runner.

### Changed

- **`peat-protocol`: iroh 0.97 → 1.0.0-rc.0** ([#923](https://github.com/defenseunicorns/peat/issues/923), [#924](https://github.com/defenseunicorns/peat/pull/924)) — exact-pinned as `=1.0.0-rc.0` alongside `iroh-blobs =0.101.0` and the new extracted `iroh-mdns-address-lookup =0.2.0` crate. API migration handled inline: `Endpoint::builder(presets::Empty).crypto_provider(...)` replaces the retired `empty_builder()`, `DiscoveryEvent` wildcard arm added for the now-`#[non_exhaustive]` enum, `iroh_mdns_address_lookup::DiscoveryEvent` import path replaces the previous `iroh::address_lookup::mdns::DiscoveryEvent`. `SecretKey::generate()` is now no-arg. `PathInfo` split into `Path<'a>` + owned `PathStats`. Exact pins reflect RC-train stability; upgrade-to-stable policy is tracked under [#923](https://github.com/defenseunicorns/peat/issues/923#issuecomment-4528407237).
- **FIPS provider cutover: `aws-lc-rs` is the active rustls crypto provider for every iroh QUIC handshake.** Wired via `Endpoint::builder().crypto_provider(peat_mesh::security::tls_provider::iroh_quic_provider())` at every endpoint-construction site in the workspace, threaded from `peat-mesh`'s `tls-aws-lc-rs` feature flag. `ring` is **not** FIPS 140-3 validated and is no longer the active provider for any handshake the workspace initiates. Residual `ring` transitive linkage remains via `noq-proto` (declares both `ring` and `aws-lc-rs`), `rcgen`, and `rustls-webpki` — these are tracked for upstream removal under [#923](https://github.com/defenseunicorns/peat/issues/923#issuecomment-4528407237). Procurement audits that grep the binary for `ring` symbols should be cross-referenced against that tracking comment.
- **Workspace `peat-mesh` floor bumped to `0.9.0-rc.20`** (was `>=0.9.0-rc.17, <0.9.1` → `>=0.9.0-rc.20, <0.9.1`). rc.20 ships the full iroh 1.0.0-rc.0 API migration (`Endpoint::builder(presets::Empty)`, extracted `iroh-mdns-address-lookup`, `Path<'a>` + `PathStats` split, `PathListStream` for the `peer_paths` accessor) atop rc.19's iroh 0.98 baseline. Pinning below rc.20 either refuses to resolve (rc.19 has iroh 0.98 which is wire-incompatible with `peat-protocol`'s `=1.0.0-rc.0` pin) or — at rc.19 — misses the 1.0 API migration. Either outcome is a build break, not a silent regression.
- **`peat-ffi` `nativeInit` cleanup**: removed four `NativeMethod` entries (`subscribe`/`unsubscribeDocumentChangesJni`, `subscribe`/`unsubscribeOutboundFramesJni`) that referenced consumer-supplied Kotlin listener interfaces (`DocumentChangeListener`, `OutboundFrameListener`) which don't exist in peat-ffi's own `PeatJni.kt`. CheckJNI (active on AndroidJUnit's debug-instrumented builds — the Galaxy Tab A9+ CI runner config) was aborting the test process during `JNI_OnLoad → RegisterNatives` because the signatures pointed at non-existent classes. The four Rust extern fns `Java_..._subscribe*Jni` / `unsubscribe*Jni` remain exported `#[no_mangle] pub extern "system" fn` and stay reachable for downstream consumers (e.g. peat-atak-plugin) that declare the listener interfaces locally — same as the pre-0.1.2 pattern documented at `peat-ffi/android/src/main/kotlin/.../PeatJni.kt:27-34`.

### Removed

- **Stale `RUSTSEC-2026-0118` / `-0119` / `-0120` ignores** dropped from `deny.toml` and `.github/workflows/ci.yml`. The original rationale ("iroh 0.98.x exact-pins `hickory-resolver =0.26.0-beta.4`, awaiting iroh 0.98.3/1.0") has been satisfied by this release landing iroh `1.0.0-rc.0`, which carries `hickory-proto` + `hickory-net` at `0.26.1` — the fixed versions. `cargo audit` against the current `Cargo.lock` no longer surfaces any of the three; the ignores were silently suppressing a class of regression they no longer needed to.
- **`peat-discovery` workspace subcrate retired entirely** ([#919](https://github.com/defenseunicorns/peat/issues/919), Phase 3 of the [#898](https://github.com/defenseunicorns/peat/issues/898) mDNS-consolidation epic; landed pre-release into `[Unreleased]` and shipped as part of rc.14). After Phase 1's `peat_discovery::mdns` deletion, the remaining `StaticDiscovery` / `HybridDiscovery` / `DiscoveryStrategy` trait had **zero in-repo consumers** (verified across `peat`, `peat-mesh`, `peat-btle`, `peat-atak-plugin`) and the crate was never published to crates.io. The canonical home for discovery strategies is `peat_mesh::discovery::{StaticDiscovery, HybridDiscovery, KubernetesDiscovery, MdnsDiscovery}`. No downstream-consumer release impact — the subcrate was internal-only.

### Supply chain

- **cargo-vet exemptions regenerated** for the new transitive deps brought in by iroh `1.0.0-rc.0` + `aws-lc-rs`: 33 new `safe-to-deploy` exemption stanzas covering the iroh ecosystem (`iroh-dns`, `iroh-util`, `iroh-metrics-derive`, `iroh-mdns-address-lookup`, `n0-error`, `n0-error-macros`), the RustCrypto major-version cuts pulled in by the iroh 1.0 crypto stack (`sha2 0.11`, `digest 0.11`, `ed25519 3.0`, `spki 0.8`, `pkcs8 0.11`, `signature 3.0`, `serdect 0.4`, `cmov 0.5`, `block-buffer 0.12`), Android JNI deps for the panic-hook + `ndk-context` path (`ndk-context 0.1`, `jni-sys 0.4`, `jni-sys-macros`, `jni-macros`, `simd_cesu8`), and the iroh-relay / hickory cut (`hickory-net 0.26.1`, `prefix-trie`, `arc-swap`, `rand_core 0.10`, `rand_pcg 0.10`). Same trust posture as the preceding versions; full reconciliation in `supply-chain/imports.lock`.

### Documentation

- **Subcrate inventory pruned** across `README.md`, `CONTRIBUTING.md`, `DEVELOPMENT.md`, `DEPENDENCY-LICENSES.md`, `SKILL.md`, `docs/RELEASING.md`, `docs/ARCHITECTURE.md`, `docs/guides/developer/DEVELOPER_GUIDE.md`, and `peat/src/lib.rs` (landed under `[Unreleased]` pre-rc.14, shipped here). Historical `[0.9.0-rc.*]` CHANGELOG entries that name `peat-discovery` are left intact — those record the state at release time and shouldn't be retroactively edited.

## [0.9.0-rc.13] - 2026-05-22

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.12` → `0.9.0-rc.13`. `peat-ffi` unchanged (no JNI ABI surface change). No wire-format change. Closes [#890](https://github.com/defenseunicorns/peat/issues/890) and [#892](https://github.com/defenseunicorns/peat/issues/892) — the two remaining QUICKSTART Scenario 4 regressions on the field-report path (laptop + 2 Pis cross-LAN). All four documented quickstart scenarios converge cleanly on this release.

Validated end-to-end on rpi-ci + rpi-ci2 + laptop (192.168.228.0/24) against the released `peat-mesh 0.9.0-rc.17`:

- **S1** (2-node static, `127.0.0.1`): 13/13 sync events per node, 0 lost. Specific bind takes the unchanged single-`bind_addr` path; interface filter does not fire.
- **S2** (3-node hub-and-spoke, `127.0.0.1`): 54 alpha sync events; transitive gossip 18 + 14 across the two spokes (bravo↔charlie via alpha), 0 lost.
- **S3** (3-node mDNS, `127.0.0.1 --mdns`): mDNS found both peers; 19 sync events per direct edge, transitive gossip 19, 0 lost.
- **S4** (Pi + Pi + laptop, `0.0.0.0`): 126/124/124 sync events per node over a 3+ min run, **0 `noq_udp: sendmsg` warnings**, 0 lost events, 0 connection-recycle disruptions. Laptop alpha kept exactly `eno1=192.168.228.236/24` + `wlp11s0=192.168.1.81/24` after the interface filter applied (`kept_count=2 dropped_count=27`).

### Added

- **iroh interface-advertisement filter — `peat-protocol::network::iroh_transport::from_seed_at_addr` now delegates to `peat_mesh::storage::interface_filter::select_advertise_interfaces` when the caller passes an unspecified bind addr (`0.0.0.0` / `[::]`)** ([#890](https://github.com/defenseunicorns/peat/issues/890), via [peat-mesh#152](https://github.com/defenseunicorns/peat-mesh/pull/152) + [peat-mesh#154](https://github.com/defenseunicorns/peat-mesh/pull/154) on the transport-layer side). The default behavior on `--bind 0.0.0.0:39001` previously enumerated every local interface (docker bridges, tailscale CGNAT, link-local, stale leases) and published the full set to peers as candidate dial targets — peers raced the candidates and dialed the unreachable ones, producing `noq_udp: sendmsg EIO/EINVAL` warnings (cosmetic, but loud) and burning path-probe budget. The filter drops loopback, link-local, tailscale CGNAT (`100.64.0.0/10`), and docker / podman / CNI bridges by interface-name pattern (docker user-defined bridges specifically match `br-<12-hex>`, not arbitrary `br-*`, so legitimate Linux bridges named `br-corp` / `br-lan` / `br-vlan10` aren't false-positives). The CIDR prefix from each interface is threaded through to `BindOpts::set_prefix_len` so iroh routes outgoing flows by longest-prefix match. Specific bind addresses (`--bind 127.0.0.1:39001` / `--bind <LAN_IP>:39001`) take the existing single-`bind_addr` path unchanged — QUICKSTART Scenarios 1–3 are unaffected. Two env-var overrides expose the filter to operators: `PEAT_ADVERTISE_ALL_INTERFACES=1` bypasses defaults (loopback still always dropped); `PEAT_ADVERTISE_INTERFACES=eno1,eth0` restricts to an explicit allowlist.
- **`PEAT_CONNECTION_RECYCLE_SECS` environment variable** to override the [#435](https://github.com/defenseunicorns/peat/issues/435) connection-recycle interval at runtime ([#892](https://github.com/defenseunicorns/peat/issues/892)). The compile-time default — `peat_protocol::network::iroh_transport::CONNECTION_RECYCLE_INTERVAL_SECS = 60` — stays unchanged; the new `connection_recycle_interval_secs()` resolver reads the env var first, falls back to the constant when it's unset or non-numeric. Setting `PEAT_CONNECTION_RECYCLE_SECS=0` skips spawning the recycler task entirely. **Why this exists**: the recycler was added in May 2025 to bound an upstream iroh memory pattern (iroh#3565) at ~0.875 MB/sec growth observed under heavy sync. On iroh 0.97 with a low-churn workload (e.g. the quickstart binary's 1-doc-per-peer demo) growth measures ~0.3–0.5 MB/min — well under what the recycle interval is sized to defend against — and the visible cost of the workaround (a 4–6 s gap in continuous sync once a minute) outweighs the leak it's bounding. `QUICKSTART.md` Scenario 4 now sets `PEAT_CONNECTION_RECYCLE_SECS=0` on all three nodes; operators with heavier sync workloads can tune up rather than disable.

### Changed

- **Workspace `peat-mesh` floor bumped to `0.9.0-rc.17`** (was `>=0.9.0-rc.16, <0.9.1` → `>=0.9.0-rc.17, <0.9.1`). rc.17 ships `peat_mesh::storage::interface_filter::{select_advertise_interfaces, BoundAddr, InterfaceSelection}` + `IrohConfig::bind_addr` honoring those for unspecified binds. Pinning below rc.17 would let the workspace build against a peat-mesh missing those exports — the new call site in `peat-protocol/src/network/iroh_transport.rs::from_seed_at_addr` would fail to compile.
- **mDNS service type changed: `_peat-node._tcp.local.` → `_peat._udp.local.`** ([#898](https://github.com/defenseunicorns/peat/issues/898), [#900](https://github.com/defenseunicorns/peat/pull/900)). Phase 1 of the mDNS-discovery consolidation retires three divergent `MdnsDiscovery` implementations (one in `peat-protocol`, one in the `peat-discovery` workspace subcrate, one in the `peat-mesh` sibling) down to peat-mesh's canonical, transport-agnostic version. The peat-mesh implementation has always used `_peat._udp.local.`; quickstart and any other in-tree consumers now align on that string. **Wire-visible**: a node running the post-PR quickstart binary will not discover — and will not be discovered by — any node still running a pre-PR binary. Operators of mixed-version fleets should use static `--peer NODE_ID@ADDR` until all nodes are upgraded.
- **Workspace `peat-mesh` floor bumped to `0.9.0-rc.14`** (was `>=0.9.0-rc.12, <0.9.1` → `>=0.9.0-rc.14, <0.9.1`). rc.14 ships `MdnsDiscovery::advertise_with_addr` and the DNS-label / `handle_removed` fixes that the consolidated `examples/quickstart` requires; pinning below rc.14 would let the workspace build against a peat-mesh missing those APIs.

### Removed

- **`peat_protocol::discovery::peer::MdnsDiscovery`** and **`peat_protocol::discovery::peer::RelayDiscovery`** (the latter was vestigial — its `start()` body was a `// TODO` and no in-repo caller existed). `DiscoveryStrategy`, `StaticDiscovery`, `DiscoveryManager`, and the `PeerInfo` re-export stay in place — only the IP-stack-specific strategy implementations moved.
- **`peat_discovery::mdns` module** (workspace subcrate). The crate had no in-repo consumers of its `MdnsDiscovery`; the module + `mdns-sd` dep are removed. `HybridDiscovery` and `StaticDiscovery` are unaffected.
- **`mdns-sd` dependency** dropped from `peat-protocol/Cargo.toml` (no remaining call sites) and from `peat-discovery/Cargo.toml`.
- **Temporary `[patch.crates-io]` override for peat-mesh, `[policy.peat-mesh]`, and `[[exemptions.peat-mesh]] 0.9.0-rc.13@git:...`** — all removed once peat-mesh `0.9.0-rc.14` published to crates.io. peat-mesh now resolves from crates.io directly. The supply-chain bookkeeping is back to the pre-#900 shape.

### Documentation

- **`docs/guides/QUICKSTART.md` Scenario 4 preflight + troubleshooting** — surfaces the SSH/`nohup` stale-process trap that bit the pre-release validation pass (a `pkill -f peat-quickstart` from the local shell does not reach Pi processes launched via `ssh ... &`; the next launch then silently exits with `error: cannot bind ...: port N is already in use.` and alpha connects to the *stale* Pi instead, producing a "stuck at fuel_minutes=0" run). §4.5 now opens with a `ssh pi-a 'pgrep -af peat-quickstart || echo CLEAN'` preflight; the troubleshooting table pairs the binary's friendly bind-error message with the new `kill -9 <PID>` recovery, and adds a dedicated row for the "alpha sees `fuel_minutes=0` immediately at startup" symptom. No code change — the friendly error in `examples/quickstart/src/main.rs::explain_bind_error` was already in place for the local-host case; this fills the multi-host doc gap.

## [0.9.0-rc.12] - 2026-05-19

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.11` → `0.9.0-rc.12`. `peat-ffi` unchanged at `0.2.3` (no JNI ABI surface change). No wire-format change.

Bug-fix release closing [#873](https://github.com/defenseunicorns/peat/issues/873) — the Android OOM chain a consumer reported on rc.8 and that survived [peat-mesh rc.13](https://github.com/defenseunicorns/peat-mesh/releases/tag/v0.9.0-rc.13)'s peer-level sync gate. Adds an upstream gate at the mDNS-discovery layer in `IrohPeerDiscovery` so unreachable peers don't trigger fresh iroh `endpoint.connect()` calls every rediscovery cycle, eliminating the per-attempt QUIC handshake-state allocations that drove the leak.

### Fixed

- **mDNS-discovery-driven iroh-connect attempts now consult peat-mesh's circuit breaker** ([#873](https://github.com/defenseunicorns/peat/issues/873), [#874](https://github.com/defenseunicorns/peat/pull/874)). Before this fix, every mDNS rediscovery (~10s on Android against an unreachable peer) fired `transport.connect_by_id(peer_id)` → iroh's `endpoint.connect()`, which allocates QUIC handshake state that doesn't reliably deallocate on failed handshakes. The 60s connection recycler at `peat-protocol/src/network/iroh_transport.rs:182` only partially mitigated this; allocation rate outpaced recycle rate 6×. peat-mesh rc.13 closed the downstream sync-push side of the chain (every rediscovery still scheduled an N-document push that immediately bailed), but the iroh-connect upstream of that bail still fired. **Post-fix**: the `IrohPeerDiscovery` mDNS handler at `peat-protocol/src/sync/automerge.rs:~2520` consults a `PeerAvailabilityCheck` closure (wired by `AutomergeIrohBackend::peer_discovery()` to `coordinator.error_handler().should_block_sync(peer_id)`) before invoking `connect_by_id`. When the breaker is open, the iroh-connect never fires — no fresh QUIC state allocation. The per-rediscovery native-heap growth pattern reported by the consumer should resolve.

### Added

- **`PeerAvailabilityCheck` type alias + `build_peer_availability_check()` free fn** at `peat-protocol/src/sync/automerge.rs:~2324`. The closure-builder is extracted from inline `peer_discovery()` construction so its negation semantic (`!coordinator.error_handler().should_block_sync(peer_id)`) is directly testable. Three behavior tests lock the three state arms: no coordinator (pre-`start_sync`) → allow; coordinator with closed breaker → allow; coordinator with open breaker → block (and per-peer — other peers still allowed in the same call).
- **Structural pin test** `mdns_discovery_handler_gates_connect_on_availability_check` source-greps the file to confirm the mDNS Discovered branch consults `availability_check.as_ref()` *before* calling `transport.connect_by_id(peer_id)`. Includes an "IF YOU REFACTOR THIS, THE TEST IS A STRUCTURAL PIN, NOT A STYLE CHECK" guidance comment for future maintainers reading a failing assertion.

### Verification

- `cargo check --workspace` clean across all features.
- `cargo test -p peat-protocol --lib` — 1000 tests pass (was 996 pre-PR; +4 new: 3 behavioral closure tests + 1 structural pin).
- `cargo test --workspace --lib` clean.
- `cargo fmt --check` clean.
- `cargo vet` — supply-chain rc.12 exemption stanzas added pre-emptively (per `supply-chain/README.md` — same release-bookkeeping discipline; without this, CI on a hypothetical post-merge PR would fail with the rc-bookkeeping miss the rc.10 ADR-060 branch hit).

### Migration

Pure consumer migration. Bump the pin:

```toml
peat-protocol = "0.9.0-rc.12"
peat-schema   = "=0.9.0-rc.12"
peat-mesh     = ">=0.9.0-rc.12, <0.9.1"  # unchanged from rc.11
```

No public API changes. The new `PeerAvailabilityCheck` plumbing is internal — backward-compatible default is "always attempt" when no check is wired.

### Known follow-ups (not in this release)

- **[#875](https://github.com/defenseunicorns/peat/issues/875)** — two structurally-identical iroh-connect call sites in `IrohPeerDiscovery::start()` remain ungated: the topology-driven `connect_peer` at `automerge.rs:~2645` and the periodic-discovery-loop `connect_peer` at `automerge.rs:~2830`. The reported trace was mDNS-only, so the immediate leak is closed. Filed as a follow-up; trigger conditions for prioritization documented (bump if a topology-driven or periodic-discovery deployment reports the same pattern).
- **[peat-mesh#130](https://github.com/defenseunicorns/peat-mesh/issues/130)** — orthogonal Iroh-side `swarm_discovery::sender` announce-side failure on Android. Per the rc.13 retest data, the symptom is no longer observed in the reporting environment; tracking remains open pending an iroh upstream resolution.

## [0.9.0-rc.11] - 2026-05-18

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.10` → `0.9.0-rc.11`. `peat-ffi` unchanged at `0.2.3` (no JNI ABI surface change). No wire-format change — the `IROH_DISTRIBUTION_COLLECTION` schema is identical to rc.10.

FIPS-posture release. Lands [ADR-060](docs/adr/060-encryption-tiers-rest-and-transit.md) (encryption tiers at-rest and in-transit) as the ecosystem-spanning crypto contract, propagates the FIPS-approved-primitives-only hard rule into `CLAUDE.md` / `SKILL.md` + every existing ecosystem doc that referenced ChaCha20-Poly1305, and consumes peat-mesh [rc.12](https://github.com/defenseunicorns/peat-mesh/releases/tag/v0.9.0-rc.12) — the matching peat-mesh release that swapped the actual primitives. Two merged PRs: [#870](https://github.com/defenseunicorns/peat/pull/870) (the ADR + amendments) and [#871](https://github.com/defenseunicorns/peat/pull/871) (the consumer-side dep bump + peat-protocol test fix).

### Added

- **`docs/adr/060-encryption-tiers-rest-and-transit.md`** — ADR-060, six revisions in the landing PR. Names the threat tiers T1–T6 explicitly (T4 = formation member without payload key, the load-bearing role for tactical deployments). Maps every encryption phase of the stack (discovery, connection setup, doc sync, doc at-rest, attachments, file_distributions, bypass) to its own decision. Introduces driver #6 "FIPS-approved primitives only" + §5 "Cryptographic primitives (FIPS posture)" as the authoritative ecosystem primitive list. Commits per-collection encryption posture (`Plaintext` / `FieldValues` / `FullOpacity`) with strict-monotonic strengthen-only LUB merge semantics for concurrent posture edits + a dedicated CAS-guarded `DowngradeCollectionPosture` RPC for the weakening path. Commits attachment encryption inline (§Decision §6: two-layer cipher, chunked AES-256-GCM at 64 KiB, Deterministic + Randomized nonce modes, peat-registry / peat-sim / peat-node consumer surfaces). Commits a per-collection `format_version` axis for the legacy-envelope → posture migration with explicit sidecar opt-in + a `peat-node migrate-collection` tool. Closes six [BLOCKER]/[ARCH]/[WARNING] QA-review findings inline.
- **`CLAUDE.md` "Hard rule: FIPS-approved cryptographic primitives only"** + **`SKILL.md` "FIPS-approved cryptographic primitives only"** invariant. Both call out AES-GCM / Ed25519 / ECDH-P256-P384 / HKDF-SHA-2 / HMAC-SHA-2 / SHA-2 / rustls-under-FIPS-mode-provider as approved, explicitly non-approve ChaCha20-Poly1305, flag X25519 as marginal, and route conflicting consultations to ADR-060 §5 over the legacy ADR-006 / ADR-044 references.
- **`supply-chain/README.md`** — durable home for the operational guidance about why each `[policy.*]` / `[[exemptions.*]]` block exists. `cargo vet` rewrites `config.toml` on every invocation (alphabetizes + strips comments), so this README is where the audit-as-crates-io footgun, the `[policy.peat]` reserved-name flow, the Slice-4.d cutover note, and the per-rc-release-bump workflow live now.
- **`peat-protocol` re-exports** `ECDH_PUBLIC_KEY_SIZE` (33 bytes, compressed SEC1 P-256) and `ECDH_SECRET_KEY_SIZE` (32 bytes) from peat-mesh's amended security module.

### Changed

- **Workspace `peat-mesh` pin floor advanced rc.10 → rc.12.** rc.12 is the [peat-mesh FIPS-posture release](https://github.com/defenseunicorns/peat-mesh/releases/tag/v0.9.0-rc.12) — `EncryptionKeypair` / `EncryptionManager` / `BypassChannelSecurity` swapped from ChaCha20-Poly1305 + X25519 to AES-256-GCM + ECDH-P256 (FIPS 140-3 approved equivalents, NIST SP 800-38D + SP 800-56A) and the at-rest `Cipher` trait shipped. The floor is rc.12-specific because peat-protocol's `reexport_encryption_keypair_dh_exchange` test calls the new `ecdh::SharedSecret::raw_secret_bytes()` accessor; pinning the floor below rc.12 would let the test fail to compile on a downgrade resolve.
- **ADR-006 (`docs/adr/006-security-authentication-authorization.md`)** amended with an explicit FIPS-posture amendment block superseding the inline ChaCha20-Poly1305 references; the latent contradiction between the existing FIPS 140-2/3 line and the prior ChaCha20-Poly1305 acceptance criterion is resolved. **ADR-044 (`docs/adr/044-e2e-encryption-key-management.md`)** amended: MLS ciphersuite selection moves from a ChaCha20/X25519 suite to `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`; the OpenMLS provider is flagged as a placeholder needing a FIPS-mode (e.g. `aws-lc-rs`-backed) provider before MLS ships. **ADR-048** flags peat-btle's ChaCha20-Poly1305 reference for the sibling-repo FIPS amendment. **ADR-049** records the 2026-05-18 FIPS amendment in its decision log (Phase 5 historical row preserved).
- **README.md** (tech stack + cryptographic primitives table + Layer 4 prose), **`docs/ARCHITECTURE.md`** (encryption layer line), **`docs/spec/005-security.md`** + **`docs/whitepaper/10b-spec-appendix.md`** (security objectives, §7.1 algorithms, §7.3 code samples, §10.2 threats, §11 implementation requirements, Appendix A references, revision history) all updated to AES-256-GCM / ECDH-P256 + ADR-060 §5 cites. **`docs/hive-btle-slicksheet.md`** carries an amendment note flagging the peat-btle sibling-repo work.
- **`peat-protocol/src/security/encryption.rs`** test `reexport_encryption_keypair_dh_exchange` updated: `shared.as_bytes()` → `shared.raw_secret_bytes().as_slice()` to match the new `p256::ecdh::SharedSecret` accessor. `reexport_constants_accessible` updated to assert `ECDH_PUBLIC_KEY_SIZE = 33` + `ECDH_SECRET_KEY_SIZE = 32` (replacing `X25519_PUBLIC_KEY_SIZE = 32`).
- **`peat-protocol/Cargo.toml`** — direct `chacha20poly1305 = "0.10"` and `x25519-dalek = "2"` entries removed. Grep of `peat-protocol/{src,tests,examples,benches}` returned zero uses; they were dead manifest declarations contradicting the FIPS hard rule. Symmetric AEAD + DH primitives are re-exported from peat-mesh now.
- **`supply-chain/config.toml`** rc.10 exemptions added (`peat`, `peat-protocol`, `peat-schema` at `0.9.0-rc.10` `safe-to-deploy`).
- **`supply-chain/audits.toml`** publisher-trust extended: 9 new `[[trusted.*]]` blocks rooted at `user-id = 267` (Tony Arcieri / tarcieri, RustCrypto maintainer — same trust path as the ambient `aead` / `aes` / `ed25519` / `sha2`) covering the new `p256` transitive deps (`p256`, `ecdsa`, `elliptic-curve`, `primeorder`, `sec1`, `rfc6979`, `crypto-bigint`, `hybrid-array`, `der`). 2 new blocks rooted at `user-id = 6289` (Jack Grigg / str4d) for `ff` + `group`. All `safe-to-deploy`; no new exemptions.

### Verification

- `cargo check --workspace` clean across all features.
- `cargo test --workspace --lib` clean.
- `cargo test -p peat-protocol --lib` — 996/996 passed (the FIPS-updated `reexport_encryption_keypair_dh_exchange` + `reexport_constants_accessible` both green).
- `cargo fmt --check` clean.
- `cargo vet` clean: "Vetting Succeeded (681 fully audited, 30 exempted)" — was 670 audited pre-rc.11; +11 from the new publisher-trust blocks, no new exemptions.

### Migration

Default consumers depending on `peat-protocol` (or the workspace) only need to bump pins:

```toml
peat-protocol = "0.9.0-rc.11"
peat-schema   = "=0.9.0-rc.11"  # peat-protocol pins peat-schema as `=`
peat-mesh     = ">=0.9.0-rc.12, <0.9.1"  # via peat-protocol's workspace dep
```

The peat-mesh primitive swap is **BREAKING at the wire** (peers on the previous primitives cannot interoperate with peers on the new ones) and **BREAKING at the API** (`EncryptionKeypair::public_key_bytes` is now `[u8; 33]`, `from_secret_bytes` returns `Result<Self, SecurityError>`). See [peat-mesh CHANGELOG entry for 0.9.0-rc.12](https://github.com/defenseunicorns/peat-mesh/blob/main/CHANGELOG.md#090-rc12---2026-05-18) for the full migration breakdown — peat-protocol consumers that didn't touch those surfaces directly (the common case) are unaffected.

### Tracked follow-ups (not in this release)

- **peat-registry `RegistryClient` adapter** for ADR-060 §6.5 (encrypt-on-push / decrypt-on-pull adapter + plaintext-offset checkpoint semantics). Needs sibling-repo maintainer ack before the adapter shape is final.
- **peat-sim `create_blob_from_bytes` signature change** for ADR-060 §6.6. Same coordination path.
- **peat-node `SendAttachments` proto `encryption_mode` extension** for ADR-060 §6.4 + Phase E.
- **FIPS-mode rustls/Iroh provider swap** — Iroh's quinn/rustls defaults to non-FIPS-validated `ring`; switching to `aws-lc-rs` is assigned to peat-mesh as the canonical Iroh consumer.
- **peat-node-side `Cipher` plumb-through** — peat-node already ships `StoreCipher` (AES-256-GCM, FIPS-approved); a follow-up consumer PR will plumb that into peat-mesh's `AutomergeBackendConfig.cipher` to exercise ADR-060 Phase A end-to-end.
- **rPi-class perf evidence** for the AEAD + DH swap on ARM-without-crypto-extensions targets — [peat-mesh#126](https://github.com/defenseunicorns/peat-mesh/issues/126).
- **`blake3` → SHA-256** for identity fingerprinting in peat-btle — NodeId-derivation blast radius warrants its own design pass.

## [0.9.0-rc.10] - 2026-05-17

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.9` → `0.9.0-rc.10`. `peat-ffi` unchanged at `0.2.3` (no JNI ABI surface change). No wire-format change — the `IROH_DISTRIBUTION_COLLECTION` schema is identical to rc.9.

Moves the receive-side distribution lifecycle into peat-protocol, where it belongs. Before rc.10, peat-protocol owned only the *sender* side; every consumer that wanted delivery had to re-implement the receive loop (observe synced distribution documents, target-match, dedup, fetch the blob, write per-receiver `node_statuses` so the sender's progress watcher emits cross-peer frames, plus the deterministic test fault seam). peat-node carried that as an explicitly-stopgap `attachments/inbox.rs`. rc.10 lifts the whole orchestration upstream so every consumer gets identical, tested behavior and supplies only a thin sink. Closes the [peat-node#68](https://github.com/defenseunicorns/peat-node/issues/68) tracker.

### Added

- **`peat_protocol::storage::ReceiveSink`** — the per-consumer tail of the receive path: `already_delivered(&doc) -> bool` (durable restart-idempotency gate) and `deliver(&doc, blob_path)` (persist the fetched bytes). Everything orchestration-shaped is owned by peat-protocol.
- **`IrohFileDistribution::start_receive_watcher(own_short_id, sink, poll_interval)`** — spawns the receive watcher (aborted on drop, mirroring the sender-side watcher lifecycle). Polls synced distribution documents, skips self-originated distributions (via the in-memory `distributions` map), target-matches `own_short_id` against `target_nodes`, consults the sink's `already_delivered` gate, writes `Transferring`, fetches the blob, hands bytes to the sink, writes `Completed`. Transient fetch/deliver errors retry on the next sweep with no terminal `Failed` flip.
- **`peat_protocol::storage::{ReceiveTestDirective, set_receive_test_directive, clear_receive_test_directives}`** — the `#[doc(hidden)]` deterministic receive-path test fault seam (relocated from peat-node), used by PRD §Testing Plan tests 24 (`HoldInFlight`) and 29 (`FailFetch`). Not a supported library API.

### Verification

- `cargo clippy -p peat-protocol --features automerge-backend --all-targets` clean.
- `cargo test -p peat-protocol --features automerge-backend --lib` — 996/996 passed.
- End-to-end verified via a peat-node `[patch.crates-io]` path override before release: the full attachment suite (`attachments_e2e_test`, `attachments_deferred_test`, `attachments_multi_peer_test`, `attachments_acceptance_test`, `attachments_subscribe_test`, `attachments_smoke_test`) passes byte-for-byte two-peer delivery, the #864 cross-peer progress/terminal regression, and the relocated seam tests — no behaviour regression.

## [0.9.0-rc.9] - 2026-05-16

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.8` → `0.9.0-rc.9`. `peat-ffi` unchanged at `0.2.3` (no JNI ABI surface change). **Wire-format change** on `IROH_DISTRIBUTION_COLLECTION` documents — see Migration.

Closes the substrate-level half of [#864](https://github.com/defenseunicorns/peat/issues/864) that rc.7 and rc.8 left exposed under CI-runner scheduling: receivers' back-to-back writes to `node_statuses` (the `Transferring` → `Completed` sequence in peat-node's attachment inbox) could leave the receiver-local doc stuck at a stale state, because both writes targeted the same wholesale-scalar `ROOT.data` field on the Automerge document and Automerge's actor-id tiebreak under load-modify-write cycles could pick the wrong op when concurrent inbound sync delivered an older version of that same scalar.

The fix is a schema split: the sender's immutable metadata (distribution_id, blob_hash, scope, target_nodes, status, etc.) stays as a JSON byte-scalar at `ROOT.metadata`, but `node_statuses` moves out of the scalar entirely into a typed `ObjType::Map` at `ROOT.node_statuses`. Each receiver writes only to its own keyed entry (`peer.fmt_short()`) inside that map, so concurrent writes from different receivers target *different* Automerge fields and never compete; a single receiver's sequential writes (Transferring → Completed) replace the receiver's own key's prior value via the normal causally-ordered `put` semantics with no merge-tiebreak race.

### Changed

- **`IROH_DISTRIBUTION_COLLECTION` wire-format**: the distribution document is now structured Automerge (`ROOT.metadata` byte-scalar + `ROOT.node_statuses` typed Map) rather than a single `ROOT.data` JSON byte-scalar. The pre-rc.9 wholesale-scalar layout is still read-only-supported during an rc-cycle upgrade — a rc.9 node can read an rc.7/rc.8 doc, but rc.7/rc.8 nodes cannot read an rc.9-written doc. See Migration.
- **`IrohFileDistribution::store_distribution_document`** writes the typed Automerge structure directly via `AutomergeStore::put` instead of going through `Collection::upsert`'s wholesale-scalar shim.
- **`IrohFileDistribution::cancel`** does read-modify-write on `ROOT.metadata` only; the `ROOT.node_statuses` Automerge map is strictly preserved. Pre-rc.9 cancel could trample receiver-written `node_statuses` entries; rc.9 cancel cannot.
- **`watch_distribution_documents`** reads the typed structure via the new `read_distribution_document` helper.

### Added

- **`peat_protocol::storage::write_receiver_node_status(store, dist_id, receiver_short_id, status)`** — the public API peat-node's `attachments::inbox` will call on rc.9+ to record a receiver's `NodeTransferStatus` into the distribution document. Writes only to the receiver's own keyed entry in the `node_statuses` map; never touches the sender's metadata field.
- **`peat_protocol::storage::read_distribution_document(store, dist_id) -> Option<DistributionDocument>`** — reads the typed structure and reconstructs the in-memory `DistributionDocument`. Supports both rc.9+ typed schema and pre-rc.9 wholesale-scalar legacy reads (transparent during rc-cycle upgrade).
- **`peat_protocol::storage::scan_distribution_documents(store) -> Vec<(String, DistributionDocument)>`** — bulk read for consumers iterating the collection (peat-node's inbox watcher uses this instead of `Collection::scan` + manual deserialize on rc.9+).
- **Two new substrate-regression tests** in `tests/iroh_file_distribution_e2e.rs`:
  - `test_concurrent_receiver_writes_dont_collide` — pins that two receivers writing their own `node_statuses` entries against the same distribution document both survive (would fail deterministically on the pre-rc.9 wholesale-scalar schema).
  - `test_sequential_receiver_writes_converge_on_latest` — pins that a single receiver's Transferring → Completed sequence on the same key converges on Completed (catches a regression of the underlying merge-tiebreak race surfacing on the per-key map after a future refactor).

### Verification

- `cargo check --workspace --all-features` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test -p peat-protocol --features automerge-backend --lib` — 996/996 passed.
- `cargo test -p peat-protocol --features automerge-backend --test iroh_file_distribution_e2e` — 9/9 passed (5 pre-existing including the rc.7 watcher test, 2 cancel-path tests from rc.7, plus the 2 new substrate-regression tests above).

### Migration (BREAKING wire-format)

This is an rc-cycle wire-format break on `IROH_DISTRIBUTION_COLLECTION`. Pre-rc.9 nodes that sync with a rc.9 node will not be able to deserialize distribution documents rc.9 wrote. Consumers should upgrade peat-protocol consumers together; the dual-read support in `read_distribution_document` covers the transient case where a rc.9 node sees a pre-rc.9 doc that synced before everyone upgraded, but the reverse direction is not supported.

Downstream consumers depending on `peat-protocol = "0.9.0-rc.8"` should bump to `"0.9.0-rc.9"` and switch reader/writer call sites:

- **Reading distribution docs**: replace `collection.get(&dist_id)` + `serde_json::from_slice::<DistributionDocument>` with `peat_protocol::storage::read_distribution_document(&store, &dist_id)`.
- **Scanning the collection**: replace `collection.scan()` + per-entry deserialize with `peat_protocol::storage::scan_distribution_documents(&store)`.
- **Writing receiver status**: replace any direct write into `DistributionDocument::node_statuses` followed by `collection.upsert` with `peat_protocol::storage::write_receiver_node_status(&store, &dist_id, receiver_short_id, &status)`.

### Unblocks

- [defenseunicorns/peat-node#76 follow-up](https://github.com/defenseunicorns/peat-node) — peat-node's `attachments::inbox::write_node_status` switches to the new typed peat-protocol API, both deferred PRD-006 tests un-ignore (`subscribe_emits_progress_then_terminal` and `receiver_writes_node_status_into_distribution_doc`).

## [0.9.0-rc.8] - 2026-05-16

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.7` → `0.9.0-rc.8`. `peat-ffi` unchanged at `0.2.3` (no JNI ABI surface change; the new behavior reaches FFI consumers transitively via the workspace's path dependency on `peat-protocol`).

Picks up [`peat-mesh 0.9.0-rc.10`](https://crates.io/crates/peat-mesh/0.9.0-rc.10), which ships the [peat-mesh#118](https://github.com/defenseunicorns/peat-mesh/pull/118) sync_cooldown defer-not-drop fix — closing the substrate-level race that left rc.7's `subscribe_progress` wire-up only half-working end-to-end. Receivers' back-to-back `Transferring → Completed` writes into the distribution doc no longer get silently dropped by the auto-sync push's 100ms `(peer, doc)` cooldown, so the sender's broadcast watcher now observes the full `Transferring → Completed` transition and emits both the IN_PROGRESS frame and the terminal frame to `subscribe_progress` subscribers for real cross-peer transfers.

Closes [defenseunicorns/peat#864](https://github.com/defenseunicorns/peat/issues/864) end-to-end. rc.7 landed the wire-up; rc.10's substrate fix closes the missing-terminal-frame half.

### Changed

- **`peat-mesh` range floor**: `>=0.9.0-rc.8` → `>=0.9.0-rc.10` in `workspace.dependencies` (`Cargo.toml`). Upper bound `<0.9.1` unchanged — the wire-shape protection at the patch line stands. The intervening `peat-mesh 0.9.0-rc.9` (the [#116 fix](https://github.com/defenseunicorns/peat-mesh/pull/116) for the [#115 receive-path notify loop](https://github.com/defenseunicorns/peat-mesh/issues/115) + `AutomergeBackendConfig` BREAKING) was a no-op for this workspace; the floor moves to rc.10 because rc.10's source-level change in `AutomergeSyncCoordinator::initiate_sync` + `sync_document_with_all_peers` is what we materially depend on.
- **`peat-protocol`'s `=peat-schema` pin** bumped from `0.9.0-rc.7` to `0.9.0-rc.8` to match the workspace's new floor.

### Verification

- `cargo update -p peat-mesh` resolves `peat-mesh 0.9.0-rc.10` from crates.io, no git fetches.
- **Transitive lockfile shift**: `cargo update -p peat-mesh` re-resolved `prost-build`'s and `prost-derive`'s `itertools` selection from `0.14.0` → `0.10.5`. Both versions were already in the lockfile pre-PR; this is a transitive *selection* change, not a new-crate introduction. `prost-build` is build-time only (proto codegen) and runtime behavior is unaffected. Logged here so a future bisection on `Cargo.lock` drift in the rc.7→rc.8 window has the explicit explanation.
- `cargo check --workspace --all-features` clean.
- `cargo test -p peat-protocol --features automerge-backend --lib` — full suite green (the rc.7-era e2e tests remain green; no source change to peat-protocol in this release).
- End-to-end confirmation (run on the rc.10 fix branch via `[patch.crates-io]` from peat-node, identical to what rc.10 just shipped to crates.io): PRD-006 test 23 (`subscribe_emits_progress_then_terminal`) passes in 5 seconds with exactly 1 IN_PROGRESS frame and exactly 1 terminal frame for a 4 MiB two-peer transfer. peat-node's new receiver-local doc-state regression test (`receiver_writes_node_status_into_distribution_doc`) also passes deterministically.

### Migration

No consumer-visible API change. Downstream consumers depending on `peat-protocol = "0.9.0-rc.7"` should bump to `"0.9.0-rc.8"`. The substrate timing-contract change (`AutomergeSyncCoordinator::initiate_sync` blocks up to `sync_cooldown` instead of fast-failing) propagates from peat-mesh; no in-workspace consumer relied on the prior fast-fail behavior. See peat-mesh's `[0.9.0-rc.10]` CHANGELOG entry for the substrate-side contract details.

### Unblocks

- [peat-node#76](https://github.com/defenseunicorns/peat-node/pull/76): bumps `peat-protocol >=0.9.0-rc.8`, un-ignores PRD-006 test 23 (`subscribe_emits_progress_then_terminal`) and the new receiver-side doc-state regression test, ships the receiver-side `attachments::inbox` writes that complete the contract.

## [0.9.0-rc.7] - 2026-05-15

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.6` → `0.9.0-rc.7`. `peat-ffi` unchanged at `0.2.3` (no JNI ABI surface change; the new behavior reaches FFI consumers transitively via the workspace's path dependency on `peat-protocol`).

Closes the [#864](https://github.com/defenseunicorns/peat/issues/864) contract gap where `IrohFileDistribution::subscribe_progress` returned a broadcast receiver that observed zero frames for the lifetime of every distribution. Lands the receiver-side observer (Design A from the issue): the sender's `IrohFileDistribution` now spawns a background watcher subscribed to `AutomergeStore::subscribe_to_observer_changes`, re-reads the distribution document on receiver-written `node_statuses` updates, and publishes a fresh `DistributionStatus` to the broadcast channel. Terminal frame emitted on `cancel()`, then channel dropped so subscribers observe `RecvError::Closed`.

This unblocks [peat-node#75](https://github.com/defenseunicorns/peat-node/pull/75), which wires the receiver-side write into `attachments::inbox` and un-ignores the two deferred PRD-006 tests (`subscribe_emits_progress_then_terminal`, `cancel_in_flight_stops_transfer`).

### Added

- **`pub DistributionDocument`** struct in `peat-protocol::storage::file_distribution` — typed schema for the `file_distributions` collection with `#[serde(default)] node_statuses: HashMap<String, NodeTransferStatus>`. Replaces the inline `serde_json::json!` doc shape; `#[serde(default)]` preserves wire compatibility with pre-rc.7 documents.
- **`pub const IROH_DISTRIBUTION_COLLECTION`** — collection name promoted from module-private to public so consumers (e.g. peat-node's inbox watcher) can address the same Automerge collection the sender uses.
- **Three new `iroh_file_distribution_e2e` tests**: `test_cancel_emits_terminal_frame_then_closes`, `test_cancel_preserves_distribution_document_fields`, `test_watcher_publishes_frame_on_receiver_node_status_write`.
- **Two new in-module tests**: `test_distribution_document_round_trip`, `test_distribution_document_legacy_compat` (asserts a freshly-serialized doc with `node_statuses` stripped still deserializes — pre-rc.7 wire format survives upgrade).

### Changed

- **`IrohFileDistribution::cancel`** now calls `broadcast_progress` with the terminal `Cancelled` status and drops the broadcast sender. Previously `cancel` overwrote the distribution doc wholesale with a `{status, cancelled_at}` stub; it now does a read-modify-write on the typed `DistributionDocument`, preserving all other fields (sender, targets, file metadata, `node_statuses`).
- **`broadcast_progress`** no longer `#[allow(dead_code)]` — callers exist (cancel path + the new receiver-side watcher).

### Verification

- `cargo test -p peat-protocol --features automerge-backend --test iroh_file_distribution_e2e` — 7/7 (including the 3 new tests).
- `cargo test -p peat-protocol --features automerge-backend --lib file_distribution` — 7/7 (including the 2 new tests).
- `cargo test -p peat-protocol --features automerge-backend --lib` — 996/996.
- `cargo check --workspace --all-features` — clean.

### Migration

No source changes required for consumers depending on `peat-protocol = "0.9.0-rc.6"` — bump the pin to `"0.9.0-rc.7"`. The two new `pub` symbols (`DistributionDocument`, `IROH_DISTRIBUTION_COLLECTION`) are additive; consumers that want to participate in the receiver-side write protocol (i.e. signal `NodeTransferStatus` back to senders so their `subscribe_progress` consumers see real frames) can now import them. Consumers of `subscribe_progress` that did nothing receiver-side will continue to see zero frames until both sides are on rc.7+.

### Open

- **Concurrent writes** on the distribution doc accepted as-is for v1. `AutomergeCollection::upsert` reads-replaces the JSON `"data"` scalar wholesale, so simultaneous sender + receiver writes race; the watcher reconciles on the next observer event and Automerge convergence guarantees no permanent loss. A proper per-key Automerge-map shape on `node_statuses` is deferred — separate refactor of `AutomergeCollection`.
- **IN_PROGRESS heartbeats** skipped for v1. One `Transferring` frame at fetch-start (written by the receiver in peat-node#75) satisfies PRD-006 test 23's "≥1 IN_PROGRESS frame" assertion. Byte-level progress remains available via the existing `FnMut(BlobProgress)` seam in `fetch_blob`.

## [0.9.0-rc.6] - 2026-05-12

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.5` → `0.9.0-rc.6`. `peat-ffi` bumps independently `0.2.2` → `0.2.3` (patch: pulls in `peat-mesh 0.9.0-rc.8` via the crates.io-resolved dep, no ABI surface change).

Workspace cuts over from `[patch.crates-io]` git-rev pins to the actual crates.io releases. The Slice-4 rc-cycle that motivated the git overrides is closed:
[peat-mesh 0.9.0-rc.8](https://crates.io/crates/peat-mesh/0.9.0-rc.8) (AutomergeBackend turnkey adapter + #107/#108/#109 sync coordinator fix train + relay-default tightening) and
[peat-btle 0.4.0](https://crates.io/crates/peat-btle/0.4.0) (Slice-4.b cycle break — deletes `mesh-translator` Cargo feature + optional `peat-mesh` dep)
both shipped to crates.io on 2026-05-12 in lockstep, and the workspace now consumes them directly.

### Changed

- **Removed `[patch.crates-io]` git overrides for `peat-mesh` and `peat-btle`.** The 40-line ADR-059 Amendment 4 Slice 4.d comment block + the two rev-pinned git URLs in `Cargo.toml` are gone. The replacement comment block documents the cutover and links to the upstream releases. Resolution is now via the workspace.dependencies pins alone:
  - `peat-mesh = ">=0.9.0-rc.8, <0.9.1"` — floor bumped from `rc.5` → `rc.8`.
  - `peat-btle = ">=0.4.0, <0.4.1"` — unchanged range; now resolves cleanly without the override.
- **`Cargo.lock` flips peat-mesh from git source to registry source.** Diff visible in `git diff Cargo.lock` — the `source = "git+https://...peat-mesh?rev=..."` line becomes `source = "registry+https://github.com/rust-lang/crates.io-index"`. Same logical commit as the prior git pin (`0d15467` is the merge commit of #110 which became `v0.9.0-rc.8`).
- **`peat-protocol`'s `=peat-schema` pin bumped** from `0.9.0-rc.5` to `0.9.0-rc.6` to match the workspace's new floor.

### Verification

- `cargo update -p peat-mesh -p peat-btle` resolves `peat-mesh 0.9.0-rc.8` + `peat-btle 0.4.0` from crates.io, no git fetches.
- `cargo check --workspace --exclude peat-ffi` clean.
- `cargo check -p peat-ffi --features bluetooth` clean (Android target-cfg deps unchanged).
- 3-device bench validated 4 scenarios green against the rc.5 cross-compiled `.so` (same logical commit as the new rc.8 dep resolution); no runtime behavior change expected, just a dependency-resolution housekeeping pass.

### Migration

No consumer-visible action required. Downstream consumers depending on `peat-protocol = "0.9.0-rc.5"` should bump to `"0.9.0-rc.6"`; those depending on `peat-ffi = "0.2.2"` should bump to `"0.2.3"`. No public API surface change in either crate.

The git-override removal also unblocks downstream consumers that hit cargo's structural cyclic-dep detection on the old peat-mesh 0.9.0-rc.7 / peat-btle 0.3.4-rc.5 combination — they can now pin to the new floor and resolve cleanly without their own `[patch.crates-io]` workaround.

## [0.9.0-rc.5] - 2026-05-11

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.4` → `0.9.0-rc.5`. `peat-ffi` bumps independently `0.2.1` → `0.2.2` (patch: non-breaking observability addition; no ABI surface change).

### Changed

- **`peat-mesh` patch pin:** `[patch.crates-io] peat-mesh` rev bumped from `9a92e7ca` (post-#106; the workspace skipped the interim `1df89a01` post-#108 mainline since rc.4's release cadence pre-dated those PRs) to `0d15467` (post-#109). The new revision pulls in three cumulative sync-coordinator fixes validated end-to-end on the 3-device bench:
  - **peat-mesh#107** — `categorize_sync_error` deduplicated. The inline categorizer at `sync_documents_batch` mirrored the one at `initiate_sync` but had drifted; refactored into a free function with shared pattern table so a Network-class transient stops getting silently categorized as Document and burning the retry budget.
  - **peat-mesh#108** — `get_alive_connection` pre-check before `open_bi`. `transport.get_or_connect` could return a cached iroh `Connection` whose `close_reason()` was already set; the subsequent `open_bi` would fail, but the categorizer's Network → Network classification wasted a circuit-breaker failure slot per cached-dead-peer attempt. The helper checks `close_reason()` before opening the stream and bails with a phrase the categorizer recognizes as Network.
  - **peat-mesh#109** — circuit-breaker one-way trap. `is_circuit_open` was pure-read and ignored `open_timeout`, so once tripped the breaker stayed blocking indefinitely. New `should_block_sync` gate respects the timeout and performs Open → HalfOpen transition. The shared `try_half_open_transition` helper is used by both `handle_error` and `should_block_sync` so the state-machine semantics stay symmetric. Both production call sites (`initiate_sync`, `sync_documents_batch`) now route through `should_block_sync` and through `anyhow::Error::from(SyncError::CircuitBreakerOpen)` for typed downcastability.

### Added

- **`peat-ffi` Android tracing bridge:** `init_android_tracing()` invoked from `JNI_OnLoad` installs a `tracing-subscriber` whose writer routes every event ≥ INFO to logcat under the `PeatRust` tag. peat-mesh and peat-protocol emit per-document sync results, transport-layer warnings, and other diagnostics via `tracing::warn!` / `tracing::info!` — without a subscriber installed those events go nowhere on Android, which is how the silent-sync regression that motivated peat-mesh#107/#108/#109 went undiagnosed until peat-ffi `request_sync` got its own `android_log` (`PeatFFI` tag, peat#848). Idempotent via `OnceLock`; level override via `PEAT_TRACING_LEVEL` env var. Closes the observability arm of peat#850.

## [0.9.0-rc.4] - 2026-05-11

### Changed

- **Release workflow:** `Publish peat-protocol` step now treats "peat-btle dep version not on crates.io" as a graceful skip rather than a hard failure. peat-protocol depends on peat-btle 0.4.0 via the workspace, but peat-btle 0.4.0 is held back from crates.io until the Slice-4.b UAT train ships (peat#828). Without this guard, every release tag's `cargo publish` step failed with `failed to select a version for the requirement \`peat-btle = …\`` even though all other release artifacts (tag, GitHub release, SBOM, peat-schema publish) produced cleanly. The step now logs a `::warning::` explaining the skip and the release proceeds. Once peat-btle 0.4.0 lands on crates.io, the resolver finds a match and the publish runs normally.

## [0.9.0-rc.3] - 2026-05-11

> **Crate-level versions in this release**: workspace bumps `0.9.0-rc.2` → `0.9.0-rc.3`. `peat-ffi` bumps independently `0.2.0` → `0.2.1` (patch: non-breaking observability addition; no ABI surface change).

### Added

- `peat-ffi` `PeatNode::request_sync` now emits Android `__android_log_write` events at INFO covering every invocation: a starting line with the connected-peer count, a per-peer push success / failure line, and a complete line. Closes the observability gap surfaced by the 2026-05-10 bench session — peat-mesh's internal `sync_document_with_peer` failures log via `tracing::warn!` which doesn't reach logcat (no tracing-subscriber installed on Android), so previously `request_sync` could return `Ok(())` while every per-doc push silently failed. The FFI-boundary log makes those silent failures visible at the layer where `android_log` is wired up. Logcat tag: `PeatFFI`, message pattern: `request_sync: starting with N connected peer(s)` / `request_sync: pushed to peer <16-hex-prefix>` / `request_sync: FAILED for peer <16-hex-prefix>: <err>` / `request_sync: complete (N peer(s) attempted)`.

## [0.9.0-rc.2] - 2026-05-10

> **Crate-level versions in this release**:
> `peat`, `peat-protocol`, `peat-schema`, `peat-discovery`, `peat-persistence`, `peat-transport` (and the workspace example crates) inherit `workspace.package.version = "0.9.0-rc.2"`. `peat-ffi` carries an independent `version = "0.2.0"` (bumped from `0.1.0`) — its semver surface is the JNI ABI + UniFFI binding shape, which moves on a different cadence than the workspace's CHANGELOG-tracked protocol surface. The JNI rename below is the breaking change driving the bump.

### Changed

- **BREAKING (FFI ABI):** all `peat-ffi` JNI extern symbols renamed
  from `Java_com_defenseunicorns_atak_peat_PeatJni_*` to
  `Java_com_defenseunicorns_peat_PeatJni_*` (103 sites). Callback
  interface class lookups in `JNI_OnLoad` and `notify_peer_event`
  similarly migrated (`PeatJni`, `PeerEventManager`,
  `DocumentChangeListener`, `OutboundFrameListener`). Kotlin
  consumers MUST move their `PeatJni` class from package
  `com.defenseunicorns.atak.peat` to `com.defenseunicorns.peat` for
  `RegisterNatives` to resolve; without that migration, deploying
  the new `.so` to a tablet running the old plugin Kotlin crashes
  on native load. See peat#846 for cross-repo coordination + the
  rationale (peat is the generic mesh substrate; consumers live in
  sibling repos, so peat's identifiers should not name a specific
  consumer). PR #845.
- **BREAKING (default behavior):** `peat-protocol` no longer enables
  n0's hosted iroh relay pool or DNS pkarr discovery by default. All
  `IrohTransport` constructors now use `Endpoint::empty_builder()`
  instead of `iroh::endpoint::presets::N0`, so default builds never
  reach `*.iroh.network` or `*.iroh-canary.iroh.link`. Cross-internet
  hole-punching that implicitly relied on n0 relays will fail in
  default builds; LAN/mDNS and direct addresses continue to work. See
  issue #833.
- Generic-vocabulary scrub across code, examples, READMEs,
  operational docs, and the `Makefile`: vendor-specific consumer
  names (ATAK, WinTAK, iTAK, WearTAK) replaced with generic terms
  ("consumer plugin", "CoT consumer", "constrained wearable", etc.).
  Make targets `demo-atak`/`configure-atak`/`start-atak`/`stop-atak`/
  `build-atak-plugin`/`deploy-atak-plugin` →
  `demo-consumer`/`configure-consumer`/`start-consumer`/`stop-consumer`/
  `build-consumer-plugin`/`deploy-consumer-plugin`. Make variables
  `ATAK_PACKAGE`/`ATAK_JAVA_HOME` → `CONSUMER_PACKAGE`/`CONSUMER_JAVA_HOME`.
  Example file `peat_tak_client.rs` → `peat_cot_client.rs`. PR #845.
- Connection-recycle dedup logic in `AutomergeBackend::start_sync`
  refactored: 5s per-peer dedup window for duplicate `Connected`
  events (connect-path / accept-path race), with `Disconnected`
  events clearing the dedup entry so legitimate fast reconnects
  (BLE flap, force-stop+restart) flow through unrestricted.
  Extracted as `AutomergeBackend::check_and_record_connect`
  testable helper with 7 regression tests in `connected_dedup_tests`.
  PR #845.

### Added

- `peat-ffi`: `MarkerInfo` UniFFI Record gains a `deleted: bool`
  field (wire key `_deleted: true`). `parse_marker_publish_json`
  accepts stripped tombstone bodies (uid + `_deleted: true` + ts, no
  geo required); `serialize_marker_json` and
  `serialize_markers_get_json` emit the sentinel only when set.
  Soft-delete sentinel pattern lets deletes propagate via the
  Updated channel (peat-mesh fan-out skips `ChangeEvent::Removed`
  in Slice 1; Slice 2 enables real removal). PR #845.
- `peat-ffi`: new JNI extern fns
  `Java_com_defenseunicorns_peat_PeatJni_publishMarkerJni` and
  `…_getMarkersJni`, registered in both `nativeInit` and
  `JNI_OnLoad`. Mirrors the existing `publishPlatformJni` /
  `getPlatformsJni` shape. PR #845.
- `peat-protocol`: `tracing::info!` on success of the post-Connected
  `sync_all_documents_with_peer` proactive push (was failure-only),
  closing an observability gap that made silently-failing pushes
  indistinguishable from successful ones. PR #845.
- `TOMBSTONE_PLACEHOLDER_TYPE` constant extracted from
  `parse_marker_publish_json`. Documented inline why the tombstone
  body uses a placeholder CoT type (`a-u-G`). PR #845.
- `getMarkersJni` storage-error logging via `android_log` (was
  silently returning `"[]"` on storage error — indistinguishable
  from "no markers"). Mirrors the publish-side log shape. PR #845.
- 14 new unit tests for the marker tombstone surface (`tests::
  marker_tombstone` + `tests::marker_tests` in `peat-ffi`) and 7
  for the dedup window (`connected_dedup_tests` in `peat-protocol`),
  including a `marker_tombstone_publish_reaches_lite_bridge_sink_with_deleted_flag`
  fanout test that verifies the `_deleted: true` flag survives the
  BLE wire round-trip. PR #845.
- `peat-protocol` cargo feature `relay-n0-hosted` (off by default)
  as a grep-able opt-in escape hatch that restores the previous
  n0-hosted relay-pool behavior. Build with
  `--features relay-n0-hosted` to re-enable n0's hosted relay pool
  and DNS discovery.
- `CLAUDE.md` + `SKILL.md`: hard rule banning consumer-specific
  references in the peat repo, including a verification grep gate
  (`git diff main -- ':!docs/adr' ':!docs/whitepaper' ':!CLAUDE.md'
  ':!SKILL.md' | grep -E '^\+' | grep -iE '\b(atak|wintak|itak|weartak)\b'
  | grep -vE 'peat-atak-plugin|com\.atakmap|atakmap\.app|ATAKActivity'`)
  that must be empty before merge. PR #845.

### Removed

- Vendor-specific identifiers from peat repo: `peat-ffi/examples/
  peat_tak_client.rs` (renamed), `Java_com_defenseunicorns_atak_peat_*`
  JNI symbols (renamed), `com.defenseunicorns.atak.peat.*` package
  references in `examples/android-ble-test` (Kotlin files moved to
  `com.defenseunicorns.peat`). See peat#846 for the full cross-repo
  rationale and the matching CLAUDE.md / SKILL.md rule + verification
  grep gate. PR #845.

## [0.9.0-rc.1] - 2026-04-23

First public release candidate for the Peat workspace. Published to
crates.io so downstream integrators (peat-sim, peat-atak-plugin, future
SDK consumers) can depend on a single crate — `peat-protocol` — which
re-exports `peat-schema` and `peat-mesh`.

### Added

- `peat-protocol` as the public facade for the Peat stack. It re-exports
  `peat_mesh` and `peat_schema`, so downstream consumers depend on one
  crate:

  ```toml
  peat-protocol = "=0.9.0-rc.1"
  ```

- `peat-schema` published as a standalone crate for consumers that need
  the Protobuf types without the full protocol layer.

- `CHANGELOG.md` at the repository root (this file).

- `docs/RELEASING.md` describing the release process.

### Changed

- Workspace version unified at `0.9.0-rc.1` to track the underlying
  `peat-mesh` release candidate.

- `peat-protocol` → `peat-schema` path dep now carries an explicit
  version (`=0.9.0-rc.1`) so it resolves on crates.io.

### Pinned

- `peat-mesh = "=0.9.0-rc.1"` at the workspace level with
  `default-features = false`. Each consumer opts in to the backend it
  needs (peat-protocol's `automerge-backend` feature pulls
  `peat-mesh/automerge-backend` explicitly). This preserves the
  pre-0.9.0 behavior for size-constrained / lite-transport builds,
  which would otherwise silently pull `automerge`, `iroh-blobs`,
  `redb`, and `negentropy` via the new peat-mesh default features.

### Ecosystem alignment

This release aligns with:

- `peat-mesh` 0.9.0-rc.1 on crates.io
- `peat-node`, `peat-registry`, `peat-gateway` pinned to
  `peat-mesh = "=0.9.0-rc.1"` — see
  [peat-node#21](https://github.com/defenseunicorns/peat-node/pull/21),
  [peat-registry#128](https://github.com/defenseunicorns/peat-registry/pull/128),
  [peat-gateway#86](https://github.com/defenseunicorns/peat-gateway/pull/86)
