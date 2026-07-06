# ADR-074: peat-schema as Single Source of Truth for All Message Types

**Status**: Proposed
**Date**: 2026-07-06
**Authors**: Parker Hornstein
**Related**: ADR-069 (Proto Package Namespace), ADR-049 (peat-mesh Extraction)

---

## Context

The peat ecosystem defines message schemas in two places today:

1. **peat-schema** — 20 protobuf definitions under `peat-schema/proto/` (node, track, cell, command, marker, capability, etc.), code-generated at build time into versioned Rust modules (`peat_schema::node::v1::NodeConfig`, `peat_schema::track::v1::Track`, etc.).

2. **Hand-written structs in sub-crates** — most notably the five `uniffi::Record` types in `peat-ffi/src/lib.rs` (`NodeInfo`, `TrackInfo`, `CellInfo`, `MarkerInfo`, `CommandInfo`) plus their supporting enums (`NodeStatus`, `CellStatus`, `TrackCategory`, `CommandStatus`). These are manually maintained projections of the schema types, with field sets that have drifted from the proto definitions over time.

This creates several problems:

- **Drift.** When a field is added, renamed, or re-typed in a proto, the hand-written FFI struct must be updated independently. There is no compile-time enforcement that the two stay in sync. Fields have already diverged (e.g. `NodeInfo.last_heartbeat` has no proto counterpart; `track.v1.Track` has fields that `TrackInfo` omits).

- **Semantic ambiguity.** Two types with overlapping but non-identical field sets for the same domain concept (e.g. `peat_schema::node::v1::NodeState` vs. `peat_ffi::NodeInfo`) force contributors to ask "which is canonical?" The answer varies by call site, which is the wrong state.

- **Maintenance cost.** Every new document type requires defining the proto, generating the Rust code, *and* hand-writing a parallel FFI struct with serialization/deserialization logic. The JSON codec in `peat-ffi` (`parse_node_json`, `serialize_node_json`, etc.) is ~600 lines of manual field-by-field mapping that would be unnecessary if the types flowed from the schema.

- **Cross-repo inconsistency.** Sibling repos that consume peat messages (consumer plugins, bridges) cannot pin to a single schema version — they must track both the proto definitions and the FFI projections, which may be at different commit points.

## Decision

**peat-schema is the single source of truth for every message type broadcast into the mesh.** No sub-crate or sibling repo may define its own parallel message struct for a domain concept that peat-schema covers.

### Rules

1. **All message types originate in peat-schema protos.** Adding a new document type means adding or extending a `.proto` file in `peat-schema/proto/`, not writing a struct in a consumer crate.

2. **Sub-crates derive from peat-schema at build time.** Crates that need message types (peat-ffi, peat-mesh, consumer plugins) must depend on `peat-schema` and use the generated types directly, or derive thin wrapper types via procedural macro / `From` impl from the generated types — never by hand-copying fields.

3. **FFI projection types are derived, not authored.** Where a sub-crate needs a simplified projection for an FFI boundary (e.g. `uniffi::Record` for Kotlin/Swift), that projection must be mechanically derived from the peat-schema type. The derivation — which fields to include, how to map proto enums to FFI enums — lives in a single, auditable conversion layer (`From<proto::Type> for FfiType`), not scattered across manual parse/serialize functions.

4. **Wire-format compatibility is a schema concern.** JSON wire keys, field defaults, and backward-compatible evolution (field additions, optional→required transitions) are governed by the proto schema's evolution rules, not by ad-hoc logic in consumer codecs.

5. **Version pinning.** Sibling repos pin to a specific peat-schema version (git ref or crate version). Upgrading the pin is an explicit, reviewable action — not an implicit side effect of pulling a different branch.

### What this replaces

The five hand-written FFI structs in `peat-ffi/src/lib.rs` (`NodeInfo`, `TrackInfo`, `CellInfo`, `MarkerInfo`, `CommandInfo`) and their manual JSON codecs will be replaced by schema-derived types with `From` impls from the corresponding peat-schema protos. The `put_document` / `publish_document` generic JSON path is unaffected — it is explicitly schema-free and remains the extensibility escape hatch.

## Consequences

### Positive

- **Single point of truth.** Field additions, renames, and deprecations happen in one place and propagate to all consumers at build time.
- **Compile-time drift detection.** If a proto field is removed or re-typed, any `From` impl that references it fails to compile — no silent runtime divergence.
- **Reduced maintenance surface.** The ~600 lines of manual JSON parse/serialize logic in peat-ffi collapse to derived `From` impls and serde on the proto types.
- **Consistent cross-repo schemas.** Every repo pins to a peat-schema version; upgrading that pin is the only way new fields appear downstream.

### Negative

- **Migration work.** The existing hand-written structs and their tests must be migrated. This is a one-time cost but touches peat-ffi's public API surface.
- **Proto constraints on FFI.** uniffi has requirements (no nested oneofs, limited generic support) that may require the `From` conversion layer to flatten or simplify proto types. This is acceptable — the conversion is explicit and auditable — but adds a thin layer that pure proto-passthrough would not need.
- **Build-time dependency.** Sub-crates now require protoc (or prost-build) in their build chain. Crates that previously had no build script will gain one.

## Compliance

Any PR that introduces a hand-written struct for a domain concept already covered by (or appropriate for) a peat-schema proto must be rejected in review. The reviewer should direct the author to either extend the relevant proto or, if the concept is genuinely new, add a new proto definition to peat-schema first.
