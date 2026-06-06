# ADR-068: Node as the Base-Unit Vocabulary

**Status**: Proposed
**Date**: 2026-06-06
**Authors**: Kit Plummer
**Amends**: ADR-066 (Abstract Hierarchy Vocabulary) — specifically the decision-table row `Platform (single node) → Platform (unchanged)`. This ADR completes ADR-066's abstraction by replacing the base-unit term `Platform` with `Node`.
**Related**: ADR-024 (Flexible Hierarchy Strategies — defines the `HierarchyLevel` enum whose base variant this ADR renames), ADR-002 (Beacon Storage — already keys on `node_id`).
**Triggered by**: the base mesh participant is named inconsistently across the ecosystem — `platform_id` in `peat-schema` and `peat-protocol`, but `node_id` in `peat-mesh` and `peat-node`. Two names for one concept.

---

## Context

ADR-066 made the *aggregation* vocabulary domain-neutral (`Squad/Platoon/Company → Cell/Cohort/Federation` + new `Coalition`), but deliberately left the **base unit** as `Platform`, reasoning it was "already abstract; no rename." In practice that leaves the ecosystem with a worse inconsistency than the one ADR-066 fixed:

1. **One concept, two names.** A single mesh participant is `platform_id` in `peat-schema` (`capability.proto`, `tasking.proto`, `track.proto`, `sensor.proto`, `effector.proto`, `actuator.proto`, `model.proto`) and in `peat-protocol` (`discovery/geographic.rs`, `discovery/coordinator.rs`, `models/operator.rs`), but `node_id` in `peat-mesh` (`beacon/types.rs`, `flat_mesh.rs`) and `peat-node` (`node.rs`, `main.rs`, `NodeListScope.node_ids`). `peat-node` even exposes a `Platform` gRPC message (with `PutPlatform`/`GetPlatforms`) that *is* a node. ADR-002 keys beacons on `node_id`. The substrate's own layers disagree on what to call its atom.

2. **`Platform` overloads two distinct ideas.** It names both (a) the *participant* (the thing with an id that joins the mesh, holds documents, fetches blobs) and (b) the *hardware/device class* (`platform_type`: UAV / UGV / soldier-system / sensor). These are different: a participant **runs on** a hardware platform. Using one word for both is the root of the confusion.

3. **`node` is already the de-facto term** at the layers that actually run the mesh (`peat-mesh`, `peat-node`, the `node_id` document keys in ADR-002). The published binary is literally `peat-node`. Converging on `Platform` would mean renaming the *more*-used, more-load-bearing term; converging on `Node` aligns with where the substrate's identity already is.

The fix: make **Node** the single canonical term for the base unit / mesh participant, and give the hardware-class concept its own unambiguous name. "Platform" is removed from the substrate vocabulary entirely.

## Decision

### Base-unit vocabulary

| Concept | Old | New |
| --- | --- | --- |
| The mesh participant (base unit) | `Platform` / `platform_id` | **`Node`** / **`node_id`** |
| The participant's hardware/device class | `platform_type` | **`node_type`** |
| Base tier of the aggregation hierarchy | `HierarchyLevel::Platform` | **`HierarchyLevel::Node`** |

The hierarchy becomes:

```
Node → Cell → Cohort → Federation → Coalition
```

