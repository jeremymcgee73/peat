# Changelog

All notable changes to the Peat workspace are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog covers the crates published to crates.io from this workspace:

- `peat-protocol` — public facade; depends on `peat-schema` and `peat-mesh`
- `peat-schema` — wire format (Protobuf) definitions

Sub-crates that stay internal (`peat-transport`, `peat-persistence`, `peat-discovery`, `peat-ffi`, `examples/*`) share the workspace version but are not published and are not documented here.

## [Unreleased]

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
