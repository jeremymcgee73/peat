# ADR-073: Peer Ejection — Rayfish Design Review and ADR-056 Gap Analysis

**Status**: Proposed  
**Date**: 2026-07-07  
**Authors**: Kit Plummer  
**Related**: ADR-056 (Compromised Node Detection, Isolation, and Ejection), ADR-007 (Automerge-Based Sync Engine), ADR-044 (E2E Encryption & Key Management), ADR-048 (Membership Certificates)

---

## Context

[Rayfish](https://rayfish.xyz) is a recently released open-source P2P mesh VPN built on the same Iroh QUIC substrate as Peat. Its docs are public. ADR-056 was written without reference to Rayfish; this ADR documents a direct review of Rayfish's admission and ejection mechanisms against ADR-056's design and surfaces specific gaps.

Rayfish is not a coordination substrate — it operates entirely at the network layer (virtual IPs, TUN device, QUIC datagrams) and has no CRDT layer. The comparison is narrow: **how does Rayfish handle peer admission security and peer ejection, and what does that reveal about ADR-056?**

---

## Rayfish Admission and Ejection: Summary

### Identity model

Every peer is an Ed25519 keypair on disk. The network's "room id" is the network's Ed25519 public key — a discovery key published to a DHT. On a closed network the room id is not an admission credential; it only lets peers find the network state.

### Admission

Three paths on a closed network, all gated by the coordinator (holder of the network secret key):

| Path | Mechanism | Notes |
|---|---|---|
| One-time invite code | `bs58(room-id ‖ coordinator-id ‖ secret)`, burned atomically on redemption, stored in coordinator-only ledger, never in shared state | Multi-coordinator gossip of mint/redemption |
| Reusable key | Multi-use, expiring; only its *hash* rides the signed record; secret not recoverable from the record | Revoke propagates via signed record |
| Live approval | Peer queues as pending; coordinator runs `ray accept` | Background retry until welcomed |

### Ejection (`ray kick`)

1. Coordinator removes the target from the signed network record and republishes (Ed25519-signed — only coordinators hold the secret key).
2. Coordinator severs its own connection to the target immediately.
3. Every surviving member re-converges from the freshly published record and **actively closes** its connection — not just stops routing.
4. The kicked node is told it was removed and stops reconnecting.

### Key constraints Rayfish documents explicitly

- **Coordinator cannot kick coordinator.** Kicking removes from the roster but does not revoke the network secret key. To remove a co-coordinator, you must rotate the key — explicitly unimplemented in the current release.
- **Kick without credential revocation is not permanent.** A kicked node whose original invite or reusable key was not also revoked can re-request entry through the normal admission path.
- **Ejection is eventually consistent, not instantaneous.** Nodes in a partition that has not yet received the updated signed record continue accepting the kicked node. Documented explicitly as a known limitation.
- **Reconnect window is a known gap.** Packets to a disconnected peer are silently dropped for up to 30 seconds during reconnect backoff. The security model section explicitly calls this "not protected."

---

## Comparison with ADR-056

### Where ADR-056 is stronger

| Dimension | Rayfish | ADR-056 |
|---|---|---|
| Propagation substrate | Single signed record re-published by coordinator; peers poll/converge | Grow-only revocation G-Set in Automerge; propagates via normal CRDT sync |
| Decentralized ejection | Coordinator-only | Threshold voting (Layer 3): cell members can eject without any coordinator online |
| Detection | None — ejection is always operator-initiated | Behavioral scoring + equivocation detection (Layers 1–2); automated ejection on cryptographic proof |
| Forward secrecy | None | MLS epoch advancement on removal (Layer 5) |
| Transitive revocation | Not addressed | ENROLL-delegate cascade (Layer 4b) |
| Evidence | None | Equivocation proofs attached to revocation proposals |
| Post-partition CRDT audit | Not applicable (no CRDT layer) | `revoked_change_audit/` map for changes authored by revoked nodes |

### Where Rayfish is cleaner — and what ADR-056 should address

**1. The coordinator-cannot-kick-coordinator problem is explicit in Rayfish and implicit in ADR-056.**

Rayfish documents plainly that kicking a co-coordinator is refused because kicking removes from the roster but does not revoke the key. ADR-056 Layer 3 says an ADMIN-tier node can enact revocation immediately, but does not address what happens when the node to be ejected *is* an ADMIN-tier node holding the network key. This is the same problem. The consequence in Peat's model: a revoked ADMIN node can still publish valid signed mutations (Layer 1), issue valid MLS proposals, and if it holds a formation-level key, re-admit itself or others.

**Resolution needed**: ADR-056 should explicitly specify that ejecting an ADMIN node requires key rotation at that tier before the tombstone is considered complete. The tombstone alone is insufficient.

**2. The permanent-bar gap is explicit in Rayfish and unaddressed in ADR-056.**

Rayfish states: "A kick doesn't bar permanent re-entry. To bar permanently, also revoke the invite or reusable key it used." ADR-056's revocation G-Set is keyed by `EndpointId`, not by admission credential. A revoked node that re-enrolls with a new keypair (explicitly permitted: "Re-enrollment with fresh credentials and explicit authority approval is required") gets a new `EndpointId` and is not in the revocation set. This is correct behavior — but it means the *credential* that originally admitted the node (membership certificate, invite analog) must also be invalidated. ADR-056 Phase 1 implementation should verify this is handled at the ADR-048 certificate layer.

**3. The partition acknowledgment in Rayfish is more operationally honest than ADR-056's treatment.**

Rayfish's security model section has an explicit "What is NOT protected" list that includes the reconnect window and partition-delayed ejection. ADR-056 covers partition behavior under Layer 4 propagation and the MLS partition limitation, but does not have an equivalent consolidated "known limitations" section. For a document the team will use as implementation reference, this matters — implementers and operators need to know where the guarantees end.

**4. Rayfish's connection-close-is-active behavior is worth calling out in ADR-056's Phase 1.**

When Rayfish kicks a node it does not just stop routing to it — it *actively closes* every surviving member's connection. ADR-056 Layer 4 says "all nodes that receive the enacted revocation via CRDT sync immediately drop connections to the target," which is equivalent. But the implementation note in ADR-056's Phase 1 scope ("Enforce revocation checks on sync receive and connection") could be read as passive. An explicit implementation requirement — that connection teardown is active, not just a gate on new connections — is worth adding to the Phase 1 implementation checklist.

---

## Decision

No change to the ADR-056 design. The five-layer architecture is sound and materially stronger than Rayfish's model in every dimension that matters for Peat's DIL operational context.

Three items require follow-up before Phase 1 implementation:

**Action 1 — Document ADMIN-tier ejection as requiring key rotation.**  
ADR-056 Phase 1 scope ("commander-initiated ejection via admin API") should explicitly note that ejecting an ADMIN-tier node is a two-step operation: tombstone + key rotation at that tier. A tombstone alone does not prevent a revoked ADMIN from continuing to operate with its formation key. This should be captured as an open question on ADR-056 or in ADR-044, where key rotation is in scope.

**Action 2 — Verify ADR-048 credential invalidation on revocation.**  
Confirm that the ADR-048 membership certificate layer invalidates the revoked node's certificate independently of the revocation tombstone, so that re-enrollment with a new keypair does not inherit the old certificate's admission chain.

**Action 3 — Add a "Known Limitations" section to ADR-056.**  
Modeled on Rayfish's "What is NOT protected" list, this section should explicitly state:
- Ejection is eventually consistent. Nodes in a partition that has not received the revocation tombstone continue accepting the ejected node until convergence.
- The reconnect window (up to the backoff maximum) represents a period during which a tombstoned node's in-flight packets may be processed before connections are torn down.
- MLS forward secrecy does not apply during a partition where the minority side has not yet processed the Remove Commit.
- CRDT changes already merged from a revoked node are not rolled back; they are auditable but not automatically undone.

---

## Consequences

ADR-056 is confirmed as the correct design direction. The Rayfish review surfaces three implementation-level clarifications that reduce risk of misinterpretation during Phase 1 build. No architectural changes are required.

---

## References

- Rayfish security model: https://rayfish.xyz/docs/20-security-model
- Rayfish membership: https://rayfish.xyz/docs/13-membership
- Rayfish network lifecycle (`ray kick`): https://rayfish.xyz/docs/18-network-lifecycle
- ADR-056: Compromised Node Detection, Isolation, and Ejection
- ADR-044: End-to-End Encryption and Key Management
- ADR-048: Membership Certificates and Tactical Trust