`Cell / Cohort / Federation / Coalition` (ADR-066) are unchanged. Only the base unit changes: `Platform → Node`. "Platform" no longer appears as an identifier, type, message, field, or enum variant anywhere in the substrate — including `platform_type`, which becomes `node_type` (a node's device class is an attribute *of the node*, not a separate "platform" entity).

### Rename map (canonical)

| Old | New |
| --- | --- |
| `platform_id`, `platform_ids` | `node_id`, `node_ids` |
| `platform_type` | `node_type` |
| `target_platforms`, `target_platform_ids` | `target_nodes`, `target_node_ids` |
| `Platform` (proto message, e.g. `peat-node/sidecar.proto`) | `Node` |
| `PutPlatform` / `GetPlatforms` (+ request/response/status types) | `PutNode` / `GetNodes` (+ `Node*`) |
| `platform_count` (e.g. `Cell.platform_count`) | `node_count` |
| `HierarchyLevel::Platform` | `HierarchyLevel::Node` |
| `my_platform_id`, `tracked_platforms`, `platform_ids()` etc. (peat-protocol) | `my_node_id`, `tracked_nodes`, `node_ids()` |

Role/label prose such as "strike platform" in human-facing descriptions is **out of scope** — this ADR governs substrate identifiers, types, and wire fields, not free-text role names a consumer may display.

### Wire-format strategy

Breaking, same posture as ADR-066: proto field/message/enum renames change the wire format; old and new peers cannot interop. Mitigated by the pre-1.0 `rc.*` posture — ship as a coordinated major/rc bump of `peat-schema` (and the downstream pins move in lockstep, as the ADR-066 rename did). No compatibility shim; this is a clean break while we are pre-1.0.

### Verification gate

Acceptance criterion — an empty grep for the base-unit term as a code identifier (mirrors ADR-066's anti-military grep):

```bash
grep -rEln '\bplatform(_id|_ids|_type|_count|s)?\b|\bPlatform(State|Status|Summary)?\b|PutPlatform|GetPlatforms|HierarchyLevel::Platform' \
  --include='*.rs' --include='*.proto' \
  -- ':!docs/adr' ':!docs/whitepaper' ':!CHANGELOG.md' ':!CLAUDE.md' ':!SKILL.md' \
  -- ':!**/tests/fixtures/**'
```

Empty output is the gate. CI adds the check after the final phase lands. (The pattern is tightened during implementation so it doesn't match unrelated words; the intent is "no `platform`-as-participant/type identifier survives.")

## Migration phases

Mirrors the ADR-066 rollout — one PR per repo, schema first, linked through a tracking issue.

| Phase | Repo | Scope |
| --- | --- | --- |
| 1 | `peat` (`peat-schema`) | Rename `platform_id`→`node_id`, `platform_type`→`node_type`, `target_platform*`→`target_node*` across all `.proto`; regenerate. Major/rc bump. |
| 2 | `peat` (`peat-protocol`) | `discovery/geographic.rs`, `discovery/coordinator.rs`, `models/operator.rs`: `platform_id(s)`/`my_platform_id`/`tracked_platforms` → `node_*`. |
| 3 | `peat-mesh` | `HierarchyLevel::Platform → Node`; base-tier doc-comments/strings; any `platform_*` → `node_*`. |
| 4 | `peat-node` | `Platform` message + `PutPlatform`/`GetPlatforms` RPCs → `Node` / `PutNode` / `GetNodes`; `Cell.platform_count → node_count`. Bump `peat-schema`/`peat-protocol` pins. |
| 5 | consumers | `peat-atak-plugin`, `peat-sim`, `peat-gateway` (+ `peat-ffi` if the rename ripples into the FFI surface); amend ADR-024/066 examples in a doc-only pass; enable the CI grep gate. |

## Consequences

- **Wire format breaks.** Same as ADR-066; pre-1.0 posture absorbs it. Sequence this rename either bundled with or immediately after the ADR-066 rename to avoid a third break.
- **ADR-066's base-unit row is amended.** ADR-066 stands for the aggregation tiers (Cell/Cohort/Federation/Coalition) and the no-military-vocabulary principle; only its `Platform`-stays decision is reversed here. A doc-only pass updates ADR-066's table and any ADR examples that say "Platform"/"platform_id".
- **`node_type` replaces a genuine distinction's name, not the distinction.** The device class (UAV/UGV/etc.) still exists as data — it's now an attribute of the Node (`node_type`) rather than a separate "platform" noun. Reviewers should not read this as collapsing participant and hardware-class into one field; they remain distinct fields, both named under `node`.
- **`peat-node` is well-named.** The binary and the gRPC surface converge on the same term the rest of the stack uses; the `Platform` gRPC message was the main outlier and is removed.
- **FFI ripple risk.** As with ADR-066 Phase 2, Phase 4/5 surfaces whether `peat-ffi` / `peat-atak-plugin` carry `Platform`/`platform_id` in generated bindings; if so, that consumer gets its own PR. Flag in the Phase 4 PR.

## Alternatives considered

- **Keep `Platform` as the base unit (status quo / ADR-066).** Rejected: it's the less-used term, overloads participant-vs-hardware-class, and contradicts `peat-node`/`node_id`/ADR-002. Converging on Platform would mean renaming the more load-bearing term.
- **Keep `platform_type` (Node participant, platform = its hardware).** Considered and explicitly rejected per this ADR's decision: retaining the word "platform" anywhere keeps the overload alive and fails the clean-grep gate. The device class is expressed as `node_type`.
- **Defer to a post-1.0 compatibility-preserving rename.** Rejected: pre-1.0 is exactly when a clean break is cheap; deferring guarantees a harder migration later.

## References

- ADR-066 (Abstract Hierarchy Vocabulary) — the aggregation-tier rename this ADR completes.
- ADR-024 (Flexible Hierarchy Strategies) — `HierarchyLevel` definition.
- ADR-002 (Beacon Storage) — already uses `node_id`.
