# ADR-062: Consolidate Iroh Transport in peat-mesh

**Status**: Proposed
**Date**: 2026-05-23
**Authors**: Kit Plummer
**Related**: ADR-011 (Automerge + Iroh backend — selects iroh as the QUIC transport; this ADR records *where* iroh integration code lives, not whether to use it), ADR-017 §Layer 1 "Network Transport" (which crate owns transport), peat#898 (mDNS-consolidation epic — Phase 1 done as peat#900, Phase 2 is this ADR's territory), peat#918 (this issue)
**Triggered by**: peat-mesh#152 / peat#913 (interface-filter helper landed in peat-mesh; peat-protocol's `IrohTransport` consumed it as the first cross-repo seam) — surfaced that peat-protocol still owns ~1700 lines of iroh transport code that structurally belongs in peat-mesh per `peat/SKILL.md`'s transport-agnosticism rule

---

## Context

`peat/SKILL.md` § Hard invariants — Transport agnosticism:

> Peat protocol logic must not assume a transport. BLE, mesh, IP, serial are all interchangeable. Transport-specific code stays in transport repos (`peat-btle`, `peat-mesh`) or the `peat-transport` workspace subcrate — never in core / protocol / persistence layers or non-transport sibling repos.

Despite the rule, `peat-protocol` currently owns substantial iroh-specific code:

