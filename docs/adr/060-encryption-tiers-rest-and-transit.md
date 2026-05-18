# ADR-060: Encryption Tiers — At-Rest and In-Transit Across the Peat Stack

**Status**: Proposed
**Date**: 2026-05-18
**Authors**: Kit Plummer
**Related**: ADR-006 (Security Architecture), ADR-016 (TTL and Data Lifecycle), ADR-034 (Deletion and Tombstones), ADR-042 (UDP Bypass), ADR-044 (E2E Encryption and Key Management), ADR-049 (peat-mesh Extraction)
**Triggered by**: peat-node #55 (filtered observe), peat-mesh #124 (Cipher trait), cross-peer regression discovered while implementing the above

---

## Context

The peat ecosystem currently has multiple places where encryption applies, and the relationships between them have grown without a single coherent design. This ADR exists to surface and reconcile those decisions before the next round of refactors (peat-node #55, peat-mesh #124) lands.

### What triggered this ADR

While implementing peat-node #55 (filtered `Subscribe` via `DocumentStore::observe`), it became clear that the existing peat-node storage shape (`{"value": "ENC:v1:<base64>"}` envelope around the entire JSON payload) makes structural filtering impossible — every document looks like a single-field opaque blob to the trait layer.

The fix proposed in peat-mesh #124 — "move encryption to the substrate, store structured Automerge docs" — was implemented and broke `cross_peer_encryption_test`. The test asserted that a peer in the formation but without the at-rest encryption key sees an opaque `ENC:v1:` envelope after sync. That property emerged as a side effect of the old design (envelope encryption at the application layer travels through Automerge sync verbatim) and is a *real* security guard, not test bookkeeping. The substrate-encryption design loses it: sync now exchanges plaintext structural patches, so any formation member sees plaintext after sync regardless of payload-key possession.

The user response: *"We can not regress on the security requirements... go back to the top-level architecture. What is the right answer for encryption at-rest and in-transit — and is this different for different phases?"*

That is this ADR.

### Phases of the stack that touch encryption

| Phase | What flows | Where it lives |
|---|---|---|
| Discovery | Peer endpoint IDs, addresses, formation IDs | mDNS, k8s headless services, static config |
| Connection setup | TLS handshake + FormationKey challenge/response | Iroh QUIC + `peat-mesh/src/security/formation_key.rs` |
| Document sync | Automerge CRDT patches | `peat-mesh/src/storage/mesh_sync_transport.rs` + `automerge_sync.rs` |
| Document at-rest | Automerge byte-blobs in redb | `peat-mesh/src/storage/automerge_store.rs` |
| Attachment transfer (wire) | Iroh blob bytes | `peat-mesh/src/storage/iroh_blob_store.rs` |
| Attachment at-rest | Blob bytes in Iroh blob store | Same |
| Distribution metadata | `file_distributions` Automerge docs | `peat-protocol/src/storage/file_distribution.rs` (rides on doc sync) |
| Bypass channel | UDP framing (ADR-042) | `peat-mesh/src/transport/bypass.rs` |

Each phase has independent decisions about what's protected, who can read it, and which keys are involved. Today those decisions are inconsistent.

---

## Threat model

Adversaries this ADR addresses, with explicit naming:

| # | Adversary | Capability |
|---|---|---|
| **T1** | External network observer (passive) | Sees all bytes on the wire; no keys; cannot inject. |
| **T2** | External attacker trying to join the mesh | Active network access; no FormationKey; cannot authenticate as a peer. |
| **T3** | Compromised host with offline disk access | Reads redb files, Iroh blob store, config files. No active process. (Post-mortem disk seizure; recovered hardware; compromised storage backups.) |
| **T4** | **Formation member without payload key** | Successfully joined the formation (has FormationKey); can sync; **does not have the at-rest payload key**. Legitimate role for read-only auditors, observers, archival nodes, third-party integrators with metadata-only access. |
| **T5** | Compromised live host with process memory access | Active RAM dump, debugger attach, kernel-level malware. Plaintext is in memory transiently regardless of design. |
| **T6** | Malicious formation member with payload key (insider) | Fully authenticated, fully privileged. ADR-044's MLS / membership-cert work addresses this; out of scope here. |

This ADR's primary contribution is making T3 and **T4** first-class. T4 in particular has been provided accidentally by the old peat-node envelope and would have been silently removed by the proposed peat-mesh #124 refactor. T4 is a real and load-bearing role for tactical deployments — observers, joint-operations partners, archival nodes — and the design must preserve it deliberately rather than by side effect.

---

## Decision drivers

Hard requirements (from user, ADRs 006/044, and the existing test corpus):

1. **No regression on T3** — at-rest protection must be at least as strong as today (preferably stronger; the old design only encrypted the `value` field, leaving Automerge metadata plaintext on disk).
2. **No regression on T4** — a formation member without the payload key must not be able to read payload contents.
3. **Per-field CRDT merges must work** — concurrent writes from different keys must merge per-field, not collide at the whole-doc level. (The old design defeated this; one of the reasons it needs replacement.)
4. **F2 filtering (peat-node #55) must work** — server-side `Subscribe` predicates must filter documents by field name and value, with structured visibility into the doc.
5. **Wire confidentiality from T1** — already provided by Iroh QUIC TLS; status quo.
6. **FIPS-approved cryptographic primitives only.** Every algorithm used in the peat ecosystem must appear on the FIPS 140-3 approved list. AEAD: AES-GCM (not ChaCha20-Poly1305). Signatures: Ed25519 (per FIPS 186-5) or ECDSA-P256/P384. Key agreement: ECDH on P-256/P-384 (X25519 only with explicit caveat — see §5). KDF: HKDF-SHA-2. MAC: HMAC-SHA-2. TLS/QUIC must run under a FIPS-mode crypto provider (e.g. `aws-lc-rs` backend for rustls). Procurement-driven (tactical/DoD customers); non-negotiable. Detailed in §5 below. This requirement supersedes any prior ecosystem reference to ChaCha20-Poly1305 (ADR-006, ADR-044, ADR-048, ADR-049, README, spec docs) — those records need amendment; tracked in §References → Follow-up amendments.

Soft requirements:

7. Implementation complexity should be commensurate with the protection delivered. Searchable encryption (homomorphic, order-preserving) is out of scope.
8. The encryption boundary should be configurable per-collection where a single posture doesn't fit all data.

---

## Decision

### 1. Per-field encryption at the application boundary; substrate at-rest is custody-dependent obfuscation/protection

For collections where T4 applies (the default for sensitive collections):

- **Field names** stored plaintext inside Automerge docs. Allows CRDT per-field merges, structural sync, and (for the local sidecar with the key) field-name-based query routing.
- **Field values** encrypted at the application boundary with the **payload key** (AES-256-GCM, per-value nonce — see §5 for FIPS rationale; this is a deliberate departure from ChaCha20-Poly1305 references in ADR-006/044/048/049). Each value is an independent ciphertext blob; concurrent updates to different fields merge naturally via Automerge.
- **Substrate `Cipher` trait** (peat-mesh #124) wraps the entire redb byte-blob at-rest. **Protection value depends on FormationKey custody** (see also §Decision #4 and §Risks). In the default deployment where the FormationKey lives in a config file on the same host as the data, a T3 attacker with offline disk access also obtains the FormationKey from those config files and can derive the at-rest key locally — the cipher functions as **on-disk obfuscation against T3**, not protection. In hardened deployments where the FormationKey is held outside the data image (HSM, TPM-sealed, operator-provided key escrow, or a separate filesystem the T3 attacker did not seize), the same T3 attacker recovers only ciphertext and the cipher delivers genuine field-name and field-value protection at rest. The cipher provides **no protection against T4** under any deployment: T4 by definition holds the FormationKey and can derive the at-rest key the same way the local sidecar does. T4 protection comes from per-field payload-key encryption, not from the substrate cipher.
- **Sync** exchanges structured Automerge patches as today. Field names ride plaintext; field values ride as opaque ciphertext blobs. A T4 peer (in the formation, no payload key) receives intelligible structure but unreadable values.
- **Query evaluation (F2 filter)** happens at the sidecar after field-level decryption. The substrate's `DocumentStore::observe(c, &query)` is called with `Query::All` (no filtering at the substrate level); the sidecar receives every change event for the collection, decrypts the per-field values, and applies the predicate post-decryption before emitting to the gRPC stream.

### 2. Per-collection encryption policy

Encryption posture is configured per collection — a new axis on the collection config introduced in #55 (alongside the lifecycle / deletion-policy axis from ADR-016 / ADR-034). The supported postures:

| Posture | Field names | Field values | Substrate at-rest | T4 protection | F2 filtering |
|---|---|---|---|---|---|
| **`Plaintext`** | Plaintext | Plaintext | Optional | None | Works at substrate (no decrypt) |
| **`FieldValues`** (default for new collections in formations with a payload key) | Plaintext | Encrypted | Yes | ✓ (values opaque to T4) | Works at sidecar (decrypt-then-filter) |
| **`FullOpacity`** | Encrypted | Encrypted | Yes | ✓ (full opacity to T4) | Not supported — collection is opaque |

Default selection on collection creation **(applies only to `format_version=v2_posture` collections — see §Decision §7)**:
- If the sidecar is configured with a payload key, new collections default to `FieldValues`.
- Otherwise, `Plaintext`.
- Operators can override per collection via the `SetCollectionConfig` RPC (#55).

Collections stamped `format_version=v1_envelope` (the legacy shape that pre-dates this ADR) do not have a posture field at all — they are read/written via peat-node's existing `StoreCipher` envelope path until explicitly migrated per §Decision §7.

Existing collection configs (ADR-034 deletion policies) and this new posture share the same per-collection registry — they are independent axes of the same config struct.

#### Posture conflict resolution (decided in this ADR, not deferred)

Posture is a **security-load-bearing** field; CRDT last-write-wins is unsafe (a concurrent edit by a peer with a later clock can downgrade the posture, after which subsequent app writes to the same collection are emitted under the weaker posture — a confidentiality bug, not an ergonomic one). The QA review of the initial draft escalated this; the semantics are decided here rather than left to Phase D implementers.

**Rule 1 — Strict monotonic strengthen-only LUB merge for concurrent edits.**

Postures are ordered: `Plaintext` (weakest, strength=0) < `FieldValues` (strength=1) < `FullOpacity` (strongest, strength=2). When the sidecar reads `_collection_configs` to determine the current posture for a collection, it walks the Automerge change history for the posture field and reduces it with a **least-upper-bound (LUB / max-strength)** operator over concurrent branches, not the Automerge-native LWW. This means:

- Concurrent `Plaintext` + `FullOpacity` writes → `FullOpacity` wins. Always.
- Concurrent strengthen + strengthen (e.g. `FieldValues` + `FullOpacity`) → strongest wins.
- A single non-concurrent strengthen → applies immediately on receipt.
- Any **weakening** edit submitted via the normal `SetCollectionConfig` path that produces a concurrent branch with a stronger value is **dropped by the LUB merge** and logged as a rejected-downgrade audit event on every peer that observes the conflict.

This is a sidecar-layer rule applied over Automerge's stored history; the substrate still stores all change events losslessly (so the audit log remains reconstructible).

**Rule 2 — Downgrades are an explicit, audited, CAS-guarded operation.**

Because Rule 1 makes downgrade un-expressible as a concurrent edit, peat-node MUST expose downgrade as its own RPC distinct from `SetCollectionConfig`:

```
DowngradeCollectionPosture(
    collection,
    from: Posture,    // CAS precondition — peer's current posture must equal this
    to: Posture,      // must be strictly weaker than `from`
    operator_id,      // who is doing this
    justification,    // free-text, recorded in audit log
)
```

Receiving peer behavior:
1. Look up the current posture via the Rule-1 LUB. If it ≠ `from`, **reject** with `PreconditionFailed` and log `downgrade.rejected.cas_mismatch`. The operator must re-read current posture and reissue.
2. If `to` is not strictly weaker than `from`, reject (`InvalidArgument`) — downgrade RPC is for downgrade only; use `SetCollectionConfig` for the strengthen path.
3. Wait one full sync round-trip with the formation (configurable timeout) so any in-flight concurrent strengthen edits are observed before the downgrade takes effect. If a strengthen arrives during the wait, abort the downgrade with `Aborted` and log `downgrade.aborted.concurrent_strengthen`.
4. Write a dedicated `_collection_configs` record of shape `{kind: "downgrade", collection, from, to, operator_id, justification, lamport_clock, timestamp}`. This is a *separate Automerge document/field* from the LUB-merged posture field; it is the audit trail, replicated across the formation.
5. Apply the downgrade locally; the new posture is the value carried by the downgrade record, not the LUB of the posture field. Other peers do the same on receipt of the downgrade record.

This makes posture changes a two-mode operation: strengthens are eventual (CRDT-merged), downgrades are explicit (RPC-driven, CAS-checked, audited).

**Worked example — the interleaving the QA review asked for:**

```
T=0    Formation F has two operators A and B. Collection `commands` has posture
       FullOpacity. Doc d1 contains field `cmd` encrypted under payload key.

T=1    Operator A writes d1.cmd = "fire" via PutDocument.
       Sidecar A reads posture → FullOpacity → encrypts → stores.
       d1 now: { cmd: <ciphertext_v2> }

T=2    Operator B, racing, issues SetCollectionConfig(commands, Plaintext).
       Under naive LWW this would write Plaintext to the posture field with
       B's Lamport clock > A's, and propagate. Under Rule 1, B's edit is a
       concurrent weakening branch in `_collection_configs`.

T=3    Sync round completes. Both peers' `_collection_configs` history now
       contains: {posture=FullOpacity from initial config} || {posture=Plaintext from B@T=2}.
       Rule-1 LUB walks the history → max(FullOpacity, Plaintext) = FullOpacity.
       Sidecar A and B both resolve posture = FullOpacity.
       Both peers log `downgrade.rejected.lub` for B's @T=2 attempt.
       Operator B's RPC returned success at T=2 (the write to _collection_configs
       did land); but the sidecar at read-time computes the merged posture, and
       the rejected-attempt audit entry surfaces in B's operator console.

T=4    Operator A writes d1.target = "x".
       Sidecar A reads posture (via LUB) → FullOpacity → encrypts → stores.
       d1 now: { cmd: <ct_v2>, target: <ct_v3> }   — both encrypted.
       NO confidentiality regression. The downgrade race is structurally
       impossible because Rule 1 made the weakening edit a no-op at read time.

T=5    Operator B, having seen the audit entry, decides the downgrade is
       intentional. Issues DowngradeCollectionPosture(commands, from=FullOpacity,
       to=Plaintext, operator_id=B, justification="moving commands to
       a public-readable channel per ops change request #N").
       Sidecar B: CAS check passes (current=FullOpacity), waits one sync RTT,
       no concurrent strengthen, writes downgrade record. Posture becomes
       Plaintext on all peers as the record propagates.

T=6    Operator A writes d1.next_target = "y" after the downgrade.
       Sidecar A reads posture → Plaintext → writes plaintext.
       d1 now: { cmd: <ct_v2>, target: <ct_v3>, next_target: "y" }.
       Old encrypted fields stay encrypted (Automerge doesn't rewrite them);
       new fields are plaintext per the new posture. App layer is expected
       to handle mixed-shape docs across a posture transition — see §Risks.
```

**Implications for the posture table:** the "default posture for new collections" rule (above) is unchanged. A new collection's first posture write is not a concurrent edit; Rule 1 kicks in only when concurrent branches exist.

### 3. Per-phase decisions

| Phase | At-rest | In-transit | T4 protection | Notes |
|---|---|---|---|---|
| **Discovery** (mDNS / k8s DNS) | Plaintext (config files) | Plaintext (multicast / DNS) | Metadata visible | Status quo. FormationKey is the gate — knowing a peer exists is not protected. Hardening discovery against T1 (e.g., signed mDNS records) is a separate ADR if needed. |
| **Connection setup** | n/a | TLS 1.3 + FormationKey challenge | Formation-gated | Status quo. Already correct. |
| **Document sync** | n/a | TLS-wrapped Automerge patches; field names plaintext, field values per-collection-posture | ✓ via per-collection posture (`FieldValues` or `FullOpacity`) | Per-field encryption inside the patches. The substrate doesn't change sync semantics; it just stores whatever it receives. |
| **Document at-rest** | Substrate `Cipher` (per peat-mesh #124, with at-rest key derived from FormationKey via HKDF) | n/a | **None at this layer** (T4 holds FormationKey, derives at-rest key the same way the sidecar does) | T3 protection is **conditional on FormationKey custody** — see §Decision #1 and §Risks. Default deployment (FormationKey in config file): substrate cipher is on-disk obfuscation against T3, not protection. Hardened deployment (FormationKey in HSM / TPM-sealed / external escrow): substrate cipher recovers real T3 protection. Field-value plaintext recovery still requires the payload key in all deployments — that is where T4 protection actually comes from. |
| **Attachment transfer** | n/a | Chunked AES-256-GCM (sidecar layer, payload key) over TLS-wrapped Iroh QUIC | ✓ via sidecar `AttachmentCipher` envelope — see §Decision §6 | Per-chunk 64 KiB AES-256-GCM. Deterministic vs Randomized nonce mode is sender-selected (default Deterministic for idempotent senders — peat-node `SendAttachments`, peat-registry OCI sync, peat-sim scenarios). Sidecar wraps before push to peat-mesh's Iroh; sidecar unwraps after pull. T4 sees ciphertext on the wire. |
| **Attachment at-rest** | Chunked AES-256-GCM ciphertext stored in Iroh blob store; substrate Iroh-blob cipher layered underneath (FormationKey-derived, same custody story as redb cipher) | n/a | ✓ at the sidecar layer (payload key); substrate layer adds T3-conditional protection | Iroh content-addresses the **ciphertext** — `blob_token` is BLAKE3 of ciphertext. The application's plaintext SHA-256 (e.g., peat-node `FileSpec.sha256`, OCI digest, peat-sim chip hash) stays the identity the app reasons about; the sidecar maintains the plaintext_sha256 → ciphertext_blob_token mapping (reproducible from inputs in Deterministic mode, requires explicit lookup in Randomized mode). |
| **`file_distributions` Automerge docs** | Rides on doc sync | Rides on doc sync | Inherits the collection posture configured for `file_distributions` | Distribution metadata (blob hashes, target node lists, scopes, filenames, MIME) may be sensitive in tactical deployments. Defaults to `FieldValues` posture. Blob payload protection (T4 visibility of the bytes themselves) lives at the sidecar `AttachmentCipher` layer described in the row above and §Decision §6; this row only covers the metadata document. |
| **Bypass channel** (ADR-042) | n/a | Plaintext UDP framing today | None | ADR-042 explicitly flagged this gap; ADR-044 proposed MLS-based addressing. Out of scope here. |

### 4. Key hierarchy

Two keys per formation, plus existing per-peer Iroh TLS:

1. **FormationKey** — pre-shared formation secret. Already exists. Gates membership; used for connection authentication.
2. **At-rest key** — derived from FormationKey via HKDF with a fixed context string (`"peat-mesh/at-rest/v1"`). Used by peat-mesh's `Cipher` trait inside `AutomergeStore` to encrypt redb bytes. Derived not stored. **Custody implication:** because this key is a deterministic HKDF of the FormationKey, anyone who reads the FormationKey can derive the at-rest key. T4 has the FormationKey by definition (no at-rest protection from this layer for T4). T3's ability to recover the FormationKey depends on where the FormationKey lives — see §Decision #1 and the FormationKey-custody Risks row.
3. **Payload key** — separate from FormationKey. Configured explicitly per sidecar (`--encryption-key` flag today). When present, the sidecar applies field-value encryption to collections with `FieldValues` or `FullOpacity` posture. When absent, the sidecar can sync structural patches (and store them) but cannot decode encrypted field values — the T4 role.
4. **Iroh QUIC TLS** — per-connection ephemeral keys generated by Iroh. Unchanged.

The FormationKey gates membership and derives the at-rest key. The payload key is a separate trust dimension explicitly chosen at deployment time. A peer with the FormationKey but not the payload key occupies the T4 role.

### 5. Cryptographic primitives (FIPS posture)

Per driver #6, every primitive used by this ADR's encryption tiers must be FIPS 140-3 approved. The table below documents the choice for each role and the FIPS rationale.

| Role | Primitive | FIPS basis | Notes |
|---|---|---|---|
| AEAD (field-value cipher, substrate at-rest cipher) | **AES-256-GCM** | NIST SP 800-38D | Fresh 96-bit nonce per encryption; never derive deterministically. |
| KDF (FormationKey → at-rest key) | **HKDF-SHA-256** | NIST SP 800-56C / SP 800-108 | Context string `"peat-mesh/at-rest/v1"`; versioned (§Open Questions #2). |
| Signatures (existing — peer identity, ADR-006) | **Ed25519** | FIPS 186-5 (Feb 2023) | Approved as of FIPS 186-5; current ecosystem usage is compatible. |
| Key agreement (peer identity, where used) | **ECDH-P256** preferred; X25519 only with explicit caveat | NIST SP 800-56A (ECDH); SP 800-186 lists Curve25519 but CMVP coverage of ECDH-with-X25519 is uneven | Flag X25519 references for explicit review during the ADR-006/044 amendments. |
| MAC (where used) | **HMAC-SHA-256** | FIPS 198-1 | — |
| Hash | **SHA-256 / SHA-384** | FIPS 180-4 | — |
| TLS / QUIC (Iroh) | **rustls under FIPS-mode provider** (e.g. `aws-lc-rs`) | rustls FIPS profile; aws-lc-rs is CMVP-validated | Iroh's quinn/rustls stack must be configured with a FIPS provider in deployments claiming FIPS posture. Default `ring` backend is **not** FIPS-validated. |

**Explicit non-choices:**

- **ChaCha20-Poly1305** — not FIPS-approved (IETF RFC 8439; never blessed by NIST). Any reference in ADR-006 / ADR-044 / ADR-048 / ADR-049 / README / spec docs is a constraint violation and is tracked for amendment in §References → Follow-up amendments.
- **X25519 as a default** — Curve25519 is approved (SP 800-186) but ECDH-with-X25519 has uneven CMVP module support; prefer ECDH-P256 where a choice exists. If an existing protocol pins X25519 (e.g. MLS suite selection in ADR-044), choose a FIPS-aligned MLS suite instead (e.g. `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`).
- **Deterministic / order-preserving / homomorphic schemes** — out of scope for this ADR (see Alternative §4); independently most fall outside FIPS approval.

**Why this matters here:** the QA review of the initial draft flagged AES-256-GCM as a deviation from a ChaCha20-Poly1305 ecosystem standard documented in ADR-006/044. Under driver #6 the deviation is the *correct* direction; the ecosystem references are what's out of line. Recording the FIPS posture here, with the amendment list in §References, prevents the same ambiguity from recurring as the substrate refactors land.

### 6. Attachment encryption (decided in this ADR, not deferred)

T4 is named load-bearing for tactical deployments throughout this ADR. The initial draft deferred attachment encryption to a follow-up ADR, which the QA review correctly flagged as inconsistent: a T4 peer with no payload key could read every Iroh blob byte verbatim while the rest of the design claimed T4 protection. Validated against actual consumer requirements in peat-node, peat-registry, and peat-sim (and the WIP `feat/7n-dual-c2-concurrent-telemetry` in peat-sim that exercises concurrent telemetry + attachment convergence), this section commits the design inline.

#### 6.1 Two-layer cipher, same shape as docs

Attachments get the same two-layer treatment as documents:

- **Sidecar `AttachmentCipher` (payload key)** — load-bearing for T4. Chunked AES-256-GCM at the sidecar boundary. Encrypts blob bytes before they enter Iroh; decrypts blob bytes after they leave Iroh. This is the tier that delivers T4 protection.
- **Substrate Iroh-blob cipher (FormationKey-derived)** — same custody-dependent T3 obfuscation/protection as the redb substrate cipher (§Decision §1). Default deployment (FormationKey in config alongside data) = on-disk obfuscation only; hardened deployment (FormationKey held outside the data image) = real T3 protection. **No T4 protection** at this layer under any deployment.

Posture inheritance: attachments inherit the posture of their referencing collection (the collection that owns the `file_distributions` doc that points at the blob). `Plaintext` posture = no sidecar cipher applied (substrate cipher still runs per its own rules). `FieldValues` and `FullOpacity` posture = sidecar cipher applied. This matches §Decision §3 row 7 (the file_distributions row).

#### 6.2 Chunked construction

Blob bytes are split into **64 KiB chunks**. Each chunk is sealed independently with AES-256-GCM:

```
chunk_ciphertext_i  = AES-256-GCM-Seal(
    key   = payload_key,
    nonce = nonce_i  (see §6.3),
    aad   = chunk_index_i || plaintext_sha256 || formation_id,
    plaintext = chunk_plaintext_i
)

blob_ciphertext = header || chunk_ciphertext_0 || chunk_ciphertext_1 || ...
```

Header (fixed-size, plaintext at the substrate layer because the substrate Iroh-blob cipher wraps the whole thing anyway):

```
struct AttachmentHeader {
    magic:             [u8; 4]    // "PAT1"
    version:           u8         // 1
    mode:              u8         // 0 = Deterministic, 1 = Randomized
    chunk_size_log2:   u8         // 16 → 64 KiB
    payload_key_id:    [u8; 16]   // identifies which payload-key generation; enables future rotation
    plaintext_sha256:  [u8; 32]   // sha-256 of the plaintext blob (the identity the app reasons about)
    plaintext_length:  u64        // bytes
    formation_id:      [u8; 16]   // FormationKey-fingerprint
    nonce_salt:        [u8; 16]   // Deterministic: zero; Randomized: 128-bit random
}
```

`chunk_size_log2 = 16` (64 KiB) is the only value v1 supports. peat-node's 256 MiB per-file cap → max 4096 chunks per blob, well below any AES-GCM nonce-collision concern.

**Receiver length-check requirement (header integrity).** The chunk-level AAD covers `chunk_index_i || plaintext_sha256 || formation_id`, which authenticates the chunk's position in the stream and the plaintext_sha256 binding — but it does **not** cover the header's `plaintext_length` field. A T1/T4 adversary that flips `plaintext_length` in the header and truncates trailing chunks at the substrate layer cannot be distinguished from a legitimately short blob by per-chunk GCM authentication alone (each surviving chunk still authenticates).

Therefore: **the receiver MUST compare the post-decode plaintext byte count against the header's `plaintext_length` field and reject any mismatch with an integrity error.** This is a normative requirement on every implementation of the receive path, not just a property the test suite checks. The substrate layer (Iroh-blob cipher) provides a second-layer integrity check under hardened FormationKey-custody deployments, but the sidecar-layer length check is the load-bearing one for T1/T4 truncation defense and must be present even when the substrate cipher is disabled.

Implementations may alternatively bind the canonically-encoded header bytes into every chunk's AAD (which automatically catches any header flip including `plaintext_length`); both approaches satisfy the requirement.

#### 6.3 Nonce derivation — Deterministic vs Randomized

Both modes use **standard FIPS-approved AES-256-GCM** with a 96-bit nonce. The mode determines how the nonce is derived:

**Deterministic mode** (default for idempotent senders — peat-node `SendAttachments`, peat-registry OCI sync, peat-sim scenarios):

```
nonce_i = HKDF-Expand(
    prk  = HKDF-Extract(salt = formation_id, ikm = payload_key),
    info = "peat-attachment-nonce/v1" || plaintext_sha256 || chunk_index_i,
    L    = 96 bits
)
```

Same plaintext + same formation + same payload key → identical nonces across chunks → identical ciphertext → identical Iroh `blob_token`. Idempotency preserved. Cross-formation: different `formation_id` → different nonces → different ciphertext → no cross-formation dedup (security boundary preserved).

This is a **synthetic-IV construction over AES-256-GCM** — security argument follows AES-GCM-SIV (RFC 8452) reasoning while keeping the primitive on the FIPS-approved list. The relevant safety property: AES-GCM nonce reuse is catastrophic only when the same `(key, nonce)` pair encrypts *different* plaintexts. Here, same nonce ⇒ same plaintext_sha256 + same chunk_index ⇒ same plaintext, so the ciphertext is bit-identical and no information is leaked beyond "this exact plaintext appears". Within a formation, birthday bound on 96-bit nonces across distinct plaintexts is 2^48; peat formations will not see 2^48 distinct attachments in any realistic deployment.

**Randomized mode** (opt-in for sensitive content where confirmation-of-equality is unacceptable):

```
nonce_salt = 128 bits from OS RNG (stored in header)
nonce_i    = HKDF-Expand(
    prk  = payload_key,
    info = "peat-attachment-nonce/v1" || nonce_salt || chunk_index_i,
    L    = 96 bits
)
```

Different `nonce_salt` per submission → different ciphertext → different `blob_token`. No idempotency. Confirmation-of-equality leak eliminated. Used when the sender explicitly opts in.

#### 6.4 Sender mode selection

Mode is a per-request field on the attachment-ingest path, not a global config:

- peat-node `SendAttachments` proto gains an `encryption_mode` enum on `FileSpec` (default `DETERMINISTIC`, alternative `RANDOMIZED`). The existing `bundle_id` idempotency contract (`proto/sidecar.proto:572-577`) holds only for `DETERMINISTIC`; `RANDOMIZED` resubmissions produce distinct `blob_token`s by design.
- peat-registry: all OCI layer sync uses `DETERMINISTIC` (OCI layers are public-by-design within their formation; confirmation-of-equality is a non-issue because the OCI digest already reveals layer identity).
- peat-sim: defaults to `DETERMINISTIC` for scenario replay reproducibility; test scenarios can opt into `RANDOMIZED` to verify that path.

#### 6.5 peat-registry boundary

peat-registry runs an OCI HTTP server toward registry clients and a peat-mesh node toward other registries. The encryption boundary is **internal to peat-registry**:

```
OCI client ──(plaintext OCI HTTP)──> peat-registry  ──(encrypted via §6.2)──> peat-mesh ──> remote peat-registry ──(plaintext OCI HTTP)──> OCI client
                                          │                                                          │
                                       encrypt                                                   decrypt
                                       on push                                                   on pull
```

OCI digest immutability is preserved because OCI clients only see plaintext. The peat-mesh side carries ciphertext; T4 peers in the formation between the two registries see only ciphertext layer bytes. peat-registry's `RegistryClient` trait (`src/transfer/engine.rs:31-32`) gains an encrypt-on-push / decrypt-on-pull adapter rather than changing the OCI semantics.

Resumable checkpoints (`src/transfer/checkpoint.rs`): the `partial_blob` checkpoint tracks the **plaintext** offset (what the OCI client thinks it received). The encrypt/decrypt adapter aligns chunk boundaries to plaintext offsets so a resume at plaintext_offset = N starts encryption from chunk floor(N / 64KiB). The checkpoint format does not need to know encryption is happening.

#### 6.6 peat-sim and the WIP concurrent-telemetry scenario

peat-sim's existing chip-injection path (`NetworkedIrohBlobStore::create_blob_from_bytes`) gains the same `encryption_mode` selector. The default (`DETERMINISTIC`) preserves the existing scenario-replay reproducibility property — same chip plaintext + same formation_id → same `image_chip_hash` across runs.

The WIP `feat/7n-dual-c2-concurrent-telemetry` branch does not exercise the Deterministic-mode dedup property (telemetry is on a separate `PutDocument` path; attachment tests assert filesystem round-trip identity, not `blob_token` equality). Phase E mandates an explicit test for the dedup property so the security/efficiency claim is verified before merge.

#### 6.7 What this design does and does not protect

| Adversary | Protection from §6 | Where it comes from |
|---|---|---|
| **T1** (passive network observer) | ✓ Full | TLS-wrapped Iroh QUIC + chunked AES-256-GCM ciphertext on the wire |
| **T2** (external attacker trying to join) | ✓ Full | FormationKey gate; never gets payload key |
| **T3** (compromised host with offline disk) | Custody-dependent. Default deployment = obfuscation only (T3 reads FormationKey from config, derives substrate at-rest key, recovers the chunked-AES-GCM ciphertext; without the payload key, cannot decrypt the chunks). Hardened deployment with FormationKey external = full T3 protection. | Substrate Iroh-blob cipher + sidecar AttachmentCipher; protection composition depends on FormationKey custody |
| **T4** (formation member without payload key) | ✓ Full for blob payload bytes | Sidecar AttachmentCipher (chunked AES-256-GCM, payload key required to decrypt) |
| **T5** (live host RAM access) | Out of scope (always-on protection against this requires memory-protected key handles — separate ADR) | n/a |
| **T6** (malicious insider with payload key) | Out of scope (ADR-044 MLS / membership-cert work) | n/a |

What §6 does **not** protect:
- **Blob size.** T1/T4 see the ciphertext length, which is plaintext_length + per-chunk GCM tag overhead. Size-equivalence is not concealed; padding is out of scope for v1.
- **Distribution metadata** beyond what `file_distributions` posture covers. Filenames/MIME/sizes ride in the distribution doc and follow §Decision §3 row 7 (collection-posture-inherited).
- **Confirmation-of-equality across submissions** in Deterministic mode (a T4 observer sees that two distinct `bundle_id`s reference the same `blob_token` ⇒ knows they carry the same plaintext). Acknowledged tradeoff for idempotency; Randomized mode eliminates this leak.

### 7. Migration from legacy envelope (v1 → v2 transition)

peat-node before this ADR encrypts entire JSON documents into a single Automerge field named `value` carrying a `ENC:v1:`-prefixed base64 ciphertext. §Decision §1 replaces that with per-field encryption (`FieldValues` posture) where the Automerge doc holds structured fields with encrypted values. The two shapes are incompatible at the CRDT level: an old peer reading a new-shape doc sees plaintext field names it cannot map back to a single decrypted JSON; a new peer reading an old-shape doc sees a single `value` field with `ENC:v1:` prefix and no posture metadata. Mixed-version formations during a rolling upgrade are the failure mode the QA review (WARNING #5) flagged. This section commits the migration story rather than relying on operator vigilance.

#### 7.1 Per-collection `format_version` axis

A new field is added to the per-collection config: `format_version` ∈ {`v1_envelope`, `v2_posture`}.

- `v1_envelope` — the legacy shape. peat-node's existing `StoreCipher` envelope is the read/write path. The posture axis from §Decision §2 does not apply.
- `v2_posture` — the new shape. The posture axis (`Plaintext` / `FieldValues` / `FullOpacity`) applies; peat-node's `FieldCipher` (Phase B) is the read/write path.

`format_version` is **monotonic v1 → v2 only**, governed by the same strengthen-only LUB merge semantics as posture (§Decision §2 Rule 1). A v2 → v1 downgrade is not expressible as a CRDT edit. Per-collection.

Collections that pre-date this ADR are stamped `v1_envelope` at upgrade time (the sidecar detects the absence of a `format_version` field in `_collection_configs` and writes `v1_envelope` as a one-shot backfill — purely additive, no doc shape changes). New collections default per §7.2.

#### 7.2 Sidecar opt-in for v2 default

A new sidecar flag `--collection-format-default` ∈ {`v1_envelope` (default), `v2_posture`} controls the `format_version` chosen when this sidecar creates a new collection without an explicit `SetCollectionConfig` first.

- **Default = `v1_envelope`.** Existing operators who upgrade peat-node without setting this flag observe no behavioral change. Existing collections keep working via the legacy envelope. New collections also use the legacy envelope. The new posture machinery is dormant until opted into.
- **`v2_posture`.** New collections default to `v2_posture`, and the posture defaults from §Decision §2 apply.

This is the QA reviewer's option (b): explicit opt-in rather than silent default change. Operators can also set `format_version` explicitly per-collection via `SetCollectionConfig` regardless of the default flag.

#### 7.3 Per-collection migration tool

For collections stamped `v1_envelope` to move to `v2_posture`, an operator runs:

```
peat-node migrate-collection \
    --collection commands \
    --to-posture FullOpacity \
    --confirm
```

Migration steps:

1. **Formation-wide version-check gate.** The migration RPC queries the formation for peer sidecar versions (a `ListPeerVersions` health-check surface that ships with Phase F). If any peer is on a sidecar version older than the v2-supporting threshold, the migration is **rejected** with `FailedPrecondition` and a list of peers to upgrade. This is the QA reviewer's option (c) made into a hard precondition.
2. **Acquire collection migration lock.** Write a `migration_in_progress` marker to `_collection_configs` for the target collection, carrying `{operator_id, lamport_clock, started_at}`. Other operators issuing concurrent migrations on the same collection see the marker and abort with `Aborted`.
3. **Walk every doc in the collection.** For each doc: (a) read via legacy envelope path, (b) re-write under the target `v2_posture` shape per the chosen posture, (c) wait for one sync round-trip before moving on (prevents overwhelming the formation; allows the next doc to observe the previous one's sync). Per-doc metadata records `format_version=v2_posture` once converted, so an interrupted run can resume by skipping already-converted docs.
4. **Flip `format_version`.** When all docs are converted, write `format_version=v2_posture` to the collection's `_collection_configs` entry. LUB strengthen merge accepts the flip; concurrent edits to `format_version` on other peers also resolve via strengthen-only.
5. **Release the migration lock.** Remove the `migration_in_progress` marker. Subsequent reads/writes use the FieldCipher path.

If migration is interrupted, the lock remains and the operator re-runs the same RPC. There is no automatic retry — this is an operator-driven operation that requires explicit confirmation each invocation.

#### 7.4 Deprecation timeline

`format_version=v1_envelope` is supported for one major version after v2 ships. The runway is part of the ADR commitment, repeated in release notes for each version:

- **v2 (this ADR ships):** Both formats coexist. Default for new collections remains `v1_envelope`. Migration tool available.
- **v2.x (subsequent minor releases):** `--collection-format-default=v1_envelope` and v1_envelope collection auto-stamp emit deprecation warnings at sidecar startup. CHANGELOG entries name the upcoming removal.
- **v3 (next major):** `format_version=v1_envelope` support is removed. Sidecars refuse to start if `_collection_configs` contains any collection still stamped `v1_envelope`. Operators must complete migration before upgrading.

This gives operators a deliberate runway and prevents the silent-format-change failure mode the QA reviewer identified.

#### 7.5 Mixed-version formation behavior

| Scenario | Outcome |
|---|---|
| All peers on v1 (pre-this-ADR) | Legacy envelope. No change. |
| Mixed v1 / v2, operator has not opted in | v2 binaries still default new collections to `v1_envelope`. v2 reads the legacy envelope through `StoreCipher`-compatibility code. Existing collections unchanged. No shape divergence. |
| All peers on v2, operator has not opted in | Same as above — no shape change. |
| All peers on v2, operator opted in to `v2_posture` default on one peer | Collections created by that peer default to `v2_posture`. Other v2 peers read/write those via `FieldCipher`. **Existing collections remain `v1_envelope` until migrated.** Doc shapes do not diverge within any single collection. |
| All peers on v2, migration run on collection X | Collection X is `v2_posture`. All other collections still `v1_envelope` until individually migrated. Per-collection isolation by construction. |
| Mixed v2 / v3, any collection still stamped `v1_envelope` | v3 sidecar refuses to start. Migration is the only path forward — and migration cannot run while a v3 peer is unable to join, which forces the operator to either complete migration before the v3 rollout begins or pause the v3 rollout. |

The cross-cutting invariant: **doc shape within a single collection never diverges between peers in the same formation.** The `format_version` field is the gate; both old and new sidecars consult it before reading/writing, so they always agree on the wire shape for each collection's docs.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                       Application (gRPC client)                      │
│                                                                       │
│       PutDocument(coll, doc_id, json={"platform_type":"vehicle",     │
│                                       "lat":37.5, "lon":-122.3})     │
└────────────────────────────┬─────────────────────────────────────────┘
                             │ plaintext JSON over Connect/gRPC
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│                          peat-node sidecar                            │
│                                                                       │
│  1. Look up collection posture: `FieldValues`                        │
│  2. For each field, encrypt value with payload key (AES-256-GCM,     │
│     fresh nonce per field):                                          │
│       platform_type -> <ciphertext_1>                                 │
│       lat           -> <ciphertext_2>                                 │
│       lon           -> <ciphertext_3>                                 │
│  3. Call DocumentStore::upsert(coll, Document{ fields = {            │
│        "platform_type": ciphertext_1,                                 │
│        "lat":           ciphertext_2,                                 │
│        "lon":           ciphertext_3 }})                              │
└────────────────────────────┬─────────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│                peat-mesh substrate (AutomergeBackend)                 │
│                                                                       │
│  - In-memory Automerge doc holds the structure {field_name: ct}.     │
│  - On redb write: cipher.encrypt(automerge.save()) — wraps the       │
│    entire byte blob with the at-rest key (peat-mesh #124).           │
│    at-rest key = HKDF(FormationKey, …). Protection vs T3 depends     │
│    on FormationKey custody (config-file ≈ obfuscation only;          │
│    HSM/TPM/external ≈ real protection). No protection vs T4.         │
│  - On sync: serialise Automerge patches and send over TLS-wrapped    │
│    Iroh QUIC. Field NAMES travel plaintext; field VALUES (which      │
│    are already encrypted ciphertext) travel opaque.                  │
└────────────────────────────┬─────────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│       Remote peer (formation member, may or may not have key)         │
│                                                                       │
│  Receives Automerge patch:                                            │
│    {"platform_type": <ciphertext_1>, "lat": <ct_2>, "lon": <ct_3>}    │
│                                                                       │
│  - With payload key: sidecar decrypts each value, app sees plaintext.│
│  - Without payload key (T4): app sees the field NAMES but the VALUES │
│    are opaque bytes that cannot be decoded. Useful for auditors /    │
│    observers who need to know what data exists without reading it.   │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Consequences

### Positive

1. **T4 explicitly preserved across docs *and* attachments.** The accidental property of the old peat-node envelope (T4 visibility of field values only) becomes a deliberate, documented design tier extended to cover attachment payload bytes too (§Decision §6). A T4 peer sees structural metadata (field names, file_distributions metadata subject to its posture) but never the protected payload — for either docs or blobs.
2. **T3 protection is conditional, not absolute** — and the ADR is honest about it. Substrate-level at-rest encryption protects field NAMES too, *but only when the FormationKey is held outside the data image* (HSM, TPM-sealed, operator-provided escrow, or a separate filesystem the T3 attacker did not seize). In the default config-file deployment, T3 reads the FormationKey from config and derives the at-rest key; the substrate cipher is on-disk obfuscation rather than protection. T4 protection does not depend on the substrate cipher under any deployment — that property lives entirely in the per-field payload-key encryption (Positive #1).
3. **CRDT semantics work.** Per-field merges, structural diffs, all Automerge guarantees hold because field names are plaintext at the structural level.
4. **F2 filtering works.** Sidecar applies the predicate post-decryption; substrate-level filtering is bypassed but the sidecar always has the data the application asked for.
5. **Per-collection policy.** Different data classifications can coexist in one formation (e.g. `platforms` plaintext for searchability, `commands` full-opacity for confidentiality).
6. **Per-phase decisions explicit.** The matrix above gives every future contributor a single place to look up "what protects what."

### Negative

1. **Metadata leak for `FieldValues` collections.** A T4 peer knows WHICH fields exist on each doc, just not their contents. For collections where the field set itself is sensitive (e.g. revealing whether a node tracks a specific data type), `FullOpacity` is the answer — but `FullOpacity` defeats query.
2. **Filter is sidecar-side, not substrate-side.** Every change event for the collection is delivered to the sidecar regardless of match; the predicate is evaluated after field-level decryption. For high write rates with low filter selectivity this has measurable cost — likely fine in practice, worth measuring during implementation.
3. **Per-value nonce overhead.** Each field write generates a fresh 12-byte nonce + 16-byte GCM tag. For tiny fields the overhead is proportionally large; for typical attachment-style fields it's negligible.
4. **Two-key cognitive load.** Operators have to reason about FormationKey vs payload key. The default behavior (`FieldValues` when both are configured, `Plaintext` when only FormationKey) should make this safe by construction, but it's still more complexity than a single-key model.
5. **`Plaintext` posture exposure is wider than it appears.** Operators choosing `Plaintext` for query convenience must understand: the substrate cipher's protection is conditional on FormationKey custody (see Positive #2 and §Decision #1). Any FormationKey holder with disk access to *any* peer in the formation — including a compromised T4 peer with shell access, a backup of a peer node, or a T3 attacker who seized a host whose FormationKey lives in the same config-file image — can recover field-name **and** field-value plaintext for `Plaintext`-posture collections. The doc warning must read: *"`Plaintext` posture protects against offline disk attackers who do not hold the FormationKey AND do not have access to a node whose config holds the FormationKey. Any FormationKey holder with disk access to any peer recovers plaintext."*
6. **Attachment cross-formation dedup is lost.** Iroh content-addresses the ciphertext, and the ciphertext depends on `formation_id` (Deterministic mode) or per-submission salt (Randomized mode), so the same plaintext blob in two different formations produces different `blob_token`s and cannot be deduped across formations. Within a formation, Deterministic mode preserves dedup; Randomized mode does not. Acknowledged efficiency cost in exchange for the formation security boundary.
7. **Confirmation-of-equality leak in Deterministic mode.** A T4 peer (or any holder of ciphertext metadata) sees that two distinct `bundle_id`s referencing the same `blob_token` carry identical plaintexts, even though it cannot decrypt them. For tactical content where this is unacceptable (e.g., a sender doesn't want observers to know it re-transmitted the same image), the sender opts into Randomized mode at the cost of idempotency. The default is Deterministic because peat-node, peat-registry, and peat-sim all require idempotency or hash stability; the confirmation-of-equality leak is the explicit tradeoff documented at the RPC surface.

### Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Operators choose `Plaintext` posture for collections that should be `FieldValues` | Medium | Confidentiality regression | Default to `FieldValues` when payload key is configured; require explicit `Plaintext` selection. Document tradeoff in collection-config help text. |
| AES-GCM nonce reuse | Low | Catastrophic when same (key, nonce) pair encrypts *different* plaintexts (confidentiality breaks) | **Field values:** use OS RNG via `aes_gcm::aead::OsRng`. Never derive nonces deterministically on this path — different operators may concurrently write different plaintexts to the same field. Already the practice in peat-node's `StoreCipher`. **Attachments:** `RANDOMIZED` mode uses OS-RNG nonce salt → standard random-nonce AES-GCM. `DETERMINISTIC` mode is a synthetic-IV construction (§Decision §6.3): the nonce derivation includes `plaintext_sha256`, so same nonce occurs only when the plaintext is also identical — the catastrophic case (same key+nonce, different plaintexts) is structurally impossible. Birthday bound on 96-bit nonces vs distinct plaintexts within a formation is 2^48, far above any realistic deployment. |
| Side-channel: field NAMES leak schema/intent | Medium | Information leak | `FullOpacity` for sensitive collections. Doc warning that `FieldValues` is not full opacity. |
| Filter post-decrypt CPU cost dominates under churn | Low | Performance | Measure during peat-node #55 implementation; if measurable, add a per-collection "fast path" where the sidecar caches a deterministic hash of the encrypted value for equality checks. Out of scope for v1. |
| ADR-044 (MLS group-key rotation) conflicts with this design | Medium | Re-encryption cost on key rotation | MLS is for cell-level group keys; the payload key here is formation-wide. When MLS lands, the payload key becomes one of the resources MLS manages. The two ADRs need to be reconciled before both ship; coordinate in implementation. |
| Stale ChaCha20-Poly1305 references in ADR-006/044/048/049 + spec docs contradict driver #6 until amended | High (already true) | Reviewer confusion; risk of an implementer "fixing" ADR-060 back to the old AEAD on the basis of the un-amended ADRs | Track the amendments in §References → Follow-up amendments. Add the FIPS rule to `CLAUDE.md` and `SKILL.md` Hard Invariants so future agents and contributors see the constraint before they consult the un-amended ADRs. |
| FormationKey custody is load-bearing for substrate-cipher T3 protection | High in the default deployment | Default deployments (FormationKey in a config file alongside data) get substrate-cipher *obfuscation* against T3, not protection. Operators may believe they have stronger T3 guarantees than they do | (1) Document the custody dependence in §Decision #1, §Decision #3 row 4, §Decision #4 (already done in this revision). (2) Surface "FormationKey custody mode" as an explicit operational concept in peat-mesh config: `formation_key_source = config_file` (default) vs `external` (HSM, TPM-sealed, environment-injected). When the external path is selected, document the recovered T3 guarantee. (3) Track operational guidance + the HSM / TPM integration design as a follow-up (see §Open Questions #6). |
| Operator confusion: "Why did my `SetCollectionConfig(Plaintext)` not take effect?" | Medium | Operators trying to weaken posture via the strengthen RPC will see their edits silently no-op'd at LUB read time (with an audit entry they may not check). Confusion + misplaced trust that posture is what they last set | (1) `SetCollectionConfig` MUST detect downgrade attempts at the RPC layer and return `InvalidArgument` with an explicit pointer to `DowngradeCollectionPosture` (do not accept the write and then silently LUB-reject it). (2) `GetCollectionConfig` returns the LUB-resolved value, not the raw posture-field value; document this in the RPC reference. (3) Operator console / CLI surfaces the audit stream prominently. |
| Mixed-shape docs after a legitimate downgrade (Rule-2 path) | Medium | After a downgrade from `FullOpacity` → `Plaintext`, existing encrypted fields on docs stay encrypted while new fields are written plaintext. App code that assumes uniform shape may misbehave; readers may mistake "old encrypted field" for "missing data" | (1) Document this explicitly in the `DowngradeCollectionPosture` RPC docs and in operator guidance. (2) Provide an optional `--re-emit-collection` admin tool (Phase D follow-up) that walks every doc, reads via current payload key, and re-writes under the new posture. Tool must require explicit confirmation and log every re-emit to the audit stream. (3) Sidecar's Get/Subscribe paths must be schema-agnostic with respect to encrypted vs plaintext fields so app code does not silently misread. |
| Sender selects wrong attachment encryption mode | Medium | Choosing `RANDOMIZED` where idempotency is required (peat-node `SendAttachments` with a stable `bundle_id`, peat-registry OCI layer sync, peat-sim scenario replay) silently breaks the consumer contract — duplicate submissions produce distinct `blob_token`s, scenarios stop replaying deterministically, OCI digests drift | (1) Default is `DETERMINISTIC` everywhere; opt-in for `RANDOMIZED`. (2) peat-registry hard-codes `DETERMINISTIC` (no API to override — OCI semantics make Randomized always wrong here). (3) peat-node `SendAttachments` proto doc explicitly states the `bundle_id` idempotency contract holds only for `DETERMINISTIC`. (4) Phase E test suite includes both "Deterministic-mode dedup" and "Randomized-mode anti-dedup" tests so accidental mode flips fail loudly in CI. |
| AttachmentCipher header tampering | Low | Bit-flip in unprotected header bytes (everything before `chunk_ciphertext_0`) could trick the decrypt path into wrong `chunk_size_log2`, wrong mode, wrong `plaintext_length`, wrong `payload_key_id` | Header is not GCM-AEAD-protected at the sidecar layer (it's data the receiver needs *before* it has chosen a chunking strategy). Mitigations: (1) AAD on chunk 0 covers `chunk_index_0 || plaintext_sha256 || formation_id` — a wrong `plaintext_sha256` in the header desynchronizes the AAD and chunk-0 decryption fails with the standard GCM auth error. (2) The substrate Iroh-blob cipher wraps the entire blob including the header in deployments where it's enabled — non-trivial header tampering also fails the substrate layer's GCM check. (3) Header-validation test (Phase E test #5) explicitly verifies each field's rejection path. |
| Long-tail blobs exceed nonce safety bound | Negligible at current limits, would matter if `DEFAULT_MAX_FILE_BYTES` is ever raised | Birthday bound on 96-bit AES-GCM nonces is 2^48 distinct plaintexts per (key, formation). Current 256 MiB cap × 4096 chunks per blob means a single payload key would need ~2^36 blobs in one formation to approach 2^48 nonce-set size. Comfortable margin today; could become tight if the cap is raised to multi-GiB | (1) Cap stays at 256 MiB for v1 (peat-node default). (2) If raised, re-evaluate the nonce-collision margin and possibly switch to extended-nonce constructions (XChaCha20-Poly1305 is non-FIPS; AES-GCM-SIV is non-FIPS as of writing — at that point an ADR addendum is needed, not a silent raise). (3) Add a peat-mesh runtime check that refuses to encrypt if the per-key formation blob count exceeds a conservative cap (e.g., 2^32), forcing payload-key rotation before the nonce pool is meaningfully consumed. |
| Operator skips migration window, v3 upgrade lands on a formation with unmigrated collections | Medium | v3 sidecar refuses to start; operators discover unmigrated collections at upgrade time rather than in advance. Service interruption. | (1) v2.x startup warnings naming each unmigrated collection (Phase F). (2) Release notes for v2.x and v3 reiterate the requirement. (3) `peat-node migrate-collection --status` admin command lists pending migrations. (4) v3 rollout playbook (in operational docs) requires running `migrate-collection --status` across all peers and resolving any unmigrated collections before v3 upgrade begins. |
| Migration runs while some peer is on a pre-v2 sidecar | Low (Phase F gate prevents this) | A new-shape doc lands in `_collection_configs`; old peer treats it as unknown and may misread or refuse to sync the collection | Phase F step 1 (`ListPeerVersions` gate) blocks the migration RPC unless every peer reports v2-or-newer. If a peer is offline / unreachable during the version check, the migration is rejected (the offline peer might be on v1; cannot be presumed safe). Operator must verify the offline peer's version out-of-band before forcing the migration with a future `--ignore-offline-peers` flag (not in v1; would require its own design). |
| Backfill misreads a non-peat-managed `_collection_configs` record | Very low | The startup backfill (§Decision §7.1) blindly adds `format_version=v1_envelope` to every collection lacking the field; if some unrelated tool ever wrote to `_collection_configs` with a different schema, the backfill could clobber meaningful data | (1) `_collection_configs` is peat-managed; no other tool should write to it. Document this. (2) Backfill checks for a recognized peat-node-written marker (e.g., the presence of any pre-existing peat config field — `posture`, deletion-policy, TTL) before adding `format_version`. Collections with no peat-recognized fields are left untouched + logged as "unknown shape, skipping backfill". (3) Phase F test #1 (backfill idempotence) explicitly covers the empty-collection-config case. |

---

## Alternatives considered

### Alternative 1: Whole-doc envelope at the substrate (Path Z, current Phase 2 working tree)

Substrate encrypts the entire Automerge byte-blob; sync exchanges structural plaintext patches. **Rejected** because it removes T4: a formation member without the payload key sees plaintext after sync.

### Alternative 2: Keep peat-node's whole-doc envelope as today

`{"value": "ENC:v1:<base64>"}` wrapper at the sidecar. **Rejected** because it defeats per-field CRDT merges (everything is one string at the Automerge level) and makes F2 filtering structurally impossible.

### Alternative 3: Wire-level encryption of sync patches (encrypted Automerge protocol)

Modify the sync coordinator to encrypt patch payloads with the payload key before sending and decrypt on receipt. **Rejected** because:
- Automerge's sync protocol is designed around plaintext op streams; encrypting at this layer means peers that need to merge concurrent edits must all decrypt locally first, defeating CRDT semantics for T4 peers.
- A T4 peer that can't decrypt would have to refuse the sync entirely or store opaque patches that can't be applied. The former defeats their auditor role; the latter creates a substrate-level "encrypted patch limbo" with no clear semantics.

### Alternative 4: Searchable encryption (deterministic / order-preserving / homomorphic)

Encrypt field values such that the substrate can still evaluate `Eq` (deterministic) or `Lt`/`Gt` (order-preserving) without decrypting. **Rejected** for v1 because:
- Deterministic AES-GCM defeats semantic security (same plaintext → same ciphertext leaks equality across docs).
- Order-preserving encryption leaks ordering across docs (a stronger leak than expected).
- Real homomorphic schemes have ~100× overhead and aren't tactical-edge appropriate.

If a future use case demands substrate-level filtering on encrypted values, revisit; out of scope here.

### Alternative 5: Per-collection separate redb databases keyed by payload key

Open a different redb file per collection-config posture, so the `Cipher` knows nothing about contents. **Rejected** as architecturally heavy — the file-handle/I/O cost and the cross-collection-query complications exceed the benefit, and the current single-file approach is well-tested.

---

## Implementation phases

### Phase A — Substrate at-rest cipher (peat-mesh #124, already drafted)

- `Cipher` trait in peat-mesh, AutomergeStore byte-I/O wrapping. Done in working tree.
- **Keep this work**, with a tweak: the substrate cipher's key derives from the FormationKey (HKDF, fixed context). The existing peat-mesh `FormationKey` infrastructure already exists; this just adds a derivation. Removes the "operators have to provide two unrelated keys" footgun.
- **Update peat-mesh #124's body** to reflect the honest framing established in §Decision #1: the substrate cipher provides **on-disk obfuscation against T3 in default deployments** and recovers genuine T3 protection only when the FormationKey is held outside the data image. It provides **no T4 protection** under any deployment (T4 derives the at-rest key from FormationKey the same way the local sidecar does). T4 protection comes from per-field payload-key encryption (Phase B), not from the substrate cipher. Note explicitly that sync exchanges patches whose values may be opaque to T4 peers — the substrate's job is only to wrap the local redb blob.

### Phase B — Sidecar field-value cipher in peat-node

- Add a `FieldCipher` impl alongside the existing `StoreCipher`:
  - Encrypts individual field values to/from byte blobs.
  - Decrypts on read (Subscribe forward path, Get, query results).
- **Keep `StoreCipher` in the codebase for legacy-envelope read/write compat** (per §Decision §7). Sidecar dispatches based on the collection's `format_version`: `v1_envelope` → `StoreCipher`; `v2_posture` → `FieldCipher`. Both paths share the same payload key.
- Introduce per-collection posture and `format_version` in peat-node's collection config.
- Default to `FieldValues` when payload key is configured **AND `format_version=v2_posture`** (per §Decision §7.2 opt-in).
- Update `put_document` / `get_document` / `list_documents` / `forward_store_changes` to dispatch through the collection's `format_version` and apply the appropriate cipher.
- Filter (F2) runs at the sidecar: `observe(c, &Query::All)` from substrate, decrypt each event's fields per `format_version`, apply predicate, emit.

### Phase C — Update `cross_peer_encryption_test`

- Rewrite the test to assert the new, deliberate invariant:
  - Formation member with payload key sees plaintext field values.
  - Formation member without payload key receives docs structurally (field names visible) but field values are opaque ciphertext.
  - Substrate-level on-disk inspection finds neither plaintext field names nor plaintext field values (substrate cipher wraps the lot).

### Phase D — Per-collection posture config wire-up

- Surface `GetCollectionConfig` / `SetCollectionConfig` / `ListCollectionConfigs` RPCs on peat-node (the read-only half ships as part of peat-node #55; the write half lands here).
- Add a separate `DowngradeCollectionPosture` RPC implementing the CAS-guarded explicit-downgrade protocol from §Decision §2 Rule 2. The strengthen path uses `SetCollectionConfig`; the weakening path requires the dedicated RPC.
- The collection-config struct gains a `posture` field alongside the existing deletion-policy / TTL axes.
- Per-collection posture is persisted in a reserved Automerge collection (`_collection_configs`) — replicated across the formation so all peers agree on posture for each collection.
- **Posture-read path** (sidecar): when resolving the current posture for a collection, walk the Automerge change history of the posture field and compute the **strengthen-only LUB** over concurrent branches (§Decision §2 Rule 1). Do not use Automerge's native LWW value. Cache the resolved posture per (collection, history-head) tuple to avoid re-walking on every read.
- **Downgrade-record path** (sidecar + substrate): the downgrade RPC writes a dedicated `{kind: "downgrade", collection, from, to, operator_id, justification, lamport_clock, timestamp}` record to `_collection_configs`. Receiving peers apply the downgrade only when their LUB-resolved current posture equals `from` (CAS); otherwise reject + audit-log.
- **Audit-log surface:** rejected downgrade attempts (LUB-rejected at merge time, CAS-rejected at apply time, or sync-RTT-aborted) emit `downgrade.rejected.*` events to a per-collection audit stream that operators can read via a future `GetCollectionAuditLog` RPC (track as Phase D follow-up; not strictly required for the safety property but required for operator visibility).
- **Test surface:** Phase D must land with a substrate-level concurrency test that runs the worked example from §Decision §2 (T=0..T=6) and asserts the LUB outcome at T=3 and the CAS-rejection paths at T=5 under contrived clock-skew conditions. Without this test, the safety property is unverified.

### Phase E — Attachment encryption (now in-ADR; see §Decision §6)

The design is committed in §Decision §6. Phase E rolls out the implementation:

**peat-mesh (substrate):**

- `AttachmentCipher` trait alongside the existing redb-blob `Cipher` trait. Implemented in peat-mesh; consumed by peat-node and any other sidecar that hands blobs to Iroh.
- Substrate Iroh-blob cipher (FormationKey-derived, parallel to the redb cipher). Reuse the HKDF context discipline from §Decision §4 / §Open Questions #2: context string `"peat-mesh/iroh-blob-at-rest/v1"`. **Versioned** for the same forward-compatibility reasons.
- Header serialization (`AttachmentHeader` from §Decision §6.2): little-endian, fixed-size, validated at decrypt time. Reject unknown `magic`, unsupported `version`, unsupported `chunk_size_log2`, unknown `mode`.

**peat-node (sidecar):**

- New `AttachmentCipher` impl in peat-node, sibling to the existing `StoreCipher` (`src/crypto.rs`). Same FIPS-approved AES-256-GCM primitive; chunked construction per §Decision §6.2; mode selection per §Decision §6.4.
- `proto/sidecar.proto`: extend `FileSpec` with `EncryptionMode encryption_mode = N` (enum: `DETERMINISTIC = 0` (default), `RANDOMIZED = 1`). Add to `bundle_id` idempotency-contract doc (`proto/sidecar.proto:572-577`) that the contract holds only for `DETERMINISTIC`; `RANDOMIZED` resubmissions produce distinct `blob_token`s by design.
- Ingest pipeline (`src/attachments/ingest.rs:136-159`): tee the input stream through (a) the existing plaintext SHA-256 computation and (b) the chunked `AttachmentCipher`. The encrypted byte stream is what gets passed to `BlobStore::create_blob_from_stream`. `blob_token` returned by Iroh is BLAKE3 of ciphertext. The sidecar stores the plaintext_sha256 → ciphertext_blob_token mapping in the `file_distributions` doc.
- Receive path: pull ciphertext from Iroh, validate header, decrypt chunk-by-chunk into the inbox file. The 256 MiB per-file cap (`DEFAULT_MAX_FILE_BYTES`) still applies to plaintext length; reject any header whose `plaintext_length` exceeds the configured cap before decrypting.

**peat-registry:**

- `RegistryClient` trait gains an encrypt-on-push / decrypt-on-pull adapter (§Decision §6.5). The adapter sits between the OCI HTTP transport (plaintext, OCI-digest-addressed) and the peat-mesh transport (ciphertext, BLAKE3-addressed).
- Resumable-checkpoint path (`src/transfer/checkpoint.rs`): `PartialBlob.bytes_transferred` is interpreted as a **plaintext** offset. Resume re-encrypts starting from chunk `floor(N / 64KiB)` and discards the prefix bytes within that chunk that were already delivered. Checkpoint format unchanged.
- Always Deterministic mode for OCI layers (no API surface needed in peat-registry config; hard-coded).

**peat-sim:**

- `NetworkedIrohBlobStore::create_blob_from_bytes` gains an `encryption_mode` parameter (default `DETERMINISTIC`, preserves existing scenario-replay reproducibility for the `image_chip_hash` field on red-track scenarios).
- The WIP `feat/7n-dual-c2-concurrent-telemetry` branch should pick up the parameter on rebase but does not need behavior changes — its assertions (filesystem round-trip identity) work in both modes.

**Tests required to merge Phase E:**

1. **Round-trip test** (peat-node): submit blob via `SendAttachments`, pull from a second peer, assert plaintext equality. Run for both modes. Run for sizes spanning the chunk boundary (e.g., 1 byte, 64 KiB – 1, 64 KiB, 64 KiB + 1, 256 MiB).
2. **Deterministic-mode dedup test** (peat-node): submit the same plaintext twice from two different `bundle_id`s in the same formation, assert identical `blob_token`. Submit the same plaintext from two different formations, assert distinct `blob_token`. **This is the test the peat-sim WIP currently does not exercise; merging Phase E without it leaves the dedup property unverified.**
3. **Randomized-mode anti-dedup test** (peat-node): submit the same plaintext twice in `RANDOMIZED` mode, assert distinct `blob_token`s.
4. **T4 wire-opacity test** (cross-peer, parallel to `cross_peer_encryption_test`): a peer in the formation without the payload key receives a synced blob, asserts it can read `file_distributions` metadata (subject to that collection's posture) but cannot recover plaintext bytes from the Iroh blob store.
5. **Header validation test** (peat-mesh): corrupt each header field in turn, assert decrypt rejects with the expected error.
6. **Chunk-tag tampering test** (peat-mesh): flip a bit in a ciphertext chunk's GCM tag, assert decrypt rejects (GCM authenticity).
7. **OCI digest stability test** (peat-registry): push a blob with a known OCI sha256 digest through the encrypt-on-push adapter and back through decrypt-on-pull; assert the OCI digest observed by the destination registry client matches the source. Run for layers sized 1 MiB, 10 MiB, 100 MiB, 1 GiB.
8. **Checkpoint-resume test** (peat-registry): interrupt a transfer at a chunk boundary and at a mid-chunk byte; resume; assert correct plaintext at destination + checkpoint format unchanged.
9. **Scenario-replay determinism test** (peat-sim): run the red-track scenario twice with `DETERMINISTIC`, assert identical `image_chip_hash` across runs.

Tests 1, 2, 3, 4, 5, 6 are merge-blockers for Phase E. Tests 7, 8, 9 are merge-blockers for the consuming repos (peat-registry, peat-sim) but ride in their respective Phase E follow-up PRs per the cross-repo workflow.

### Phase F — Legacy-envelope migration (see §Decision §7)

The v1 → v2 transition path. Ships alongside or after Phase D.

**peat-node sidecar:**

- `format_version` field added to the per-collection config struct alongside `posture` and the ADR-034 deletion-policy axis. Stored in `_collection_configs`.
- One-shot backfill at sidecar startup: any collection in `_collection_configs` lacking a `format_version` field gets `v1_envelope` written via a single Automerge edit per collection. Backfill is idempotent (skip collections that already have the field) and runs before the sidecar accepts client RPCs. Logged at INFO level so operators see what happened.
- New `--collection-format-default` flag (default `v1_envelope`; opt-in `v2_posture`).
- Dispatch layer in `put_document` / `get_document` / `list_documents` / `forward_store_changes` consults `format_version` per collection (Phase B-coupled).
- New `ListPeerVersions` RPC: returns the sidecar version of each peer in the formation. Lightweight; cached per peer with a TTL. Used by the migration tool's pre-check.
- New `MigrateCollection` RPC implementing §Decision §7.3 steps 1–5. Operator-driven, requires `--confirm` flag, idempotent across retries (resume from the `migration_in_progress` marker + per-doc `format_version` field).
- `peat-node migrate-collection` CLI subcommand wrapping the RPC.

**peat-mesh:**

- `_collection_configs` schema gains the `format_version` field. LUB-merge implementation (Phase D) extended to also handle `format_version` strengthen-only merges (v1_envelope → v2_posture only).
- Migration lock primitive (`migration_in_progress` marker) — a small Automerge record with operator_id + lamport_clock + started_at. Concurrent migrations detect the marker and abort.

**Deprecation hooks:**

- v2.x minor releases: startup warning if any collection is stamped `v1_envelope` (visible in logs, exit code unchanged). Same warning if `--collection-format-default=v1_envelope` is set explicitly.
- v3 major release: sidecar refuses to start with a fatal error if any collection is stamped `v1_envelope`. Tracked as a separate v3 release PR; ADR-060 commits the timeline but the actual removal is gated on the next major.

**Tests required to merge Phase F:**

1. **Backfill idempotence test** (peat-node): start sidecar against a pre-ADR `_collection_configs` (no `format_version` field); assert all collections get `v1_envelope`. Restart; assert no additional edits.
2. **Default-flag behavior test** (peat-node): create collection with `--collection-format-default=v1_envelope`; assert `v1_envelope`. Same with `v2_posture`; assert `v2_posture` + correct posture default.
3. **Mixed-version read-compat test** (cross-peer): a v2 sidecar reading a `v1_envelope` collection from a v1 sidecar's prior state must succeed via the `StoreCipher` path. Asserted byte-for-byte plaintext equality.
4. **Version-check gate test** (peat-node): mock `ListPeerVersions` to return an old version on one peer; assert `MigrateCollection` returns `FailedPrecondition` with the offending peer list.
5. **Concurrent-migration abort test** (peat-node): start one migration; assert a second concurrent migration on the same collection returns `Aborted` with the in-progress marker's `operator_id`.
6. **Resume-after-interrupt test** (peat-node): kill the migration mid-collection; restart; assert it resumes from where it left off + only converts the remaining docs.
7. **Format-version monotonicity test** (peat-mesh, parallel to §Decision §2 Rule 1 concurrency test): submit a concurrent `v2_posture` → `v1_envelope` "downgrade" edit to `_collection_configs`; assert the LUB rejects it and the collection remains `v2_posture` after merge.
8. **End-to-end migration test** (peat-node): create `v1_envelope` collection with N docs, run migrate-collection to `v2_posture` with `FieldValues`, assert all docs are readable via the new path AND old-shape reads no longer succeed (since the v1 envelope no longer exists in the docs).

All eight are merge-blockers for Phase F.

---

## Open questions

1. ~~**Attachment encryption.**~~ **RESOLVED in §Decision §6** (rev 4, 2026-05-18): two-layer cipher (sidecar payload-key envelope for T4 + substrate Iroh-blob cipher for T3 with the same custody story as redb). Chunked AES-256-GCM, 64 KiB chunks. Two nonce-derivation modes: `DETERMINISTIC` (HKDF-derived nonce, preserves idempotency / OCI digests / scenario replay) and `RANDOMIZED` (per-submission random salt). Posture inherits from the referencing collection. Phase E rolled out inline; tests itemized in Phase E and gating Phase E merge. Design validated against peat-node `SendAttachments`, peat-registry OCI sync, and peat-sim's `feat/7n-dual-c2-concurrent-telemetry` WIP.
2. **At-rest key derivation context.** `HKDF(FormationKey, info="peat-mesh/at-rest/v1")` — version the context string so we can rotate the derivation function without rotating the FormationKey.
3. **Payload-key rotation.** ADR-044 (MLS) addresses cell-level group keys with rotation. The payload key in this ADR is formation-wide and configured at deployment. Future: integrate with MLS so payload-key rotation has a defined protocol. Tracked as ADR-044 follow-up.
4. ~~**Per-collection posture persistence and conflict resolution.**~~ **RESOLVED in §Decision §2** (rev 3, 2026-05-18): strict monotonic strengthen-only LUB merge for concurrent edits + explicit CAS-guarded `DowngradeCollectionPosture` RPC for weakening. See the worked example in §Decision §2 for the safety argument. The original framing (Lamport-clock with operator-notification-on-rollback) was unsafe because rollback happens after the affected writes are already on disk — too late.
5. **Discovery hardening.** mDNS records are plaintext; should they be signed (or encrypted) by FormationKey-derived material? Out of scope; separate ADR if a use case appears.
6. **FormationKey custody / external key store.** The substrate cipher's T3 protection collapses to obfuscation when the FormationKey lives on the same disk image as the data. A future enhancement is to define an `external` `formation_key_source` mode: FormationKey held in HSM, TPM-sealed, fetched at startup from a key-management service, or injected via a separate filesystem the T3 attacker did not seize. Open design questions: which custody surfaces are required (HSM, TPM, env, KMS); how peat-mesh's startup loads the FormationKey via each path; what the test matrix looks like for "T3 with config access but no FormationKey access"; whether this becomes a posture per-formation or per-peer; FIPS implications of the external store (HSMs are typically FIPS 140-3 validated, TPM/KMS varies). Tracked as a follow-up; for v1 of ADR-060, the design surfaces the custody-dependence explicitly but does not commit to an external mode.

---

## References

- [ADR-006: Security, Authentication, and Authorization](006-security-authentication-authorization.md) — DeviceKeypair, FormationKey, SecureChannel, GroupKey primitives
- [ADR-016: TTL and Data Lifecycle Abstraction](016-ttl-and-data-lifecycle-abstraction.md) — Per-collection config axis (lifecycle)
- [ADR-034: Record Deletion and Tombstone Management](034-record-deletion-tombstone-management.md) — Per-collection `DeletionPolicy` (sibling axis to the encryption posture introduced here)
- [ADR-042: Direct UDP Bypass Pathway](042-direct-udp-bypass-pathway.md) — Plaintext bypass framing; out of scope here
- [ADR-044: End-to-End Encryption and Key Management](044-e2e-encryption-key-management.md) — MLS group keys; payload key in this ADR is its formation-wide cousin
- [ADR-049: peat-mesh Extraction](049-hive-mesh-extraction.md) — Sync layer architecture
- peat-mesh #124 — `Cipher` trait + AutomergeStore at-rest hook (Phase A in this ADR)
- peat-node #55 — Filtered Subscribe via `DocumentStore::observe`; consumes Phases B and D
- `tests/cross_peer_encryption_test.rs` (peat-node) — Existing test that revealed the T4 invariant; updated in Phase C

### Follow-up amendments (FIPS posture, driver #6)

Driver #6 supersedes ChaCha20-Poly1305 references elsewhere in the ecosystem. The items below are all in the peat repo and can ride in this PR or bundle into a single follow-up PR — whichever fits the review story:

- **ADR-006** (`docs/adr/006-security-authentication-authorization.md:558,952`) — `ChaCha20Poly1305::new` in the code sample and the "Basic encryption (ChaCha20-Poly1305)" acceptance criterion. Replace with AES-256-GCM and resolve the latent contradiction with the existing FIPS 140-2/3 line at 992.
- **ADR-044** (`docs/adr/044-e2e-encryption-key-management.md:12,68,73,348`) — Peer-to-peer crypto line, MLS ciphersuite table row, OpenMLS rationale, `openmls_rust_crypto` provider comment. Select a FIPS-aligned MLS suite (e.g. `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`) and a FIPS-mode crypto provider.
- **ADR-048**, **ADR-049** — Secondary references; audit and amend.
- **`README.md`**, **`docs/spec/005-security.md`**, **`docs/whitepaper/10b-spec-appendix.md`** — Narrative references; amend in line with the ADR amendments.
- **`CLAUDE.md`**, **`SKILL.md`** — Hard rule + Hard invariant added in the same PR as this ADR so future agents and contributors see driver #6 before they consult the un-amended ADRs above.

**Status of the inline amendments (2026-05-18, rev 6):** all peat-repo-internal amendments landed in PR #870 commit `64dd033c` (ADR-006 / ADR-044 / ADR-048 / ADR-049 + README + ARCHITECTURE + spec + whitepaper + btle-slicksheet). The CLAUDE.md hard rule and SKILL.md hard invariant landed in commit `73cc9511`. The remaining open items are the cross-repo amendments and the FIPS-mode crypto-provider swap (see "Cross-repo coordination and follow-ups" below).

### Cross-repo coordination and follow-ups

This ADR commits ecosystem-spanning design decisions that touch sibling repos. Tracking the cross-repo work explicitly so the contracted shapes don't drift:

- **peat-mesh AEAD + DH primitive swap.** AES-256-GCM + ECDH-P256 in `src/security/encryption.rs` + `src/transport/bypass.rs`. Landed on `feat/cipher-trait-and-fips-aead-swap` (peat-mesh PR #125). When peat-mesh #125 merges, **peat-protocol** (workspace member in this repo at `peat-protocol/src/security/encryption.rs:42-46`) requires a coordinated one-line test update (`shared.as_bytes()` → `shared.raw_secret_bytes().as_slice()`); track as a follow-up commit in this repo's next workspace-deps bump.
- **peat-btle AEAD + DH primitive swap.** AES-256-GCM + ECDH-P256 in `src/security/mesh_key.rs` + `src/security/peer_key.rs` + `src/security/peer_session.rs`. Wire format `KeyExchangeMessage` grows 37 → 38 bytes. Landed on `feat/fips-aead-and-ecdh-swap` (peat-btle PR #62). Pre-production swap, no live peers — wire-compat work not needed.
- **peat-registry `RegistryClient` adapter.** §Decision §6.5 specifies an encrypt-on-push / decrypt-on-pull adapter on the `RegistryClient` trait with plaintext-offset checkpoint semantics. **Not yet acked by peat-registry maintainers** — file a tracking issue in peat-registry referencing this section + open a coordination thread before Phase E starts implementation. The RPC/trait shape committed here is the ecosystem-level decision; the sibling-repo PR may push back on specific signature details.
- **peat-sim `encryption_mode` parameter on `NetworkedIrohBlobStore::create_blob_from_bytes`.** §Decision §6.6 specifies the signature change. **Not yet acked by peat-sim maintainers** — same coordination path as peat-registry.
- **peat-node `SendAttachments` proto extension.** §Decision §6.4 + Phase E specify `EncryptionMode encryption_mode` on `FileSpec` + the narrowing of the `bundle_id` idempotency contract to `DETERMINISTIC` mode only. peat-node consumer-side amendment to land alongside Phase E.
- **FIPS-mode rustls/Iroh provider swap.** Driver #6 + §5 mandate that TLS/QUIC run under a FIPS-mode provider (e.g. `aws-lc-rs`), but Iroh's quinn/rustls stack today brings the default `ring` backend which is **not** FIPS-validated. Switching requires either (a) Iroh upstream changes, (b) workspace-level `patch.crates-io` overrides, or (c) `default-features = false` + explicit feature dance in peat-mesh / peat-node. Track as an explicit ADR-060 substrate task: assigned to peat-mesh as the canonical Iroh consumer; until it lands, "FIPS-mode aligned deployment" remains an *intent* at the binary level even though the algorithms are correct.

The cross-repo amendments table above will be removed from this ADR once each row is closed by a merged sibling-repo PR. Until then, future contributors and reviewers consulting ADR-060 see the open commitments rather than discovering them by archaeology.

---

## Decision log

**2026-05-18**: Initial draft. Substrate cipher (peat-mesh #124, already coded but not committed) repositioned as Phase A defense-in-depth, deriving its key from FormationKey via HKDF. New per-collection posture axis (`Plaintext` / `FieldValues` / `FullOpacity`) introduced as Phase B. Per-field encryption at the sidecar layer added as Phase B. Attachments deferred to a follow-up ADR. The Phase 2 peat-node working tree from the original "Path Z" approach is to be repurposed: keep the `DocumentStore` migration and the substrate `Cipher` trait, replace the sidecar-side `StoreCipher` with a field-level `FieldCipher`.

**2026-05-18 (rev)**: Added driver #6 (FIPS-approved primitives only) and §5 Cryptographic primitives (FIPS posture) in response to QA review (BLOCKER on AES-256-GCM vs ChaCha20-Poly1305). The QA finding identified a deviation from the documented ecosystem standard correctly, but under the FIPS constraint the ecosystem documents are what needs amending; AES-256-GCM stands. Tracked amendments in §References → Follow-up amendments. Added `CLAUDE.md` / `SKILL.md` hard-rule entries in the same PR so the constraint is visible to future agents/contributors before they read the un-amended ADRs.

**2026-05-18 (rev 6)**: Addressed residual QA review findings on PR #870. (1) Fixed two broken intra-doc ADR links in §References (`006-security-architecture.md` → `006-security-authentication-authorization.md`; `042-udp-bypass-channel.md` → `042-direct-udp-bypass-pathway.md`). (2) Added a normative MUST-clause on receiver `plaintext_length` verification in §Decision §6.2 — the chunk-level GCM AAD does not cover the header's length field, so a truncation attack against a flipped `plaintext_length` requires an explicit post-decode length check (or alternatively, header-in-AAD binding). (3) Restored the operational comments to `supply-chain/config.toml` (audit-as-crates-io footgun, `[policy.peat]` reserved-name flow rationale, Slice-4.d cutover note) that the alphabetization pass had stripped. (4) Added a "Cross-repo coordination and follow-ups" subsection in §References tracking the peat-mesh / peat-btle / peat-registry / peat-sim / peat-node + FIPS-mode rustls provider commitments explicitly so the sibling-repo work is visible rather than implicit. (5) `supply-chain/imports.lock` itertools 0.10.5 publisher entry confirmed trust-covered by the existing `[[trusted.itertools]] safe-to-deploy` audit block (no action needed).

**2026-05-18 (rev 5)**: Resolved WARNING #5 from QA (default-to-`FieldValues` silently changes wire/at-rest shape for existing `--encryption-key` operators). Added §Decision §7 "Migration from legacy envelope (v1 → v2 transition)" committing: (a) per-collection `format_version` axis (`v1_envelope` vs `v2_posture`) with strengthen-only LUB merge semantics matching §Decision §2 Rule 1; (b) explicit sidecar opt-in via `--collection-format-default` (default `v1_envelope`, so upgrades observe no behavioral change); (c) `peat-node migrate-collection` tool with a formation-wide version-check gate, migration lock, per-doc resume support, and operator confirmation requirement; (d) a deprecation timeline (v2 ships both formats, v2.x warns, v3 removes v1_envelope); (e) a mixed-version behavior matrix proving doc shape never diverges within a single collection. Phase B updated to keep `StoreCipher` for legacy-envelope read/write compat and dispatch by `format_version`. Added Phase F for migration with eight merge-blocker tests. Added Risks rows for skipped migration windows, mid-migration peer-version races, and backfill safety. §Decision §2 default-selection updated to scope the `FieldValues`-when-payload-key default to `v2_posture` collections only.

**2026-05-18 (rev 4)**: Resolved ARCH #3 from QA (attachments deferred while T4 is named load-bearing). Lifted attachment encryption out of "follow-up ADR" and committed the design inline as §Decision §6: two-layer cipher (sidecar `AttachmentCipher` for T4 + substrate Iroh-blob cipher for T3 with the same custody story as redb), chunked AES-256-GCM at 64 KiB chunks, FIPS-approved primitive. Two nonce-derivation modes — `DETERMINISTIC` (HKDF-derived nonce, synthetic-IV construction, preserves peat-node `SendAttachments` idempotency / peat-registry OCI digest stability / peat-sim scenario-replay reproducibility) and `RANDOMIZED` (per-submission OS-RNG salt, eliminates confirmation-of-equality leak at cost of idempotency). Design validated against peat-node, peat-registry, and peat-sim's `feat/7n-dual-c2-concurrent-telemetry` WIP. Phase E rewritten from placeholder to concrete implementation tasks per repo + a nine-test merge gate (six on peat-mesh/peat-node, three on the consuming repos). §Open Questions #1 closed out. §Consequences Positive #1 expanded to cover attachments; new Negative #6 (cross-formation dedup loss) and #7 (Deterministic-mode confirmation-of-equality). §Risks expanded with sender-mode-confusion, header-tampering, and nonce-safety-bound rows. The pre-existing "Per-field nonce reuse" risk row refined to distinguish the field-value path (always random nonces) from the attachment Deterministic-mode synthetic-IV path (safe by SIV reasoning because the nonce derivation includes `plaintext_sha256`).

**2026-05-18 (rev 3)**: Resolved ARCH #2 from QA (per-collection posture conflict resolution is security-load-bearing). Decided the CRDT semantics in this ADR rather than deferring to Phase D: strict monotonic strengthen-only LUB merge for concurrent posture edits + a dedicated `DowngradeCollectionPosture` RPC with CAS preconditions, one-sync-RTT debounce, and an audit-log record for the weakening path. Worked example pinned in §Decision §2 covering T=0..T=6 across two concurrent operators. §Open Questions #4 closed out and replaced with a pointer to §Decision §2. Phase D updated with the implementation tasks + the substrate concurrency test required to verify the safety property. Risks table expanded with operator-confusion + mixed-shape-doc rows.

**2026-05-18 (rev 2)**: Corrected the substrate-cipher framing throughout (ARCH #1 + WARNING #6 from QA). The at-rest key is `HKDF(FormationKey, …)`, so any FormationKey holder can derive it. T4 holds FormationKey by definition → substrate cipher offers **no T4 protection** under any deployment. T3 protection is **conditional on FormationKey custody**: default deployments with FormationKey in a config file get on-disk obfuscation only; hardened deployments with FormationKey held outside the data image (HSM, TPM-sealed, external escrow) recover real T3 protection. Removed "defense in depth" framing from §Decision #1, §Decision #3 row 4, and §Consequences Positive #2. Rewrote §Consequences Negative #5 (`Plaintext` posture warning) to match. Added a Risks row for FormationKey custody and §Open Questions #6 for the external-key-store follow-up.
