# ADR-065: Auth Handshake Version Negotiation and Capability Advertising

**Status**: Proposed
**Date**: 2026-05-30
**Authors**: Kit Plummer
**Related**: ADR-006 (Security model — defines the device-identity Ed25519 handshake this ADR extends), ADR-060 (FIPS posture)
**Triggered by**: [peat#952](https://github.com/defenseunicorns/peat/pull/952) QA review round-5 [ARCH] finding: "the auth handshake protocol carries no version negotiation token. […] every future correction to signed-message construction forces a coordinated full-fleet upgrade to avoid mutual-auth failures between mixed-version peers."

---

## Context

The mutual-authentication handshake between peer devices (`DeviceAuthenticator::generate_challenge` → `respond_to_challenge` → `verify_response`) signs a deterministic byte string with an Ed25519 keypair. Any change to that byte construction — including the rc.20 → rc.21 correction in peat#952 (signer's signed-message source changed from `challenge.timestamp.seconds` to `response.timestamp.seconds`) — makes pre-change and post-change peers mutually un-authenticatable. A rolling upgrade across a deployed mesh sees auth failures between old-code and new-code segments for the duration of the rollout window.

The pre-peat#952 protocol carried no field that would let the receiver detect "I am talking to a different version" — verification just failed with `InvalidSignature("Verification equation was not satisfied")`, the same error a tampered-with signature would produce. The receiver could not distinguish "peer is on an older protocol" from "peer's keypair doesn't match its claimed identity" from "wire was MITM-corrupted."

peat#952's CHANGELOG `[Unreleased]` operational note tells operators to upgrade all nodes together for that one transition. That works once, but cannot scale: every future signed-message tweak repeats the same incompatibility, and operators with large fleets cannot reliably coordinate simultaneous upgrades across the entire mesh.

This ADR establishes a forward-compat negotiation mechanism so subsequent protocol changes can roll out staggered-safely.

## Decision

Two additive fields land on both `Challenge` and `SignedChallengeResponse`:

```protobuf
message Challenge {
  // ... existing fields ...

  // Protocol version the challenger speaks. 0 = pre-rc.21+1 peers
  // (no version field, prost default). The responder negotiates
  // by picking `min(challenge.protocol_version, CURRENT_PROTOCOL_VERSION)`
  // and uses that version's signed-message construction.
  uint32 protocol_version = 5;

  // Capability strings the challenger advertises. Forward-compat
  // hook — see "Capabilities" below for the v1 semantics.
  repeated string capabilities = 6;
}

message SignedChallengeResponse {
  // ... existing fields ...

  // Protocol version covered by this signature. Set by the
  // responder to the negotiated version (the minimum of the
  // peer's advertised version and CURRENT_PROTOCOL_VERSION on
  // the responder's side). Included in the signed-message byte
  // string from version 1 onward so a MITM cannot downgrade
  // without invalidating the signature.
  uint32 protocol_version = 7;

  // Capability strings the responder advertises. Same v1
  // semantics as Challenge.capabilities — see below.
  repeated string capabilities = 8;
}
```

### Negotiation rule

At the responder:

```text
negotiated = min(challenge.protocol_version, CURRENT_PROTOCOL_VERSION)
```

The responder uses `negotiated`'s signed-message construction (see below), embeds `negotiated` in `response.protocol_version`, and signs.

At the verifier:

```text
let claimed = response.protocol_version;
if claimed > CURRENT_PROTOCOL_VERSION:
    Err(SecurityError::IncompatibleProtocolVersion { ours: CURRENT_PROTOCOL_VERSION, theirs: claimed })
else:
    reconstruct signed bytes using claimed's construction
    verify signature
```

A version mismatch surfaces as a distinct error variant from `InvalidSignature`, so operators can distinguish "peer is too new for me" from "signature was tampered with." See "Error type implementation" below for the v1 rc.21+1 shape and the typed-variant follow-up.

### Error type implementation

**v1 rc.21+1 shape**: the version-mismatch case returns `SecurityError::AuthenticationFailed` with a message of the form `"incompatible protocol version: peer claims X, our maximum is Y"` (X = `response.protocol_version`, Y = `CURRENT_PROTOCOL_VERSION`). This variant is already in `peat-mesh::security::error` and is distinct from `SecurityError::InvalidSignature` — its `code()` returns `"AUTH_FAILED"` vs `"INVALID_SIGNATURE"` — so the reviewer's "operators can distinguish 'peer is too new' from 'sig tampered'" requirement is satisfied.

**Typed-variant follow-up**: a future peat-mesh release will introduce a typed `SecurityError::IncompatibleProtocolVersion { ours: u32, theirs: u32 }` variant (analogous to the existing `ChallengeExpired(u64)`), eliminating the substring-match shape and giving callers a cleaner `matches!` pattern. Tracked as a polish follow-up so this ADR's implementation can land without a peat-mesh release round-trip; the wire format and security semantics are identical with or without the typed variant. The substring `"incompatible protocol version:"` is exported as `pub const INCOMPATIBLE_PROTOCOL_VERSION_PREFIX: &str = ...` in `peat-protocol/src/security/authenticator.rs` so external consumers can match deterministically pending the typed variant.

### Signed-message construction by version

**Version 0** (pre-rc.21+1 peers, default when field is absent):

```text
signed = challenge.nonce
      || challenge.challenger_id
      || response.timestamp.seconds
```

This is the rc.21 construction. Pre-peat#952 peers used a different construction (signer's bytes drew on `challenge.timestamp.seconds`); they could not authenticate with anyone post-peat#952. They are not represented in v0 — they're pre-protocol-version peers entirely, and operators who attempt to roll a pre-rc.21 mesh forward must do that one coordinated upgrade before this version-negotiation mechanism can help.

**Version 1** (rc.21+1+):

```text
signed = challenge.nonce
      || challenge.challenger_id
      || response.timestamp.seconds
      || response.protocol_version (u32 little-endian, 4 bytes)
```

Binding `response.protocol_version` in the signed bytes prevents downgrade attacks: a MITM cannot strip the field or change its value without breaking the signature. A pre-version-token peer's `protocol_version` is absent (defaults to 0 via prost) and falls through the v0 construction, which doesn't cover the field — so a v0 peer talking to a v1 peer negotiates down to v0 cleanly. A v1-aware peer cannot be tricked into using v0 against a v1 partner because the v1 verifier reconstructs from `claimed = response.protocol_version = 1` and uses the v1 construction.

`CURRENT_PROTOCOL_VERSION = 1` at the rc.21+1 release.

### Capabilities (v1 semantics)

`capabilities` is a sorted set of opaque ASCII identifier strings each side advertises. At v1 it is **advertised-but-not-signed**: consumers driving feature-flagged behaviour (e.g. "this peer supports the new cell-formation flow") can read `peer.capabilities` and adjust their behaviour, but the field is not part of the signed-message byte construction. A future v2 protocol revision may extend the signed bytes to cover `capabilities` once the canonicalisation format (sort order, separator, length-prefixing) is settled in a follow-up ADR.

The v1 scope deliberately stops at "advertise" rather than "negotiate-and-bind" because:

1. The v0 → v1 transition's goal is **to enable safe rollouts**. Binding capabilities adds a second moving target inside v1 itself.
2. A capability canonicalisation format is a separate design decision (sorting? UTF-8 normalisation? case sensitivity?) and locking it in alongside the version-negotiation mechanism would couple the two unnecessarily.
3. v1 consumers that read peer capabilities for feature-flagged behaviour are running soft policy (e.g. "if the peer doesn't support batched-cell-formation, fall back to per-cell"); soft policy doesn't need cryptographic binding. Hard policy (rejecting peers without a capability) is a stronger contract that v2 can introduce when there's a concrete need.

### CURRENT_PROTOCOL_VERSION bumps

A bump from N to N+1 requires:

1. A new signed-message construction (or other wire-level change) be specified in this ADR's revision history.
2. The verifier's match arm for the new version added.
3. The responder's negotiation logic updated to handle the new ceiling.
4. A regression test that pins the v(N-1) ↔ vN mixed-version case (no auth failure, falls through to v(N-1) construction).
5. A CHANGELOG note for operators: "rc.X bumps `CURRENT_PROTOCOL_VERSION` to N+1; mixed-version rollouts are supported between vN and v(N+1)."

A v1 peer talking to a vN peer for N > 1 falls back to v1's construction (because that's the highest the v1 peer knows), so as long as the negotiation logic preserves the "min(peer, ours)" rule, the v1 peer never tries to reconstruct an N>1 signed-message it doesn't know how to read.

