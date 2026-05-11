# Changelog

All notable changes to the Peat workspace are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog covers the crates published to crates.io from this workspace:

- `peat-protocol` — public facade; depends on `peat-schema` and `peat-mesh`
- `peat-schema` — wire format (Protobuf) definitions

Sub-crates that stay internal (`peat-transport`, `peat-persistence`, `peat-discovery`, `peat-ffi`, `examples/*`) share the workspace version but are not published and are not documented here.

## [Unreleased]

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
