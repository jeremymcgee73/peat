# Changelog

All notable changes to the Peat workspace are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog covers the crates published to crates.io from this workspace:

- `peat-protocol` — public facade; depends on `peat-schema` and `peat-mesh`
- `peat-schema` — wire format (Protobuf) definitions

Sub-crates that stay internal (`peat-transport`, `peat-persistence`, `peat-discovery`, `peat-ffi`, `examples/*`) share the workspace version but are not published and are not documented here.

## [Unreleased]

### Changed

- **BREAKING (default behavior):** `peat-protocol` no longer enables n0's
  hosted iroh relay pool or DNS pkarr discovery by default. All
  `IrohTransport` constructors now use `Endpoint::empty_builder()`
  instead of `iroh::endpoint::presets::N0`, so default builds never
  reach `*.iroh.network` or `*.iroh-canary.iroh.link`. Cross-internet
  hole-punching that implicitly relied on n0 relays will fail in
  default builds; LAN/mDNS and direct addresses continue to work. See
  issue #833.

### Added

- `peat-protocol` cargo feature `relay-n0-hosted` (off by default) as a
  grep-able opt-in escape hatch that restores the previous behavior.
  Build with `--features relay-n0-hosted` to re-enable n0's hosted
  relay pool and DNS discovery.

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