## Alternatives considered

### A. No version token (status quo)

What rc.21 ships if this ADR doesn't land. Every signed-message-construction change requires a coordinated full-fleet upgrade. Acceptable for a small lab mesh; not acceptable for a fleet of any operational size. Rejected.

### B. Version token only, no capabilities field

Cheaper to implement; covers the immediate operational concern. But the capabilities field costs nothing (an empty `repeated string` adds zero wire bytes when unused) and lets us land the negotiation hook for v2 use without a second schema change. Rejected — the marginal cost of the field is negligible and pre-allocating the slot avoids a second schema bump for the same security message.

### C. Signed transcript including challenge, nonce, capabilities, response, all under one signature

Hardens the protocol substantially (a MITM-altered Challenge would fail signature verification even if it never reached the responder). But this is a much broader hardening pass: it requires the responder to sign over fields the challenger doesn't currently authenticate (challenge.nonce, challenge.capabilities), and it changes the wire format more aggressively. Deferred to a future ADR; orthogonal to this ADR's narrower scope of "stop mutual-auth failures during rolling upgrades."

### D. Capability negotiation as a separate handshake round

E.g., "challenger sends supported-capabilities, responder picks subset, both sides sign over the agreed set." Closer to TLS's ClientHello/ServerHello model. Rejected for v1 because v1 advertises capabilities but doesn't bind them — there's no negotiation outcome to commit to. v2's "bind capabilities in the signed bytes" decision will require this discussion to be resolved (does the responder pick? min-set? union?), but v1 doesn't need it.

