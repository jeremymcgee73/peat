# ADR-069: Proto Package Namespace `peat.*`

**Status**: Proposed
**Date**: 2026-06-06
**Authors**: Kit Plummer
**Related**: ADR-066 (Abstract Hierarchy Vocabulary), ADR-068 (Node Base-Unit Vocabulary) — this ADR follows the same pre-1.0 clean-break precedent for a naming change, but on a different axis: the Protobuf package namespace rather than field/message vocabulary.
**Triggered by**: every `peat-schema` proto declares its package under `cap.*` (`cap.common.v1`, `cap.node.v1`, …) — a fossil of the project's original academic name, **C**apability **A**ggregation **P**rotocol. The project is now **Peat** (a name, not an acronym), and the gRPC sidecar already uses `peat.sidecar.v1`, so the schema's `cap.*` is the last holdout.

---

## Context

`peat-schema/proto/*.proto` (20 files) declare `package cap.<domain>.v1;`. This drives:

- The generated Rust module tree (`peat-schema/src/lib.rs`: a `pub mod cap { … }` wrapper with `include!(".../cap.<domain>.v1.rs")`, re-exported flat via `pub use cap::*;`).
- The fully-qualified Protobuf type names (`cap.node.v1.Node`, `cap.common.v1.Timestamp`).

`cap` no longer means anything to the project. It is mildly confusing (suggests a `cap`/Capability subsystem), inconsistent with the sidecar's `peat.sidecar.v1`, and not something to ship to early adopters as the canonical wire identity.

## Decision

Rename the Protobuf package namespace **`cap.<domain>.v1` → `peat.<domain>.v1`** across all schema protos. Flatten the generated module tree so the canonical Rust path is **`peat_schema::<domain>::v1::<Type>`** (dropping the now-redundant `cap` wrapper module). Pre-1.0 clean break — no aliasing of the old `cap.*` names.

## Breaking surface

Deliberately scoped and verified to be **low-risk**:

- **proto3 binary wire / persisted Automerge + postcard documents — unaffected.** proto3 does not encode the package name in serialized messages (only field numbers). No data migration; existing beacons/docs/blobs are byte-compatible.
- **gRPC method paths — unaffected.** The `cap.*` protos define messages only; there are no services, so no `/cap.*/…` method paths exist.
- **proto3 JSON — unaffected.** JSON keys are field names, not the package.
- **Rust consumer API — near-zero churn.** `lib.rs` already re-exported flat (`pub use cap::*;`), so consumers import `peat_schema::<domain>::v1`; flattening preserves that path. Only one `peat_schema::cap::…`-qualified reference existed and is updated.
- **Breaks: descriptor / reflection / fully-qualified names** (`cap.node.v1.Node` → `peat.node.v1.Node`) — affects buf, grpcurl reflection, and any registry keyed on FQN.
- **Latent: `payload_type_url` / `summary_type_url`** (`event.proto` string fields) — currently empty; only relevant if applications populate them with `cap.*` type-URLs.

## Consequences

- The schema's wire identity matches the project name and the existing `peat.sidecar.v1` service namespace; the CAP fossil is retired.
- Any out-of-tree consumer that hardcoded `cap.*` fully-qualified names or proto-descriptor lookups must update — flagged as the only real break, consistent with the pre-1.0 clean-break stance of ADR-066/068.
- The `spec/proto/cap/v1/` reference tree and `draft-peat-protocol-*.md` are updated to `peat.*` (may ride the spec-doc follow-up).
