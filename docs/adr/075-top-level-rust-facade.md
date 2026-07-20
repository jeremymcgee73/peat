# ADR-075: Top-Level Rust Facade and FFI Distribution Boundary

**Status**: Proposed
**Date**: 2026-07-20
**Authors**: Kit Plummer
**Related**: ADR-049 (peat-mesh Extraction), ADR-060 (Encryption Tiers at Rest and in Transit), ADR-062 (Iroh Transport Consolidation), ADR-074 (peat-schema Single Source of Truth)
**Tracking**: [peat#1036](https://github.com/defenseunicorns/peat/issues/1036)

---

## Context

The `peat` crate is published to crates.io with every workspace release, but it is an empty reserved-name placeholder. Rust consumers currently depend on `peat-protocol`, which describes itself as the public entry point and re-exports `peat-schema` and `peat-mesh`. Other capabilities are spread across independently useful crates:

- `peat-schema` owns generated wire types.
- `peat-protocol` owns coordination and protocol semantics.
- `peat-mesh` owns mesh networking, discovery, routing, and synchronized storage.
- `peat-btle` owns Bluetooth Low Energy transport and platform adapters.
- `peat-lite` owns the lightweight protocol and CRDT primitives for constrained environments.
- `peat-ffi` owns UniFFI, JNI, native-library, and generated-language binding surfaces.

This makes the canonical Rust dependency unclear. The most obvious crate name delivers no functionality, while the crate currently acting as the facade has a component-specific name.

Repository guidance also assigns two incompatible future roles to `peat`:

1. a top-level facade that depends on and re-exports component crates; and
2. a foundational dependency anchor that component crates depend on for shared types and traits.

A crate cannot hold both positions without creating a dependency cycle. The ecosystem needs one explicit role for `peat`, a feature policy that does not force every transport into every build, and a firm boundary between the Rust facade and platform-specific FFI artifacts.

## Decision

### 1. `peat` is the canonical Rust facade

`peat` will be a thin, safe Rust facade and compatibility bill of materials for the supported Peat stack. It will contain no protocol, transport, storage, platform, or FFI implementation logic.

The dependency direction is:

```text
Rust consumer
    |
    v
  peat  (facade; no component may depend on it)
    |
    +--> peat-schema
    +--> peat-protocol --> peat-schema + peat-mesh
    +--> peat-mesh -----> optional transport integrations
    +--> peat-btle       (optional, after the FIPS gate below)
    `--> peat-lite       (optional host-side bridge surface)
```

Component crates MUST NOT depend on `peat`. Doing so would reverse the facade edge and create a cycle.

If a concrete need for shared foundational Rust types or traits emerges, it must be addressed below the components in a separately decided workspace crate such as `peat-core`. This ADR does not create that crate. Types already governed by ADR-074 remain in `peat-schema`, and protocol contracts remain in `peat-protocol` until a demonstrated cross-component dependency requires extraction.

### 2. The public surface is namespaced

The initial facade will re-export component crates under stable, descriptive namespaces:

```rust
pub use peat_mesh as mesh;
pub use peat_protocol as protocol;
pub use peat_schema as schema;

#[cfg(feature = "bluetooth")]
pub use peat_btle as btle;

#[cfg(feature = "lite-transport")]
pub use peat_lite as lite;
```

Consumers will use paths such as `peat::schema`, `peat::protocol`, and `peat::mesh`.

The facade will not glob-re-export component items into the `peat` root. Root-level flattening would create naming collisions, make component ownership unclear, and turn every component API change into a facade API change. Any future curated prelude or root-level type requires a separate public-API decision.

`peat-protocol` may retain its existing `peat_schema` and `peat_mesh` re-exports for compatibility. New Rust integration documentation will recommend `peat`; specialized consumers may continue to depend directly on component crates.

### 3. Features select capabilities, not architectural layers

The facade will mirror established component feature names:

| Feature | Default | Effect |
|---|---:|---|
| `automerge-backend` | yes | Enables the standard Automerge/Iroh synchronized-storage backend through `peat-protocol`. |
| `lite-transport` | no | Enables the host-side Peat Lite bridge and the `peat::lite` namespace. |
| `bluetooth` | no | Enables Bluetooth integration and the `peat::btle` namespace after the FIPS gate below is satisfied. |
| `relay-n0-hosted` | no | Explicitly opts into the hosted relay and discovery behavior already governed by the corresponding component feature. |

`peat-schema`, base `peat-protocol`, and base `peat-mesh` remain available without separate facade features. Layer-named features such as `schema`, `protocol`, or `mesh` would make the documented namespace vary across arbitrary feature combinations without producing a meaningfully smaller facade contract.

There will be no `full` feature. In particular, hosted relay use must remain an explicit operator choice and must never be activated by a convenience bundle.

The `peat` facade targets full Rust environments. Constrained or `no_std` consumers should depend directly on `peat-lite` or the relevant transport crate rather than pulling the facade and its Tokio-based protocol stack.

### 4. `peat-ffi` remains separate

`peat-ffi` will not be a dependency, re-export, default feature, or optional feature of `peat`.

The two crates deliver different products:

- `peat` delivers a Rust `rlib` facade.
- `peat-ffi` delivers UniFFI and JNI surfaces, `staticlib`/`cdylib` artifacts, generated Kotlin and Swift bindings, and platform-specific packages.

Re-exporting `peat-ffi` from `peat` would expose Rust wrapper types but would not build or distribute the native libraries or generated bindings that FFI consumers need. It would also couple the facade release to `peat-ffi`'s independently versioned ABI and platform verification matrix.

Rust consumers that are themselves building an FFI wrapper may depend directly on `peat-ffi`. Foreign-language consumers must use the platform artifact intended for their toolchain. All protocol and transport behavior remains implemented in Rust below the FFI boundary; this separation does not move logic into host languages.

### 5. The facade is curated

`peat-transport` and `peat-persistence` are not part of the initial facade. They remain internal or directly consumed crates until their public contracts are intentionally stabilized. Workspace membership alone does not qualify a crate for facade exposure.

Adding another component to `peat` requires evidence that it is a supported Rust SDK surface, a compatible version constraint, feature-matrix coverage, and an explicit public-API review.

### 6. `peat` is the tested compatibility set

A `peat` release asserts that its selected component versions resolve and work together. Workspace-published components use versioned path dependencies suitable for crates.io publication. Independently released pre-1.0 components use the workspace's narrow compatibility ranges; the facade must not widen those ranges beyond the combinations exercised by CI.

The crates.io publish order is dependency order: component versions must be indexed before `peat` is published. Because `peat-ffi` is not a facade dependency, its independent release does not gate a `peat` release.

## Bluetooth FIPS Gate

The currently selected `peat-btle` release directly declares ChaCha20-Poly1305, X25519, and BLAKE3 dependencies. These conflict with ADR-060 and the ecosystem's mandatory FIPS posture.

The `bluetooth` facade feature described above is the intended post-migration contract, but it MUST NOT be implemented or advertised until a compliant `peat-btle` release is available and selected by the workspace. The migration requires, at minimum:

- an approved AES-GCM construction in place of ChaCha20-Poly1305;
- ECDH on P-256 or P-384 in place of X25519; and
- an approved SHA-2 hash wherever BLAKE3 is used as a primary cryptographic hash.

That work is cross-repository and requires separate linked PRs: first `peat-btle`, then any `peat-mesh` compatibility update, then the workspace pin and facade feature in `peat`.

The facade may ship before that sequence completes only if the `bluetooth` dependency, feature, and namespace are omitted from the release.

## Consequences

### Positive

- Rust users get one canonical dependency and documentation entry point.
- Component ownership remains visible through namespaced paths.
- The facade acts as a tested compatibility set without merging independently useful crates.
- Minimal, embedded, and platform-specific consumers retain direct component dependencies.
- FFI ABI changes and platform artifact releases remain independent of Rust facade releases.
- Dependency direction is unambiguous: components never depend on the facade.
- Hosted relay behavior, Bluetooth, and lightweight transport remain explicit opt-ins.

### Negative

- The facade adds another public surface whose feature forwarding and version constraints must be maintained.
- Existing documentation that calls `peat-protocol` the public entry point must be updated.
- Consumers may encounter both `peat::protocol::...` and direct `peat_protocol::...` paths in the ecosystem.
- A future foundational shared-types crate would require a separate extraction rather than placing those types directly in `peat`.
- The complete requested facade cannot advertise Bluetooth until the FIPS migration is released.

### Operational and release impact

- CI must compile the default facade, no-default facade, every individual optional feature, and supported feature combinations.
- Packaging verification must consume the generated crate package rather than relying only on workspace path dependencies.
- Release documentation must reflect the actual crates.io and platform-artifact channels and their ordering.
- `peat` must remain free of unsafe code and consumer-specific identifiers.

## Alternatives Considered

### Keep `peat` empty and use `peat-protocol` as the facade

Rejected. It preserves a misleading canonical crate name and leaves the published `peat` package without value. It also makes a protocol component responsible for presenting unrelated transport namespaces.

### Make `peat` the foundational core crate

Rejected for the top-level name. It would prevent `peat` from serving as the requested facade unless a second umbrella crate were introduced. A narrowly scoped `peat-core` remains available as a future option if actual shared dependency pressure appears.

### Include `peat-ffi` behind a default-off feature

Rejected. A Cargo re-export does not deliver FFI artifacts, while the dependency would couple the Rust facade to an independently versioned ABI and substantially broader platform matrix.

### Enable every component by default

Rejected. Bluetooth and lightweight transports are environment-specific, hosted relay use requires explicit authorization, and constrained consumers need direct minimal dependencies rather than the full Tokio-based stack.

## Compliance

Implementation PRs for this ADR must demonstrate:

1. no dependency edge from a component crate back to `peat`;
2. namespaced re-exports without root-level glob flattening;
3. default, no-default, and per-feature compile checks;
4. a packaged-crate consumer smoke test for every advertised namespace;
5. crates.io dry-run packaging with all dependency versions available;
6. the ecosystem consumer-identifier diff gate; and
7. no `bluetooth` facade surface until the FIPS gate in this ADR is satisfied.