## Implications

- **rc.21+1 mesh consumers** see a clean negotiation: a v1 peer talking to a v1 peer signs v1 construction; a v1 peer talking to a v0 peer (the rc.21 baseline this ADR rolls forward from) negotiates down to v0 and continues. No coordinated upgrade required beyond the one already-acknowledged rc.20 → rc.21 cut.
- **Pre-rc.21 peers** (using the broken `challenge.timestamp.seconds` construction) are NOT covered by v0 negotiation. The rc.20 → rc.21 jump remains a one-shot coordinated upgrade. Subsequent jumps (rc.21 → rc.22+, when v2 lands) are staggered-safe.
- **MITM downgrade resistance**: the version field is signature-covered from v1 forward, so a network attacker cannot force v1 peers to talk v0 with each other (they'd have to forge the signature). The remaining attack surface — forcing a v1 peer to talk v0 with a v0 peer — is benign because v0 IS the documented fall-through path.
- **Error type proliferation**: one new `SecurityError::IncompatibleProtocolVersion` variant. Existing consumers matching on `InvalidSignature` for legacy reasons may need to also match the new variant; documented in the CHANGELOG release-notes section.

## Acceptance criteria

- ADR landed in `docs/adr/` (this file).
- Schema fields added to `peat-schema/proto/security.proto` with explicit doc-comments naming the version semantics.
- `peat-protocol/src/security/authenticator.rs`:
  - `CURRENT_PROTOCOL_VERSION` constant.
  - `generate_challenge` sets `protocol_version = CURRENT_PROTOCOL_VERSION`.
  - `respond_to_challenge` negotiates `min(challenge.protocol_version, CURRENT_PROTOCOL_VERSION)`, signs the v-appropriate byte construction, embeds `negotiated` in `response.protocol_version`.
  - `verify_response` reads `response.protocol_version`, surfaces `IncompatibleProtocolVersion` for ceiling overshoot, reconstructs signed bytes using the claimed version's construction.
- `peat-protocol/src/security/authenticator.rs`: public const `INCOMPATIBLE_PROTOCOL_VERSION_PREFIX` so callers can deterministically detect the version-mismatch case via `SecurityError::AuthenticationFailed(msg).to_string().contains(...)` until the typed variant lands.
- Regression tests:
  - `v1_responder_accepts_v0_challenger` — verifier reconstructs v0 bytes when `response.protocol_version = 0`.
  - `v1_v1_roundtrip_uses_v1_construction` — signed bytes include the version field; modifying `response.protocol_version` on the wire breaks verification.
  - `v1_verifier_rejects_unknown_future_version` — `claimed > CURRENT_PROTOCOL_VERSION` surfaces `IncompatibleProtocolVersion`, not `InvalidSignature`.
- Spec docs (`docs/spec/005-security.md` §5.3, `docs/whitepaper/10b-spec-appendix.md` §5.3) updated to describe v0 vs v1 byte constructions and the negotiation rule.
- CHANGELOG `[Unreleased]` operational note updated: rc.21 also lands the version-negotiation framework, so rc.21 → rc.22+ jumps will be staggered-safe.

## Status flip

This ADR moves from **Proposed** to **Accepted** when:

1. The peat#952 PR merges with the implementation above.
2. The rc.21 release ships with `CURRENT_PROTOCOL_VERSION = 1` on disk and on the wire.
3. A v1 ↔ v0 mixed-mesh test runs cleanly in CI (existing tests cover v0 ↔ v0; the new pins above cover v1 ↔ v0 and v1 ↔ v1; a full mixed-mesh integration test in `peat-sim` is a separate follow-up).

Status flip recorded in this file when (1)–(3) are met.

## References

- [peat#952](https://github.com/defenseunicorns/peat/pull/952) — auth-timestamp second-boundary fix; the change that motivated this ADR's [ARCH] flag.
- ADR-006 — security model.
- ADR-060 §5 — FIPS posture (Ed25519 / SHA-256 / AES-GCM only; this ADR doesn't introduce new primitives).
- `peat-schema/proto/security.proto::SignedChallengeResponse` — the wire surface this ADR extends.
- `docs/spec/005-security.md` §5.3 — protocol-spec entry for the challenge-response handshake.
