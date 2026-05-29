# ADR-064: Deployment Formation Fall-Through for Unassigned Platforms

**Status**: Proposed
**Date**: 2026-05-29
**Authors**: Kit Plummer
**Related**: ADR-012 (Distributed Software/AI Operations — defines the deployment-directive surface this policy governs), ADR-018 (AI Model Capability Advertisement — defines the `CapabilityAdvertisement` shape carrying the optional `formation_id`)
**Triggered by**: [peat#773](https://github.com/defenseunicorns/peat/issues/773) (real `CapabilityMatcher` implementation; peat#942 QA review flagged the policy as undocumented)

---

## Context

`DeploymentDirective::targets(adv)` (introduced in peat#942 / peat#773) evaluates whether a directive applies to a candidate platform given its `CapabilityAdvertisement`. For `DeploymentScope::Formation(fid)`, the natural check is:

```rust
adv.formation_id.as_deref() == Some(fid)
```

But `CapabilityAdvertisement.formation_id` is `Option<String>` — platforms that have not yet been assigned to (or have not yet advertised assignment to) a formation publish `formation_id = None`. The question is what `targets(adv)` should return in that case under a `Formation` scope:

- **Conservative ("missing info → non-match"):** `None` returns `false`. The platform never receives formation-scoped directives until it advertises membership. Symmetric with how this PR treats missing `HardwareSpec` fields.
- **Optimistic ("missing info → fall through to issuer's formation"):** `None` returns `true` iff the directive's `issuer_formation_id` matches `fid`. The platform receives formation-scoped directives from issuers whose formation matches the scope, even without an explicit `formation_id` on its own advert.

Pre-peat#773 `DeploymentDirective::targets_node(node_id)` for the `Formation` arm read:

```rust
self.issuer_formation_id.as_deref() == Some(fid)
```

— it did not consult any platform-side advert at all, so every node received directives whose issuer's formation matched the scope, regardless of the receiver's own membership. The new `targets(adv)` strictly tightens this (it now considers the advert) but inherits the convention that **issuer formation membership is meaningful when the receiver hasn't yet self-identified**.

The peat#942 QA reviewer flagged the asymmetry against the new hardware-bound posture (`HardwareSpec` missing → non-match) and asked for an explicit decision recorded outside an inline comment.

## Decision

`targets(adv)` for `Formation(fid)` returns `true` if **either**:

1. `adv.formation_id == Some(fid)` — the platform has explicitly self-identified as a member of the scoped formation; or
2. `adv.formation_id.is_none() && self.issuer_formation_id == Some(fid)` — the platform has not yet advertised a formation, and the directive was issued by a node in the scoped formation.

Otherwise, `false`. Formally:

```rust
DeploymentScope::Formation(fid) => {
    adv.formation_id.as_deref() == Some(fid)
        || (adv.formation_id.is_none()
            && self.issuer_formation_id.as_deref() == Some(fid))
}
```

A platform that explicitly advertises membership in a *different* formation (`adv.formation_id == Some("other")`) does **not** fall through — explicit non-membership wins.

## Rationale

Formation membership and hardware advertisement are different semantic axes:

- **Hardware** is a *capability claim* the platform makes about itself. The absence of a claim is meaningful: "I have not introspected my GPU" is genuinely different from "I have a 4 GB GPU." Treating absence as non-match prevents the deployment system from acting on unverified hardware fitness.
- **Formation membership** is *provisioning state* set by an external orchestrator (issuer, operator, bootstrap script). The absence of a `formation_id` on the advert typically means the platform was recently powered on or hasn't yet completed assignment — not that it actively belongs to no formation. Treating absence as non-match would strand newly-provisioned platforms in a state where they cannot receive even their own issuer's bootstrap directives.

The fall-through addresses transitional bootstrap flows: an issuer in formation `alpha` cuts a directive scoped to formation `alpha`; a freshly-deployed platform with `formation_id = None` should be able to receive that directive (which may include the very configuration that sets its `formation_id`). Once the platform has been provisioned and starts advertising `formation_id = Some("alpha")`, the explicit check fires and the fall-through becomes irrelevant.

Equivalently: pre-peat#773 the system relied entirely on issuer-formation matching, with no advert check. This ADR's policy is strictly more conservative than that baseline — we now reject explicit membership in a *different* formation — while preserving the bootstrap semantics that downstream consumers (peat-node provisioning flows, peat-sim test fixtures) implicitly depended on.

## Alternatives considered

### A. Conservative (`None` → non-match)

Reject. Has the appeal of internal symmetry with the new hardware-bound semantics, but the symmetry is superficial — hardware and provisioning are different axes (see Rationale). The concrete cost is breaking every bootstrap flow that depends on the pre-peat#773 issuer-formation-only semantics. peat-node's auto-provisioning, peat-sim's lab fixtures that issue formation-scoped directives during node startup, and any operational deployment that issues a "join formation X" directive to a freshly-deployed platform would all silently stop working.

A future migration to this posture is possible once formation assignment is guaranteed to be set before any formation-scoped directive can land — e.g., once peat-node's startup contract is amended to require a `formation_id` advertisement before subscribing to deployment directives. That migration would be its own ADR.

### B. Fall-through with cell-level constraint

Reject. A finer-grained variant: `formation_id == None` falls through only when the platform's `cell_id` is also `None` (i.e., we're confident the platform is genuinely unprovisioned, not just partially provisioned). This adds operational complexity without addressing the underlying observation that provisioning state and capability advertisement live on different axes. The simpler binary check is adequate for the current scope.

### C. Explicit transitional flag

Reject. Add a boolean `bootstrap_eligible: bool` field to `CapabilityAdvertisement` that opts a platform into the fall-through explicitly. More surgical, but the cost (a new field on every advert, every consumer updated to set it correctly) outweighs the benefit (slightly less implicit semantics). Revisit if the fall-through produces operational confusion.

## Implications

### Operational

- **Unprovisioned platforms with `formation_id = None` will receive formation-scoped directives** from any issuer in that formation. This is the documented behaviour and is the same posture the pre-peat#773 system shipped under.
- **Platforms must explicitly advertise their formation** to receive directives that should not also reach unprovisioned hardware. If a platform belongs to formation `alpha` and an issuer in `bravo` cuts a `Formation("alpha")` directive that should NOT reach the `alpha` platform under any circumstance, the `alpha` platform must publish `formation_id = Some("alpha")` — both at startup and on advert refresh — to take the explicit-match path.
- **Tooling / provisioning contracts should set `formation_id` as early as possible** in the platform lifecycle. The fall-through is a safety net for the transitional state, not a steady-state expectation.

### Security

The fall-through means an unprovisioned platform with `formation_id = None` could receive operational instructions issued by any node in that formation, including model artifacts, configuration bundles, or executable code. This is not a new exposure — the pre-peat#773 system had the same property and worse (it didn't check the advert at all) — but it is a property that future tightening should consider.

A future ADR amending this one should pair `formation_id = None` → non-match with a separately-authenticated bootstrap channel that gates which directives a freshly-deployed platform can receive before formation assignment. Until that channel exists, the fall-through is the correct trade-off.

### Testing

`peat-protocol::distribution::directive::tests::targets_formation_scope_uses_advert_formation_id` pins the three cases:

- Explicit-match (`Some("alpha")` vs scope `Formation("alpha")` → match).
- Explicit-mismatch (`Some("bravo")` vs scope `Formation("alpha")` → non-match; explicit non-membership wins).
- Fall-through (`None` vs scope `Formation("alpha")` issued by a node in `alpha` → match).

Any future change to this ADR's policy must update those tests and produce a follow-up amendment ADR.

## Status flip

Flip to Accepted once the fall-through has been validated end-to-end in the peat-sim 7n-dual-c2 lab against a bootstrap scenario — a platform starts with `formation_id = None`, receives a formation-scoped directive that sets its `formation_id`, and then matches subsequent formation-scoped directives via the explicit-match path rather than the fall-through. The fixture lives in peat-sim; the scenario is documented as a follow-up in `peat-sim/experiments/7n-dual-c2/REPORT.md`.

## References

- [peat#773](https://github.com/defenseunicorns/peat/issues/773) — `CapabilityMatcher` implementation that introduced `targets(adv)`.
- [peat#942](https://github.com/defenseunicorns/peat/pull/942) — PR implementing peat#773; QA review surfaced the fall-through as undocumented.
- ADR-012 — Distributed Software/AI Operations, defines `DeploymentDirective`.
- ADR-018 — AI Model Capability Advertisement, defines `CapabilityAdvertisement`.
