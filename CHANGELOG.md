# Changelog

All notable changes to the Peat workspace are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This changelog covers the crates published to crates.io from this workspace:

- `peat-protocol` — public facade; depends on `peat-schema` and `peat-mesh`
- `peat-schema` — wire format (Protobuf) definitions

Sub-crates that stay internal (`peat-transport`, `peat-persistence`, `peat-ffi`, `examples/*`) share the workspace version but are not published and are not documented here.

## [Unreleased]

### Removed

- **`peat-discovery` workspace subcrate retired entirely** ([#919](https://github.com/defenseunicorns/peat/issues/919), Phase 3 of the [#898](https://github.com/defenseunicorns/peat/issues/898) mDNS-consolidation epic). After Phase 1's `peat_discovery::mdns` deletion, the remaining `StaticDiscovery` / `HybridDiscovery` / `DiscoveryStrategy` trait had **zero in-repo consumers** (verified across `peat`, `peat-mesh`, `peat-btle`, `peat-atak-plugin`) and the crate was never published to crates.io. The canonical home for discovery strategies is `peat_mesh::discovery::{StaticDiscovery, HybridDiscovery, KubernetesDiscovery, MdnsDiscovery}` per `peat/SKILL.md`'s transport-agnosticism rule. No release impact: the subcrate was internal-only (workspace member, not on crates.io), so this change has no downstream-consumer surface.

### Documentation

- **Subcrate inventory pruned** across `README.md`, `CONTRIBUTING.md`, `DEVELOPMENT.md`, `DEPENDENCY-LICENSES.md`, `SKILL.md`, `docs/RELEASING.md`, `docs/ARCHITECTURE.md`, `docs/guides/developer/DEVELOPER_GUIDE.md`, and `peat/src/lib.rs`. Historical `[0.9.0-rc.*]` CHANGELOG entries that name `peat-discovery` are left intact — those record the state at release time and shouldn't be retroactively edited.

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