| File | Surface | Lines |
|---|---|---|
| `peat-protocol/src/network/iroh_transport.rs::IrohTransport` | iroh `Endpoint` wrapper: 32 public methods (constructors, accept loop, connect_peer/connect_by_id, peer-event broadcaster, connection registry, recycle helper, tactical QUIC config) | ~1700 |
| `peat-protocol/src/storage/automerge_backend.rs:1289-1322` | Spawns the connection-recycling task that calls `transport.recycle_old_connections(max_age)` | ~33 |
| `peat-protocol/src/transport/iroh.rs::IrohMeshTransport` | iroh impl of `MeshTransport` trait (which already lives in `peat_mesh::transport` per `peat-protocol/src/transport/mod.rs:5`'s own doc — "*Core transport types and traits are defined in `peat_mesh::transport` and re-exported here for backwards compatibility. **Backend-specific implementations (Iroh) remain in this crate**.*"). The "remain in this crate" clause is the violation this ADR closes. | ~900 |
| `peat-protocol/Cargo.toml` | Direct deps on `iroh = "0.97"`, `iroh-blobs = "0.99"` | n/a |

The mDNS-consolidation epic (peat#898) handled the discovery half of this in two phases: Phase 1 (peat#900) deleted three divergent `MdnsDiscovery` implementations down to peat-mesh's canonical version; Phase 3 (peat#919, peat#920) retired the residual `peat-discovery` subcrate. Phase 2 — the iroh transport half — is the remaining structural debt and the territory of this ADR.

`peat-mesh` already owns a sizable iroh transport surface: `NetworkedIrohBlobStore` with `build_endpoint_with_hooks` (the central endpoint construction, including the interface filter from peat-mesh#152), `MeshSyncTransport` (sync-protocol-aware transport layer), `FormationEndpointHooks` (formation-peer-gated accept hook), and the connection lifecycle inside the `iroh::Router` it spawns. The two crates' iroh surfaces have **substantial functional overlap** — endpoint construction (duplicated), connection registry (duplicated), peer-event broadcasting (in different shapes), recycle helper (only in peat-protocol).

## Decision

**Delete `peat-protocol/src/network/iroh_transport.rs::IrohTransport` entirely.** Rewire peat-protocol's `AutomergeBackend`, `transport/iroh.rs` (`IrohMeshTransport`), and `ffi/peer.rs` to consume peat-mesh's transport surface directly. Drop `iroh` and `iroh-blobs` as direct deps in `peat-protocol/Cargo.toml`; they remain transitive via peat-mesh.

The replacement does **not** involve moving `IrohTransport` as-is into peat-mesh. peat-mesh already has the necessary primitives — the consolidation is a one-way unwinding of peat-protocol's parallel implementation.

### What peat-mesh exposes (new public surface)

To support the call sites peat-protocol currently has, peat-mesh adds a small layer of new public types and functions, all in `peat_mesh::network` (a new module) or as additions to existing modules:

1. **`peat_mesh::network::TransportPeerEvent`** — the typed peer-event enum (`Connected { endpoint_id, .. }`, `Disconnected { endpoint_id, reason }`, etc.). Currently lives at `peat_protocol::network::iroh_transport::TransportPeerEvent`. Moves to peat-mesh.

2. **`peat_mesh::network::iroh_transport::IrohTransport`** — a thin façade owning the iroh `Endpoint` and (a single) connection registry, exposing the actually-used surface (`connect_peer`, `connect_by_id`, `subscribe_peer_events`, `emit_peer_connected`, `disconnect`, `endpoint_id`, `endpoint_addr`, `bound_socket_addr_string`, `peer_count`, `connected_peers`). Implemented by composing `NetworkedIrohBlobStore` (already does endpoint construction + accept-loop + formation gating) with a small connection-tracker + event-broadcaster on top.

3. **`peat_mesh::network::iroh_transport::from_seed_at_addr(seed, bind_addr)`** etc. — deterministic-seed constructors. Currently use peat-protocol's `peat-iroh-key-v1:` domain separator on SHA-256 of the seed (`peat-protocol/src/network/iroh_transport.rs:402-411`): `SHA-256("peat-iroh-key-v1:" || seed) → 32-byte secret → iroh::SecretKey::from_bytes`. The move **replaces this with HKDF-SHA-256** at the same time — see the "FIPS KDF gate" in the acceptance criteria. NodeId derivation will change as a consequence: every `from_seed*` caller using the same `seed` string post-migration produces a different NodeId than pre-migration, so this is a wire-visible change for any deployment that re-uses seeds across the rc.14 boundary. The CHANGELOG entry must call this out as a breaking change at the NodeId layer (operators of pinned-NodeId deployments need to either accept new NodeIds or stick to pre-rc.14 binaries during the migration window).

4. **`peat_mesh::network::iroh_transport::{CAP_AUTOMERGE_ALPN, QUIC_MAX_IDLE_TIMEOUT_SECS, QUIC_KEEP_ALIVE_INTERVAL_SECS, CONNECTION_RECYCLE_INTERVAL_SECS, CONNECTION_RECYCLE_ENV, connection_recycle_interval_secs}`** — public constants and the env-var resolver from peat#912.

5. **The connection-recycling task spawn moves into peat-mesh's `IrohTransport::start_recycler()`** (or equivalent lifecycle method), invoked from `AutomergeBackend::start_sync`'s peat-mesh-side replacement. peat-protocol's `automerge_backend.rs:1289` block goes away.

### What peat-protocol still owns

- The **formation handshake** (`peat-protocol/src/network/formation_handshake.rs::perform_initiator_handshake`). This is a protocol-level concern (Ed25519 challenge, formation-key verification), not transport. peat-mesh's `IrohTransport::connect_*` returns the raw `iroh::Connection`; the handshake runs on the caller side after connect.
- The **sync coordinator** (`peat-protocol/src/sync/automerge.rs`). The sync loop, `peer_discovery()`, the `PeerAvailabilityCheck` gates added in peat#874/#917 — all stay. They just call peat-mesh's transport surface instead of peat-protocol's.
- The **`AutomergeIrohBackend`** trait adapter at `sync/automerge.rs:1240`. Inner `AutomergeBackend` still drives sync; the adapter is unchanged.

### What's deleted

- `peat-protocol/src/network/iroh_transport.rs` — ~1700 lines.
- `peat-protocol/src/transport/iroh.rs::IrohMeshTransport` — ~900 lines, deleted entirely. The `MeshTransport` *trait* already lives in `peat_mesh::transport` (verified at `peat-protocol/src/transport/mod.rs:5`); only the iroh *impl* sits in peat-protocol, which is exactly the violation this ADR closes. Same reasoning peat-mesh#152 used when it placed the new `interface_filter` module in peat-mesh rather than peat-protocol: backend-specific impls of transport-layer traits belong with the trait.

  **Consumers of `IrohMeshTransport` to migrate or retire:** one integration test (`peat-protocol/tests/dual_active_simultaneous.rs`, three `IrohMeshTransport::new` call sites) is the only in-repo dependency. The test exercises a peat-mesh-level invariant (dual simultaneous connection establishment with conflict resolution) that survives in peat-mesh; the test moves to peat-mesh's test suite alongside the impl. No production code, no examples, no `peat-protocol::ffi` callers depend on `IrohMeshTransport`.
- `iroh`, `iroh-blobs`, and `ed25519` / `pkcs8` workarounds in `peat-protocol/Cargo.toml`'s direct-deps section (they stay transitive via peat-mesh).

## Consequences

**Positive:**

- Single iroh code path in the ecosystem. iroh version bumps, QUIC tuning, per-OS bind quirks, NAT-traversal config — one place, not two.
- `peat-protocol/Cargo.toml` shrinks; the protocol layer is no longer an iroh consumer.
- The architectural debt that peat#890 (interface filter) and peat#892 (recycler env var) papered over is now actually fixed. Those bugs are user-side closed but the structural-violation half stays open until this ADR's work lands.
- Future transport-layer fixes (e.g., a third site for the breaker gate found later) live in one place by construction.

**Negative / cost:**

- This is a **multi-PR cross-repo change** with a release in the middle. The chain is: peat-mesh PR adds the new public surface → peat-mesh release rc.18 → peat workspace floor bump + peat consumer-side PR → peat release rc.14. Per peat#864's gotcha, develop under `[patch.crates-io]` overrides and run the release chain only after end-to-end verification.
- 30+ call sites in `peat-protocol/src/sync/automerge.rs`, plus `transport/iroh.rs`, `ffi/peer.rs`, `peat-quickstart/src/main.rs`, and ~5 integration tests, need to switch from `peat_protocol::network::IrohTransport` to `peat_mesh::network::IrohTransport`. The methods are 1:1 (same names, same signatures) by design of this ADR — the touch is mechanical but wide.
- **The `IrohMeshTransport::MeshTransport` adapter at `peat-protocol/src/transport/iroh.rs`** wraps the to-be-retargeted `IrohTransport`. Its trait-impl methods need to keep working through the move; if peat-mesh's transport surface is byte-compatible with peat-protocol's, the wrapper's body changes only its `use` lines.
- **Backward-compat re-export.** During the transition (or longer if downstream consumers depend on the path), `peat-protocol/src/network/mod.rs` re-exports `IrohTransport` from peat-mesh under the old path: `pub use peat_mesh::network::IrohTransport;`. Same for `TransportPeerEvent`, `CONNECTION_RECYCLE_INTERVAL_SECS`, etc.

## Migration plan

1. **peat-mesh PR** — add the `peat_mesh::network` module + `IrohTransport` façade. Implement by composing the existing `NetworkedIrohBlobStore` + a small peer-event broadcaster + the recycler. Add the deterministic-seed constructors. Surface tests cover constructor + connect/disconnect happy path + peer-event broadcast.
2. **peat-mesh release rc.18** — version bump + CHANGELOG. The new public surface ships; nothing is deleted yet.
3. **peat consumer PR** — bump workspace `peat-mesh` floor to `>=0.9.0-rc.18`. Add the `[patch.crates-io]` override during development (per peat#864 gotcha) so end-to-end verification can run before the floor bump lands on main. Switch every caller from `peat_protocol::network::IrohTransport` to `peat_mesh::network::IrohTransport` via the re-export shim. Delete `peat-protocol/src/network/iroh_transport.rs`. Delete the recycle task spawn in `automerge_backend.rs`. Drop `iroh`, `iroh-blobs` from `peat-protocol/Cargo.toml`'s direct deps (verify they stay transitive via peat-mesh). Re-run all four QUICKSTART scenarios.
4. **peat release rc.14** — version bump + CHANGELOG citing peat#918.

Each PR carries its own structural-pin tests (mirror the peat#874/#917 pattern for breaker-gate placement). The four-scenario QUICKSTART regression is the load-bearing acceptance gate.

## Alternatives considered

- **Move `peat-protocol::IrohTransport` as-is into peat-mesh.** Rejected: peat-mesh already has `NetworkedIrohBlobStore` with overlapping concerns. Moving the parallel implementation would land peat-mesh with two competing iroh surfaces; the consolidation is the point.
- **Keep both surfaces; document `IrohTransport` as the protocol-layer wrapper.** Rejected: `SKILL.md`'s transport-agnosticism rule is a hard invariant. Leaving the wrapper in place fails the verification gate every release.
- **Phase the move via wrapper-then-delete.** Considered. Risk is that the wrapper phase ships and the delete phase never does, leaving a permanent in-between state. Migration plan above is one-shot once peat-mesh rc.18 publishes, which is cleaner.

## Acceptance (close #918 when)

- [ ] `peat-protocol/src/network/iroh_transport.rs` deleted; the public path re-exports peat-mesh's `IrohTransport` for backward compat (or the path itself is removed if no out-of-repo consumer depends on it — survey TBD per peat#919's approach).
- [ ] `peat-protocol/src/transport/iroh.rs` deleted (`IrohMeshTransport` impl). `peat-protocol/src/transport/mod.rs`'s docstring updated to drop the "Backend-specific implementations (Iroh) remain in this crate" clause that documented the now-closed violation.
- [ ] **Cross-repo consumer survey for `IrohMeshTransport`** — parallel to the `IrohTransport` "survey TBD per peat#919's approach" above. Before deletion: `grep -rln "IrohMeshTransport\|peat_protocol::transport::iroh\|peat_protocol::transport::IrohMeshTransport" --include='*.rs' --include='*.toml'` across `peat-mesh`, `peat-btle`, `peat-atak-plugin`, `peat-lite`, `peat-sim`, plus any other peat-* sibling repo in `/home/kit/Code/DU/`. Plus a `crates.io` reverse-deps check (`curl -s https://crates.io/api/v1/crates/peat-protocol/reverse_dependencies | jq '[.versions[] | select(.crate_size > 0)] | length'`) to verify no published consumer imports `IrohMeshTransport`. If any consumer is found: provide a backward-compat re-export shim at `peat-protocol/src/transport/iroh.rs` instead of deletion, and file a tracking issue for the consumer's migration. Survey result recorded in the implementation PR's description.
- [ ] `peat-protocol/tests/dual_active_simultaneous.rs` either migrated to peat-mesh's test suite (preserving the dual-simultaneous-connect invariant it tests) or retired with an equivalent assertion already present in peat-mesh — decision recorded in the implementation PR.
- [ ] `peat-protocol/Cargo.toml` direct-deps section no longer lists `iroh` or `iroh-blobs`.
- [ ] Connection-recycle task spawn lives in peat-mesh; `PEAT_CONNECTION_RECYCLE_SECS` env var still honored.
- [ ] All four QUICKSTART scenarios pass on rpi-ci + rpi-ci2 + laptop against the released `peat-mesh 0.9.0-rc.18`.
- [ ] `cargo test --workspace --features automerge-backend` clean.
- [ ] `cargo vet` clean (publisher trust from peat#916 covers the new versions).
- [ ] Consumer-name policy gate clean.
- [ ] CHANGELOG entries on both repos cite peat#918 + this ADR.
- [ ] **Verification gate per `SKILL.md`**: `grep -rln "use iroh\|use iroh_blobs\|iroh::Endpoint" peat-protocol/src/` must return zero matches outside the public re-export shim. Same gate for `IrohTransport` and `IrohMeshTransport` identifiers in any peat-protocol non-shim source file. The ADR is closed when peat-protocol's source tree no longer participates in the iroh API surface beyond re-export.
- [ ] **FIPS KDF gate (per `peat/CLAUDE.md` § "Hard rule: FIPS-approved cryptographic primitives only" — KDF: HKDF-SHA-256 / HKDF-SHA-384, SP 800-56C / SP 800-108)**: the `from_seed*` constructors in the new `peat_mesh::network::iroh_transport` surface derive the iroh `SecretKey` via **HKDF-SHA-256**, not the legacy `SHA-256("peat-iroh-key-v1:" || seed)` shape. The plain SHA-256-with-prefix construction in `peat-protocol/src/network/iroh_transport.rs:402-411` produces a 32-byte secret without an extract step, without context binding via a salt, and outside CMVP module coverage for HKDF usage. Moving the pattern as-is into peat-mesh's public surface would record a non-HKDF KDF as the intended design in an ADR; the migration is the right moment to switch.

  **Concrete shape (informational; the implementation PR refines):** `HKDF::<Sha256>::new(Some(b"peat-iroh-v2-salt"), seed.as_bytes()).expand(b"peat-iroh-key-v2:", &mut secret_bytes)` — domain-separated info, fixed crate-wide salt, 32-byte output. The salt + info bump the version tag to `v2` precisely so the NodeId derivation differs from `v1`, surfacing the wire-visible break (see decision point 3 above) at the version-tag level rather than silently.

  **Wire-visible consequence**: every `from_seed*` caller using the same `seed` string post-rc.14 produces a different NodeId than pre-rc.14. The rc.14 release CHANGELOG entry must call this out as a breaking change at the NodeId layer. Operators of pinned-NodeId deployments need to either (a) accept the new NodeIds (re-pin downstream configs) or (b) stay on pre-rc.14 binaries during the migration window. The peat-mesh rc.18 release notes carry the same callout.

- [ ] **FIPS-mode provider gate (per `peat/CLAUDE.md` § "Hard rule: FIPS-approved cryptographic primitives only" — TLS / QUIC: must run under a FIPS-mode provider such as `aws-lc-rs`)** — choose exactly one of the two paths below and check that path's sub-bullet; the other path stays unchecked:

  - [ ] **(a) Close the gap in this PR.** `cargo tree -e features -p iroh` *and* `cargo tree -e features -p iroh-blobs` confirm `aws-lc-rs` is the active TLS backend in the post-migration workspace — no `ring` feature activated on any `rustls`-related dep. Both `peat-mesh/Cargo.toml`'s `rustls = ...` activation and any feature flags peat-protocol's deletion shuffle would otherwise drop are flipped to `aws-lc-rs` as part of the same change.

  - [ ] **(b) Defer the gap; do not close it in this PR.** A dedicated tracking issue is filed and its number is recorded **here in the implementation PR description** (e.g. "peat#NNN — flip rustls activation to aws-lc-rs across peat-mesh + peat-protocol"). The issue body explicitly cites this ADR's "FIPS-mode provider gate" and `peat/CLAUDE.md`'s hard rule. **The tracking issue MUST record a hard closure milestone — a named release ("close by rc.15", "before any FIPS-validated artifact ships", etc.) — written in its body and surfaced in its title. Open-ended issues do not satisfy this path.** `peat/CLAUDE.md` frames the FIPS rule as "non-negotiable" for tactical / DoD procurement; option (b) is a deferral, not a waiver, so it has to be time-bound. The rc.14 release CHANGELOG entry calls out that #918 does not close the FIPS posture gap, points at the follow-up issue, and quotes the closure milestone. `cargo tree` output is recorded in the implementation PR description to pin the post-migration `ring` state for the follow-up to compare against.

  **Background context** (informational; not gating): a survey of the current resolution graph (2026-05-23) shows iroh's tree contains both `aws-lc-rs v1.16.1` *and* `ring v0.17.14`, with `peat-mesh/Cargo.toml:114` activating the `ring` feature on `rustls`. The pre-#918 baseline is therefore *already* on `ring`, not `aws-lc-rs` — the FIPS posture gap is pre-existing rather than introduced by this ADR. Path (a) is preferred (close-the-gap-now); path (b) is acceptable when (a) materially expands the migration's risk surface, but only with the tracking-issue receipt above. Either way the gate produces an auditable checkbox — silent pass is not available.
- [ ] **Cross-compiled FFI binding validation** — both platforms required, no conditional substitutes:

  - [ ] **Android JNI**: the migration PR's CI run includes the existing `Build Android (aarch64-linux-android / armv7-linux-androideabi / x86_64-linux-android)` and `peat-ffi Android Surface Test` jobs (already in `.github/workflows/ci.yml`); they stay green on the migration PR. `ffi/peer.rs` migrates from `peat_protocol::network::IrohTransport` to `peat_mesh::network::IrohTransport` per the migration plan, so these jobs are the structural pin that catches a JNI-symbol-table regression host-side `cargo test --workspace` would miss.

  - [ ] **iOS UniFFI**: the migration PR gates on an iOS UniFFI binding-generation CI job. If the job does **not** exist at PR-open time, **adding it is in scope for the implementation PR** — landing a peat-ffi cross-platform change without iOS validation is what the QA criteria explicitly disallow. The "CHANGELOG note about downstream-consumer responsibility" approach considered in an earlier draft of this ADR was rejected: a CHANGELOG note is not a validation gate; deferring to downstream consumers means an iOS link-time regression surfaces at an external consumer's CI rather than at merge time, after the PR has already shipped. The implementation PR's touch on `ffi/peer.rs` is the correct place to verify the iOS surface; the gate is non-conditional.

## Design review history

This ADR went through seven rounds of QA review on [peat#921](https://github.com/defenseunicorns/peat/pull/921) before reaching the form above. Each round produced a substantive design change rather than a wording tweak; recording the trail here so a future implementer reading the ADR cold doesn't have to reconstruct what was considered-and-rejected vs. agreed-and-codified.

| Round | Commit | Finding class | What changed in the ADR |
|---|---|---|---|
| 1 | `80a35846` (initial draft) | [ARCH] missed `IrohMeshTransport` in scope | `peat-protocol/src/transport/iroh.rs` added to "What's deleted" + acceptance checklist; `transport/mod.rs:5` quoted explicitly as the documented violation this ADR closes |
| 2 | `b08e6a85` | [WARNING] FIPS-mode provider continuity gate missing | `cargo tree -e features` confirm-`aws-lc-rs` gate added; background survey noting the workspace is already on `ring` (`peat-mesh/Cargo.toml:114`) |
| 2 | `b08e6a85` | [WARNING] FFI binding validation gate missing | Android JNI + iOS UniFFI CI gates added to acceptance |
| 3 | `4cb5da61` | [WARNING] FIPS gate internally inconsistent — checkbox conflates close-now and defer paths | Gate split into mutually-exclusive sub-checkboxes (a) close in this PR / (b) defer with auditable receipts |
| 4 | `bd4352e8` | [WARNING] FIPS path (b) lacks a hard deadline | Path (b) now requires a named release-milestone in the tracking issue title + body; rc.14 CHANGELOG quotes the milestone |
| 4 | `bd4352e8` | [WARNING] no cross-repo survey gate for `IrohMeshTransport` consumers | Cross-repo grep + crates.io reverse-deps check added, mirroring the [peat#919](https://github.com/defenseunicorns/peat/issues/919) "survey first, then delete" template |
| 5 | `c521fd7a` | [WARNING] iOS UniFFI gate conditionally gated → no-gate fallback | iOS gate made unconditional; if the CI job doesn't exist at PR-open time, **adding it** is in scope; CHANGELOG-note fallback rejected explicitly |
| 6 | `d1b2a4d7` | [WARNING] FIPS KDF gate missing — `SHA-256("peat-iroh-key-v1:" || seed)` would have moved into peat-mesh as ADR-recorded design | KDF replaced with **HKDF-SHA-256** (`peat-iroh-key-v2:`); wire-visible NodeId-break callout added to Decision §3 + CHANGELOG requirement |
| 7 | `d9db1904` | [WARNING]s PR description stale relative to ADR body (KDF + IrohMeshTransport) | PR description updated to match ADR; comment posted explaining the description-only fix |

The cumulative effect: the original draft would have shipped an implementation PR with no FIPS KDF check, no auditable FIPS provider gate, no cross-repo `IrohMeshTransport` survey, a conditional iOS UniFFI gate that could silently no-op, and missed `IrohMeshTransport` entirely from scope. Each was caught at the design layer rather than after implementation. If the implementation PR runs into a gate that's still unclear, the trail above is the reading order for the original reasoning.
