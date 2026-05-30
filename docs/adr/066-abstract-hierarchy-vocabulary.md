# ADR-066: Abstract Hierarchy Vocabulary

**Status**: Proposed
**Date**: 2026-05-30
**Authors**: Kit Plummer
**Related**: ADR-024 (Flexible Hierarchy Strategies — defines `HierarchyLevel` enum that this ADR renames), ADR-021 (Document-Oriented Architecture — references `squad-{id}-summary`, `platoon-{id}-summary` document keys), ADR-027 (Event Routing & Aggregation Protocol — references `SquadSummaryCreated` events)
**Triggered by**: [peat#904](https://github.com/defenseunicorns/peat/issues/904) "epic: rename military-hierarchy terms (workspace-wide)", which extends the existing `CLAUDE.md` rule against consumer-specific references (ATAK/WinTAK/iTAK) to military-vocabulary references generally.

---

## Context

The Peat ecosystem currently uses military-hierarchy nouns — `Squad`, `Platoon`, `Company`, plus the `squad_id`/`platoon_id`/`company_id` identifier fields and `SquadSummary`/`PlatoonSummary`/`CompanySummary` protobuf messages — as the canonical names for aggregation levels in the mesh.

This naming is wrong for peat as a substrate for three reasons:

1. **Peat is a generic mesh protocol, not a tactical command-and-control framework.** Consumers include CoT/TAK plugins, wearables, CLI tools, server bridges, embedded sensor platforms, and (anticipated) civilian and industrial deployments. Military framing lifts one consumer's world-model into the API and signals to other potential consumers that the substrate isn't generic. This is the same concern the existing `CLAUDE.md` "no consumer-specific references in peat" rule addresses for ATAK/WinTAK/iTAK — just broader.
2. **The naming is internally inconsistent.** `peat-schema` already ships a `cell.proto` (`CellConfig`, `CellState`) and `peat-protocol` already ships a `Cell` model and `CellCoordinator`, while `hierarchy.proto` redundantly defines `SquadSummary` whose `squad_id` "matches `cell_id` in NodeState." `Squad` and `Cell` are the same concept named twice.
3. **The names presume a fixed unit-strength model.** Proto comments name specific counts ("typically 8 nodes," "1 Squad Leader, 5-7 Squad Members"). Peat's hierarchy is supposed to be size-agnostic — ADR-024's flexible-hierarchy strategies let any node operate at any level — but the type names encode an army-doctrine assumption that doesn't match the underlying flexibility.

The fix is to commit to one canonical, domain-neutral vocabulary for hierarchy levels and rename across the workspace. This ADR establishes that vocabulary and the migration strategy.

## Decision

### Hierarchy-level vocabulary

The ecosystem adopts an **abstract, topology-spirit** vocabulary for naming aggregation levels above the single-node level. The names describe *relationships among grouped peers* without committing to any single real-world domain (military, biological, K8s/cloud, civic, transport):

| Old (military)          | New (abstract)         | Meaning                                                                            |
| ----------------------- | ---------------------- | ---------------------------------------------------------------------------------- |
| `Platform` (single node) | `Platform` (unchanged) | A single peer / device. Already abstract; no rename.                              |
| `Squad`                 | `Cell`                 | Smallest aggregation. Consolidates with the existing `Cell` concept.               |
| `Platoon`               | `Cohort`               | A mid-level grouping — a set of cells sharing a mission, role, region, or window.  |
| `Company`               | `Federation`           | Group of cohorts coordinating with autonomy.                                       |
| `Battalion` (1000+ scale) | `Coalition`          | Group of federations coordinating for combined action. New tier in this ADR — required by deployments needing 1000+ peer scale. |
| `Regiment` / `Brigade` / `Division` / `Corps` (>1000 scale) | — | **Deferred to ADR-067 (parametric N-tier aggregation).** Naming and protocol shape for tier 5+ will be designed in a follow-up ADR; see "Tier-count extensibility" below. |
| `Fireteam` (sub-cell) | — | Not currently used in implementation code. If needed, addressed by ADR-067. |

The vocabulary is chosen because:

- **No single domain owns these terms.** *Cell* spans biology, organizations, hardware, security ops, and everyday English. *Cohort* spans statistics, epidemiology, software studies, and generic English ("group with a shared characteristic at a point in time"). *Federation* spans civic structures, software federated-identity / federated-learning ecosystems, and political alliances — and the federated-software sense is structurally what peat actually does (independently-managed cells coordinating without a central authority). None of these words are owned by a single deployment context.
- **K8s collision is avoided.** Kubernetes vocabulary (`Pod`, `Cluster`, `Fleet`) has strong directional baggage — a K8s `Pod` lives *inside* a `Node`, and a K8s `Cluster` is the *whole installation*. Reusing those terms would invert the aggregation direction (peat's Pod would *contain* nodes; peat's Cluster would be one of many per deployment). Architects coming from the K8s world would do double-takes. `Cell`/`Cohort`/`Federation` have no such collision.
- **Cell already exists.** The lowest-level rename is consolidation, not invention — `peat-schema/proto/cell.proto`, `peat-protocol/src/cell/*`, and `peat-protocol/src/models/cell/*` already use `Cell`. The new `SquadSummary` → `CellSummary` rename meets the existing `Cell` model, the existing `cell_id` field on `NodeState`, and the existing `CellCoordinator` runtime type. The half-done consolidation completes naturally.
- **The hierarchy is unambiguous.** A reader sees `Cell → Cohort → Federation` and reads scale progression (small → mid → top) without needing to import a domain analogy.
- **Coalition extends the federation pattern naturally.** A *Coalition* is "a temporary or enduring alliance for combined action, typically of autonomous parties." Federations already operate with local autonomy under this ADR; a Coalition is the next coordination layer up *of federations* without subsuming them. The semantic stacks cleanly: Cells aggregate into Cohorts (shared trait), Cohorts into Federations (autonomous alliance), Federations into Coalitions (combined-action alliance of alliances).
- **Tier-count extensibility is bounded, not open-ended.** The hierarchy commits to 4 named tiers above `Platform`. Beyond 4 tiers (US Army Brigade / Division / Corps scales, or any deployment exceeding ~10000 peers organized hierarchically) the rigid-tier approach breaks down: every new tier requires a wire-format change, a new protobuf message, a new `HierarchyLevel` variant, and new aggregation code paths. **ADR-067 (planned follow-up)** will design a parametric N-tier aggregation model that lifts this limit. Until ADR-067 lands, 4 tiers is the hard limit and deployments needing more should surface as input to that ADR's design.

### Field-name renames

| Old                                    | New                                       |
| -------------------------------------- | ----------------------------------------- |
| `squad_id`                             | `cell_id` (consolidates with existing field) |
| `platoon_id`                           | `cohort_id`                               |
| `company_id`                           | `federation_id`                           |
| (new — no military analogue used in code) | `coalition_id`                         |
| `SquadSummary`                         | `CellSummary`                             |
| `PlatoonSummary`                       | `CohortSummary`                           |
| `CompanySummary`                       | `FederationSummary`                       |
| (new)                                  | `CoalitionSummary`                        |
| `SquadDelta` / `PlatoonDelta` / `CompanyDelta` | `CellDelta` / `CohortDelta` / `FederationDelta` / `CoalitionDelta` |
| `update_squad_summary` / etc.          | `update_cell_summary` / `update_cohort_summary` / `update_federation_summary` / `update_coalition_summary` |
| `init_squad_summary` / etc.            | `init_cell_summary` / `init_cohort_summary` / `init_federation_summary` / `init_coalition_summary` |
| `HierarchyLevel::Squad` / `::Platoon` / `::Company` | `HierarchyLevel::Cell` / `::Cohort` / `::Federation` / `::Coalition` (new variant) |
| `peat-protocol/src/squad.rs` (module)  | Merge into `peat-protocol/src/cell/summary.rs` (decision finalized in Phase 2) |

### Document-key renames

Per ADR-021, summary documents are keyed `squad-{id}-summary`, `platoon-{id}-summary`, `company-{id}-summary`. These keys change too:

| Old                       | New                       |
| ------------------------- | ------------------------- |
| `squad-{id}-summary`      | `cell-{id}-summary`       |
| `platoon-{id}-summary`    | `cohort-{id}-summary`     |
| `company-{id}-summary`    | `federation-{id}-summary` |
| (new)                     | `coalition-{id}-summary`  |

This is **persistence-incompatible**. Existing stored docs under old keys will not be read by post-rename code. Migration strategy: the rename ships in a release-candidate cycle (rc.22+) with no on-disk migration. Stored state from earlier rc.* releases is discarded on upgrade; users replay state from peers. This is consistent with peat's current "rc.* releases reserve the right to break persistence" posture and avoids carrying a one-shot migration layer indefinitely.

### Wire-format strategy

The change to `peat-schema/proto/hierarchy.proto` (renaming the message types and field names) is **wire-incompatible**. Old and new peers cannot mutually decode each other's summary messages.

Of the three options the epic listed — (a) hard rename + major-version-bump, (b) deprecated aliases for one release cycle, (c) field-level renames within a renamed package — this ADR selects **(a) hard rename with a `peat-schema` major version bump**:

- **Scope.** Peat is pre-1.0; rc.* releases explicitly reserve the right to break wire format. There are no external consumers of `peat-schema` outside the peat ecosystem repos to coordinate with.
- **Deprecated-alias cost.** Option (b) doubles the schema surface for a release cycle, tempts stragglers to skip the migration, and leaves the old names visible in tooling output and generated docs for that cycle. The benefit (rolling upgrades across mixed-version peers) is not needed in a pre-1.0 ecosystem where rc.* upgrades coordinate the whole mesh anyway.
- **Cleanliness.** The user's revealed preference (per peat#898 mDNS consolidation, peat#704 hive/eche cleanup) is clean cuts over compatibility shims. This rename should match.

The `peat-schema` major version bump is the canonical signal of the break. Downstream crates (`peat-protocol`, `peat-mesh`, `peat-transport`) bump their floors to the new `peat-schema` version in the same release-candidate wave.

### CoT/TAK boundary translation

The CoT (Cursor on Target) wire protocol carries its own military vocabulary (squad/platoon/company appear as CoT type strings). The `peat-transport/src/tak/bridge/` layer is the boundary between external CoT and internal peat data structures. Per Phase 4 of the epic:

- The CoT-side wire vocabulary is **external** and **structurally load-bearing**: CoT type strings like `a-f-G-U-C-I` ("infantry / squad") are produced by external systems and Peat cannot rename them. Bridge code may legitimately read those strings.
- The peat-side internal types the bridge translates **into** must use the new abstract vocabulary. The bridge layer is where the rename happens at the boundary.
- This is the same shape as ADR-020's CoT integration rule: external protocol names appear in the bridge's translation logic; internal peat types use peat's own vocabulary.

### Boundaries — where military vocabulary IS allowed

The following carveouts mirror the existing `CLAUDE.md` consumer-name carveouts:

- **ADRs (`docs/adr/`) and the whitepaper** when citing a real-world use case that motivated a design decision. ADR-024's "Use Case 1: Dynamic Military Operations" example may keep the military framing because it's narrating a motivating scenario, not naming an implementation type. Existing ADRs (ADR-021, ADR-024, ADR-027) that reference military terms in their *implementation examples* are pre-existing debt; an amendment pass updates the code blocks once the rename lands.
- **`CHANGELOG.md` entries** that record the history of the rename itself. A release-notes file cannot usefully describe a rename without naming what was renamed.
- **Test fixtures only when documenting external interop.** A CoT XML test fixture that carries a real CoT type name (`a-f-G-U-C-I`) is documenting an external protocol; the test's own types must use the new vocabulary.
- **`peat-transport/src/tak/bridge/` CoT-side variables.** A local variable named `cot_squad_type` that holds an inbound CoT type string is naming an external protocol artifact, not an internal type. Bridge code may use such names for clarity at the boundary.

### Tier-count extensibility and the ADR-067 follow-up

This ADR commits to **four fixed, named tiers** above `Platform`:

```
Platform → Cell → Cohort → Federation → Coalition
```

Coalition (tier 4) is included now — not deferred to a later ADR — because a concrete deployment (US Army echelon scaling to ~1000+ peers) requires it, and shipping Phase 1 without Coalition would force a second wire-format break almost immediately.

**Hard limit**: 4 tiers is the maximum this rigid-schema design supports. Each tier requires its own protobuf message type, its own `HierarchyLevel` variant, and its own aggregation/storage/routing code paths. Adding a 5th tier (Brigade / Division / Corps scale, or any deployment exceeding ~10000 hierarchically-organized peers) under this model would require another schema break + another multi-PR rollout.

**Planned follow-up — ADR-067 (parametric N-tier aggregation).** When a 5th-tier deployment becomes concrete, ADR-067 will design a parametric model where:

- A single generic `AggregationSummary` proto message replaces the per-tier `*Summary` messages, carrying a `level: uint32` (or equivalent depth marker) plus a `parent_level_id` field.
- `Cell` / `Cohort` / `Federation` / `Coalition` become *labels* attached to specific depths in a deployment-side hierarchy configuration, not distinct types in the schema.
- Adding tier 5, 6, 7+ requires only configuration changes on the deployment side; no schema break.
- Storage, routing, and aggregation code becomes depth-parameterized rather than per-tier.

ADR-067 is *planned but not yet in flight*. Until it lands, treat 4 tiers as the hard limit, and surface any deployment needing more as a forcing input to that ADR's design. The choice to ship ADR-066 (rename + Coalition) without ADR-067 (parametric refactor) is deliberate: the rename's purpose is the *vocabulary* fix, and conflating it with a structural refactor would expand blast radius without a deployment driving the structural need *today*.

### Verification gate

After the rename lands, the workspace-wide grep gate becomes:

```bash
grep -rEln '\b(Squad|Platoon|Company|Battalion|Regiment|Brigade|Division|Fireteam)\w*' \
  --include='*.rs' --include='*.proto' \
  -- ':!docs/adr' ':!docs/whitepaper' ':!CHANGELOG.md' ':!CLAUDE.md' ':!SKILL.md' \
  -- ':!**/tests/fixtures/**' ':!peat-transport/src/tak/bridge/**'
```

Empty output is the acceptance criterion. CI adds this check after Phase 4 lands.

## Migration phases

Per peat#904's execution shape, the rename rolls out as five PRs across two repos:

| Phase | Repo            | PR scope                                                                 |
| ----- | --------------- | ------------------------------------------------------------------------ |
| 0     | `peat`          | **This ADR.** No code. Establishes vocabulary + tier-4 addition + wire-break strategy. |
| 1     | `peat`          | `peat-schema` rename of Squad/Platoon/Company → Cell/Cohort/Federation + **add new `CoalitionSummary` message + `coalition_id` field + `HierarchyLevel::Coalition` variant**. Major version bump. |
| 2     | `peat`          | `peat-protocol` consumer-side rename + **implement Coalition-tier aggregation** (`update_coalition_summary`, `init_coalition_summary`, routing/discovery integration). Bumps peat-schema floor. |
| 3     | `peat-mesh`     | `peat-mesh` rename + **extend hierarchy strategies / routing / beacon code to support the Coalition tier**. Independent of Phase 2. |
| 4     | `peat`          | `peat-transport/src/tak/bridge/` boundary translation; verification gate enabled in CI. |

Each phase is its own PR, per the ecosystem skill's "one PR per repo" rule. peat#904 is the tracking issue.

### Pre-existing bug fixes that keep the military names

[#902](https://github.com/defenseunicorns/peat/pull/902) and [#903](https://github.com/defenseunicorns/peat/issues/903) shipped history-preservation fixes for `update_squad_summary` / `update_platoon_summary` / `update_company_summary` / `update_command_status`. Those PRs keep the verbatim type names because the bug fixes had to land before this epic completed — grep needs to find the call sites until they are renamed. Phase 2 of this rename carries the renames of those exact functions; their fixes are preserved in the renamed equivalents.

## Consequences

### Positive

- One canonical vocabulary across the ecosystem, with no military framing in implementation code and no K8s-style directional collisions for enterprise-architecture readers.
- Consolidates the pre-existing `Squad` ↔ `Cell` redundancy in `peat-schema` and `peat-protocol`.
- Removes the unit-strength assumptions embedded in proto comments ("typically 8 nodes," "1 Squad Leader").
- *Federation* aligns with peat's actual coordination model (independently-managed cells coordinating without a central authority), so the name carries semantic information rather than just being a label.
- Aligns with the existing `CLAUDE.md` rule against consumer-specific references — same spirit, broader application.

### Negative

- Wire format breaks. Old peers cannot decode summaries produced by new peers and vice versa. Mitigated by the pre-1.0 posture and the rc.* release cycle.
- Persistence breaks. Stored summary docs from earlier rc.* releases are unreadable post-rename. Mitigated by replaying state from peers and by the existing pre-1.0 reservation against persistence stability.
- Phases 1–3 are no longer pure renames. They also add the Coalition tier (new proto message, new `HierarchyLevel` variant, new aggregation/routing code paths in `peat-protocol` and `peat-mesh`). Modest scope growth: the new code touches the same files as the rename, but exercise paths are new and need their own tests. Without this, Phase 1 would ship and immediately be followed by a second wire-format break to add Coalition for the 1000+-peer deployment driving the need.
- 4-tier hard limit. Tier 5+ (Brigade / Division / Corps scale, ~10000+ peers) is not supported under this ADR and requires ADR-067 (parametric N-tier aggregation, planned). Surface tier-5 deployment needs as forcing input to that ADR.
- Five PRs across two repos, plus this ADR. Coordination cost. Mitigated by the tracking issue (peat#904) and the per-phase blockedness.
- ADRs that use the old vocabulary in code examples (ADR-021, ADR-024, ADR-027) become inconsistent until an amendment pass updates them. This ADR explicitly defers those amendments to a follow-up doc-only PR after Phase 4 ships, so reviewers know not to fold them into the per-phase PRs.
- *Federation* is a 10-character word and shows up in many field names (`federation_id`, `FederationSummary`, `update_federation_summary`, etc.). Line lengths in the renamed code grow modestly. Acceptable; not a blocker.

### Neutral

- `peat-atak-plugin` (Android consumer) consumes `peat-ffi` bindings that may carry these type names. Phase 2 surfaces whether the rename ripples into the FFI surface; if it does, `peat-atak-plugin` gets a follow-up PR (a sixth phase, scoped to that repo). Flag in the Phase 2 PR description.

## Alternatives considered

### Organismal biology: Cell → Tissue → Organ

Original proposal in the first revision of this ADR. Rejected after review: although well-known and unambiguous, the names import a specific scientific domain wholesale. "This drone formation is a Tissue" reads strange in non-biological deployments (logistics, maritime, agricultural, urban infrastructure), and the explicit goal of this rename is to free the substrate from any single consumer's world-model. Biology is no more native to peat's anticipated deployments than soldiering.

### Biological-collective: Cell → Colony → Swarm

Considered as a broader-biological alternative. Each upper term spans many domains (microbes, ants, drones, satellites, AI agents). Rejected because *Swarm* has growing software-product overload (Docker Swarm being the loudest) and the vocabulary still leans biology-flavored where `Cohort`/`Federation` are fully domain-neutral.

### Topology-native: Node → Pod → Cluster → Fleet

Considered as the most software-native alternative. Rejected because the K8s ecosystem has strong directional claims on these terms:
- *Pod* lives *inside* a Node in K8s. Reusing it inverts the aggregation direction (peat's Pod would *contain* nodes) and would confuse enterprise architects coming from the K8s ecosystem.
- *Cluster* is the *whole installation* in K8s. Peat would have many clusters per deployment — cardinality flip.
- *Fleet* (Rancher Fleet, GKE Fleets) is "a group of clusters managed together." This actually aligns with peat's intent, but the spelling collision still adds confusion.

### Ecological hierarchy: Cell → Colony → Population → Community

Rejected. "Colony of cells" is metaphorically off — in biology, colonies are of organisms (ant colonies, bacterial colonies), not cells.

### Generic hierarchy: Cell → Cluster → Group

Rejected. `Cluster` and `Group` are too generic; they appear elsewhere in the codebase (e.g., capability clusters, peer groups) and would collide.

### Animal grouping: Pod → School → Aggregation

Rejected. Animal-grouping vocabulary is taxonomically inconsistent (a pod is whales, a school is fish, an aggregation is generic) and doesn't compose into a clear hierarchy.

### Keep the military terms, scope the rule to public APIs only

Rejected. The user's direction during peat#903 scoping was unambiguous about removing military references from implementation code. Internal-only military naming would still leak through stack traces, log messages, error variants, and source-level documentation.

## References

- [peat#904](https://github.com/defenseunicorns/peat/issues/904) — tracking epic
- [peat#903](https://github.com/defenseunicorns/peat/issues/903) — bug-fix PR where the user's direction surfaced
- `CLAUDE.md` § "Hard rule: no consumer-specific references in peat" — sibling rule, same spirit
- `SKILL.md` § "Hard invariants (cross-cutting)" — workspace ground rules
- ADR-021 — document-key naming this ADR breaks
- ADR-024 — `HierarchyLevel` enum this ADR renames
- ADR-027 — event-routing event names this ADR renames
