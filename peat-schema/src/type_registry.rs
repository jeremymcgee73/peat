//! Runtime type metadata registry (peat#946).
//!
//! Consumer-facing surface that lets downstream clients identify and
//! validate Peat documents against their known schema types at runtime
//! — the missing link that has been blocking type-aware features across
//! the workspace (typed renderers, schema-validated writes, runtime
//! type-introspection tooling).
//!
//! # Usage
//!
//! ```
//! use peat_schema::type_registry::{BuiltinRegistry, TypeRegistry};
//!
//! let registry = BuiltinRegistry::with_peat_schema_types();
//!
//! // Look up by collection convention (renderer dispatch path).
//! let desc = registry.for_collection("capabilities").expect("known");
//! assert_eq!(desc.id.as_str(), "peat.capability.v1.Capability");
//!
//! // Validate a proposed JSON document against the type
//! // (write-validation path).
//! let proposed = serde_json::json!({
//!     "id": "cap-1",
//!     "name": "sensor",
//!     "confidence": 0.95,
//! });
//! // The full Capability shape has more required fields; this would error.
//! let _ = (desc.validate_json)(&proposed);
//! ```
//!
//! # Scope (v1)
//!
//! - Core types from `peat_schema::validation::core` are wired in:
//!   `Capability`, `NodeConfig`, `NodeState`, `CellConfig`, `CellState`.
//! - Per-collection lookup uses conventional collection names (e.g.
//!   "capabilities" for `Capability`). Consumers operating under a
//!   different convention can wrap the builtin registry with their own
//!   `register(…)` calls to add or override mappings.
//! - Type inference from document content alone (no `_type` field,
//!   no collection hint) is **not** in v1 — `for_collection(…)` and
//!   `get(&TypeId)` are the v1 lookup surfaces. A content-based
//!   `type_of(&doc)` can be layered additively once we settle on a
//!   `_type` marker convention or structural-match heuristic.
//! - Additional types (tasking, sensor, actuator, effector, product,
//!   track, model — each with existing validators) land in follow-up
//!   commits.

use crate::validation::{ValidationError, ValidationResult};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Canonical type identifier — proto-qualified name with version namespace,
/// e.g. `peat.capability.v1.Capability`. The namespace prefix is `peat.`
/// rather than the protobuf wire prefix `cap.` to communicate that this
/// is the consumer-facing identifier surface, not the protobuf-internal one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TypeId(String);

impl TypeId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TypeId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Validator dispatch function: takes a JSON value (the document content
/// as JSON), deserialises it into the typed protobuf message, then runs
/// the existing typed validator. Returns `Err` if either the deserialise
/// or the validation fails.
pub type JsonValidatorFn = fn(&Value) -> ValidationResult<()>;

/// Common prefix used by every `validate_json` closure in
/// [`mod@descriptors`] when wrapping a strict prost-derived
/// `Deserialize` failure into [`ValidationError::InvalidValue`].
///
/// Centralising the prefix as a `const` (rather than duplicating the
/// literal at each error site) means the
/// `proto3_zero_deserialises_cleanly_for_every_registered_type`
/// regression test can match on the same identifier the producers use.
/// If the prefix is ever renamed and any of the six sites (five
/// validators in this module + the test guard) fails to update in
/// lockstep, the change surfaces as a compile error — not as silent
/// coverage degradation where the test's `Err(_other)` arm starts
/// swallowing a deserialise failure that should have been flagged.
pub(crate) const DESERIALISE_ERROR_PREFIX: &str = "could not deserialise as ";

/// Proto3 wire-zero JSON dispatch function (peat#953).
///
/// Returns the canonical proto3 zero `serde_json::Value` for a registered
/// type — the JSON form of `Type::default()` for that type's prost-
/// generated struct. Every field is populated with its proto3 zero
/// (`string → ""`, numeric `→ 0` / `0.0`, `bool → false`, `enum →
/// first variant, `repeated → []`, `optional message → null`, etc.),
/// matching the proto3 default semantics exactly.
///
/// **Round-trip property by construction.** The result is `serde_json::to_value(<T>::default())`
/// where `<T>` is a prost-generated message implementing `serde::Serialize` +
/// `Default`. Calling `<T>::deserialize` on this value succeeds by
/// construction — the JSON is the wire-serialised form of an instance
/// that prost already produced.
///
/// Consumers driving JSON-based defaulting (e.g. `peat-cli`'s
/// `apply_proto3_defaults` for partial `--set` payloads) merge user-
/// supplied fields on top of this zero object, then deserialise the
/// result through the type's strict prost-derived `Deserialize` impl.
/// The descriptor-driven path eliminates the per-collection hardcoded
/// table that consumers previously had to maintain.
///
/// The signature matches [`JsonValidatorFn`]'s `fn` pointer shape so
/// the descriptor can store it in a `Copy + 'static` slot alongside
/// the other dispatch functions.
pub type Proto3ZeroFn = fn() -> Value;

/// Default `Proto3ZeroFn` used by [`TypeDescriptor::new`] for descriptors
/// constructed without an explicit proto3-zero supplier (peat#953).
///
/// Returns an empty JSON object (`{}`). Consumers that hit this fallback
/// from a descriptor built via the public `TypeDescriptor::new` API see
/// "no per-field defaults" rather than "type-shaped defaults," and can
/// either register the descriptor through the `descriptors::*` helpers
/// in this module (which set the function explicitly) or supply their
/// own via the public field. Returning `{}` (rather than `Value::Null`)
/// keeps the merge-on-top-of-defaults pattern in `apply_proto3_defaults`
/// safe — merging a user object onto `{}` always yields the user object.
pub fn default_proto3_zero() -> Value {
    Value::Object(serde_json::Map::new())
}

/// How a single field is intended to be rendered. Downstream renderers
/// (CLI typed-output paths, operator UIs, introspection tools) dispatch
/// off this to pick a display strategy. The renderer owns the actual
/// formatting; the descriptor just carries the hint.
///
/// `FieldFormat::Text` is the safe default — a renderer that doesn't
/// recognise a more specific variant can always fall back to text.
///
/// `#[non_exhaustive]`: additional format hints can be added in follow-on
/// commits without breaking external consumers that `match` on this enum
/// (they need a wildcard arm).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldFormat {
    /// Plain string. Renderer displays the value verbatim.
    Text,

    /// Numeric value with an optional unit suffix (e.g. `"m"`, `"m/s"`,
    /// `"min"`). Renderer typically displays `"<n> <unit>"`.
    Number { unit: Option<&'static str> },

    /// Floating-point value in [0.0, 1.0] rendered as a percentage
    /// (e.g. `0.95` → `"95.0%"`).
    Percentage,

    /// Boolean — true/false.
    Boolean,

    /// `common.v1.Timestamp` — renderer typically emits RFC 3339.
    Timestamp,

    /// `common.v1.Position` — renderer typically emits
    /// `"<lat>°N, <lon>°W, <alt>m"`.
    Position,

    /// Enum (proto3 integer at the wire level). Variants are the
    /// canonical proto3 names in declaration order; index by integer
    /// value to look up the label. Renderer typically displays the
    /// label.
    Enum { variants: &'static [&'static str] },

    /// Nested protobuf message. Renderer recurses via the registry to
    /// the referenced type's `TypeDescriptor`. `nested_type_id` is the
    /// canonical id of the sub-message.
    Nested { nested_type_id: TypeId },

    /// Repeated field. `item_format` describes how each element
    /// renders. Renderer typically emits a list / table.
    List { item_format: Box<FieldFormat> },

    /// JSON-encoded string field (e.g. proto3 `metadata_json`).
    /// Renderer typically pretty-prints the embedded JSON.
    JsonString,

    /// BlobRef metadata — renderer emits `"<blob:<size> sha256:<hash>>"`
    /// without dereferencing the blob contents.
    BlobRef,
}

/// Descriptor for one field of a typed document. Carries enough metadata
/// for a renderer to display the field with its proper label, in proper
/// order, with a format hint dispatched off `FieldFormat`.
///
/// `#[non_exhaustive]` so additional fields (e.g. nullability hints,
/// deprecation markers) can land additively without breaking external
/// consumers that struct-literal-construct.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FieldDescriptor {
    /// The proto3 field name (snake_case) — matches the JSON key used
    /// when the message round-trips through serde.
    pub name: &'static str,

    /// Human-readable display label — typically TitleCase or
    /// space-separated for multi-word names.
    pub label: &'static str,

    /// Rendering hint.
    pub format: FieldFormat,
}

impl FieldDescriptor {
    /// Construct a `FieldDescriptor`. Use this from external crates that
    /// register their own types — direct struct-literal construction is
    /// reserved for `peat-schema` itself so future field additions stay
    /// non-breaking.
    pub fn new(name: &'static str, label: &'static str, format: FieldFormat) -> Self {
        Self {
            name,
            label,
            format,
        }
    }
}

/// Descriptor for one known type. Registries hand back references to
/// these so consumers can use them as a stable handle through their
/// own data flow.
///
/// `#[non_exhaustive]`: additional fields (renderer hints, deprecation
/// markers, etc.) can be added in follow-on commits without breaking
/// external consumers. Construct externally via [`TypeDescriptor::new`]
/// plus direct field assignment for the optional fields.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TypeDescriptor {
    /// Canonical, stable identifier.
    pub id: TypeId,

    /// Human-readable display name (without the proto path).
    pub name: String,

    /// Version label (e.g. `"v1"`). Convention; not parsed.
    pub version: String,

    /// Conventional collection where documents of this type live.
    /// `None` for types that aren't associated with a single collection
    /// by convention. Consumers MAY override this at the registry-build
    /// layer for deployment-specific collection layouts.
    pub canonical_collection: Option<String>,

    /// Validator: JSON value → typed message → field-level validate.
    pub validate_json: JsonValidatorFn,

    /// Proto3 wire-zero JSON supplier (peat#953). Returns the canonical
    /// proto3 zero JSON for this type — every field populated with its
    /// proto3 default (string → "", numeric → 0/0.0, bool → false,
    /// enum → first variant, repeated → [], optional message → null).
    ///
    /// Driven by the prost-generated `Default` impl on the underlying
    /// message struct, so the value cannot drift from the proto3 field
    /// list — consumers always get the current canonical zero shape as
    /// the schema evolves.
    ///
    /// Defaulted to [`default_proto3_zero`] (returns `{}`) by
    /// [`Self::new`] so external callers that haven't wired in a
    /// proto3-aware zero supplier fall back to a safe empty-object
    /// shape; descriptors registered by
    /// [`BuiltinRegistry::with_peat_schema_types`] are built by the
    /// crate's private `descriptors` module helpers, which set this
    /// field explicitly via the prost-generated `Default` of each
    /// type. (Plain prose rather than an intra-doc link on the
    /// helpers — the `descriptors` module is private, so a
    /// `[\`mod@descriptors\`]` link would fail
    /// `rustdoc::private-intra-doc-links` under `cargo doc -- -D warnings`.)
    pub proto3_zero_fn: Proto3ZeroFn,

    /// Fields in canonical display order, with rendering hints.
    /// Renderer-side downstream consumers iterate this list to produce
    /// typed output. Empty for types that don't yet have field
    /// metadata authored.
    pub fields: Vec<FieldDescriptor>,
}

impl TypeDescriptor {
    /// Construct a `TypeDescriptor` with the required fields. Optional
    /// fields (`canonical_collection`, `proto3_zero_fn`, `fields`)
    /// default to empty / no-op; callers set them via direct field
    /// assignment after construction.
    ///
    /// Use this from external crates that register their own types —
    /// direct struct-literal construction is reserved for `peat-schema`
    /// itself so future field additions stay non-breaking.
    pub fn new(
        id: TypeId,
        name: impl Into<String>,
        version: impl Into<String>,
        validate_json: JsonValidatorFn,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            version: version.into(),
            canonical_collection: None,
            validate_json,
            proto3_zero_fn: default_proto3_zero,
            fields: Vec::new(),
        }
    }

    /// Return the canonical proto3 wire-zero JSON for this type
    /// (peat#953).
    ///
    /// Every field of the proto3 message is populated with its zero
    /// value, exactly as the prost-generated `Default::default()` for
    /// the underlying struct would produce after `serde_json::to_value`.
    /// Consumers that need to fill in a partial user payload before
    /// strict prost-derived `Deserialize` (e.g. `peat-cli`'s
    /// `apply_proto3_defaults` for partial `--set` payloads, or any
    /// SDK that needs to construct a baseline document programmatically)
    /// merge user-supplied fields on top of this value.
    ///
    /// Round-trips through this type's `Deserialize` impl by
    /// construction: the value is `serde_json::to_value(<T>::default())`
    /// where `<T>` already implements both `Default` (prost) and
    /// `Serialize` (peat-schema build script), so the deserialise of
    /// the wire-form of a valid in-memory instance is always a valid
    /// in-memory instance.
    ///
    /// # Example
    ///
    /// ```
    /// use peat_schema::type_registry::{BuiltinRegistry, TypeRegistry};
    ///
    /// let registry = BuiltinRegistry::with_peat_schema_types();
    /// let desc = registry.for_collection("capabilities").unwrap();
    /// let zero = desc.proto3_zero();
    /// // Every field of `Capability` is present at its proto3 zero.
    /// assert!(zero.is_object());
    /// ```
    pub fn proto3_zero(&self) -> Value {
        (self.proto3_zero_fn)()
    }
}

/// Trait for type registries. Consumers can build their own, or use the
/// builtin one for `peat-schema`'s known types.
///
/// `Send + Sync` so the registry can be shared across threads (e.g.
/// embedded inside a long-lived consumer process).
pub trait TypeRegistry: Send + Sync {
    /// Look up by canonical identifier.
    fn get(&self, id: &TypeId) -> Option<&TypeDescriptor>;

    /// Look up by conventional collection name (the convention each
    /// `TypeDescriptor` carries in its `canonical_collection` field).
    fn for_collection(&self, collection: &str) -> Option<&TypeDescriptor>;

    /// Iterate over all registered descriptors.
    fn iter(&self) -> Box<dyn Iterator<Item = &TypeDescriptor> + '_>;
}

/// In-memory registry. Construct with `BuiltinRegistry::with_peat_schema_types()`
/// for the default set, then optionally extend with `register(…)`.
#[derive(Debug, Default, Clone)]
pub struct BuiltinRegistry {
    by_id: HashMap<TypeId, Arc<TypeDescriptor>>,
    by_collection: HashMap<String, Arc<TypeDescriptor>>,
}

impl BuiltinRegistry {
    /// Empty registry. Add types via [`register`](Self::register).
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a type descriptor. If the descriptor carries a
    /// `canonical_collection`, the collection→type mapping is also
    /// recorded. Later inserts with the same canonical collection
    /// override earlier ones (last-write-wins).
    pub fn register(&mut self, desc: TypeDescriptor) -> &mut Self {
        let arc = Arc::new(desc);
        if let Some(coll) = arc.canonical_collection.as_ref() {
            self.by_collection.insert(coll.clone(), Arc::clone(&arc));
        }
        self.by_id.insert(arc.id.clone(), arc);
        self
    }

    /// Registry pre-populated with the peat-schema types that have
    /// validators in [`crate::validation`].
    ///
    /// The pre-populated set is intentionally narrow in v1 (the core
    /// five). Additional types (tasking, sensor, actuator, effector,
    /// product, track, model) land in follow-on commits.
    pub fn with_peat_schema_types() -> Self {
        let mut r = Self::new();
        r.register(descriptors::capability());
        r.register(descriptors::node_config());
        r.register(descriptors::node_state());
        r.register(descriptors::cell_config());
        r.register(descriptors::cell_state());
        r
    }
}

impl TypeRegistry for BuiltinRegistry {
    fn get(&self, id: &TypeId) -> Option<&TypeDescriptor> {
        self.by_id.get(id).map(|a| a.as_ref())
    }

    fn for_collection(&self, collection: &str) -> Option<&TypeDescriptor> {
        self.by_collection.get(collection).map(|a| a.as_ref())
    }

    fn iter(&self) -> Box<dyn Iterator<Item = &TypeDescriptor> + '_> {
        Box::new(self.by_id.values().map(|a| a.as_ref()))
    }
}

/// Individual type descriptors. Each one carries an adapter that
/// takes JSON, deserialises into the typed message, runs the validator.
///
/// Implementation note: the adapter is a free `fn`, not a closure, so
/// `TypeDescriptor` stays `Copy`-friendly at the function-pointer level
/// without `Box<dyn Fn>` overhead.
mod descriptors {
    use super::*;

    /// `peat.capability.v1.Capability`.
    pub fn capability() -> TypeDescriptor {
        fn validate(value: &Value) -> ValidationResult<()> {
            let msg = crate::capability::v1::Capability::deserialize(value).map_err(|e| {
                ValidationError::InvalidValue(format!("{DESERIALISE_ERROR_PREFIX}Capability: {e}"))
            })?;
            crate::validation::validate_capability(&msg)
        }
        // Proto3 CapabilityType variants, in declaration order (index = wire integer).
        const CAPABILITY_TYPE_VARIANTS: &[&str] = &[
            "Unspecified",
            "Sensor",
            "Compute",
            "Communication",
            "Mobility",
            "Payload",
            "Emergent",
        ];
        fn proto3_zero() -> Value {
            // peat#953: prost-generated `Default` + serde::Serialize
            // (both wired by build.rs) produce the canonical proto3
            // wire-zero JSON for this message type.
            serde_json::to_value(crate::capability::v1::Capability::default())
                .expect("proto3 message serialises to JSON cleanly")
        }
        TypeDescriptor {
            id: TypeId::new("peat.capability.v1.Capability"),
            name: "Capability".to_string(),
            version: "v1".to_string(),
            canonical_collection: Some("capabilities".to_string()),
            validate_json: validate,
            proto3_zero_fn: proto3_zero,
            fields: vec![
                FieldDescriptor {
                    name: "id",
                    label: "ID",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "name",
                    label: "Name",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "capability_type",
                    label: "Type",
                    format: FieldFormat::Enum {
                        variants: CAPABILITY_TYPE_VARIANTS,
                    },
                },
                FieldDescriptor {
                    name: "confidence",
                    label: "Confidence",
                    format: FieldFormat::Percentage,
                },
                FieldDescriptor {
                    name: "metadata_json",
                    label: "Metadata",
                    format: FieldFormat::JsonString,
                },
                FieldDescriptor {
                    name: "registered_at",
                    label: "Registered",
                    format: FieldFormat::Timestamp,
                },
            ],
        }
    }

    /// `peat.node.v1.NodeConfig`.
    pub fn node_config() -> TypeDescriptor {
        fn validate(value: &Value) -> ValidationResult<()> {
            let msg = crate::node::v1::NodeConfig::deserialize(value).map_err(|e| {
                ValidationError::InvalidValue(format!("{DESERIALISE_ERROR_PREFIX}NodeConfig: {e}"))
            })?;
            crate::validation::validate_node_config(&msg)
        }
        fn proto3_zero() -> Value {
            // peat#953: prost Default + serde Serialize → canonical proto3 zero JSON.
            serde_json::to_value(crate::node::v1::NodeConfig::default())
                .expect("proto3 message serialises to JSON cleanly")
        }
        TypeDescriptor {
            id: TypeId::new("peat.node.v1.NodeConfig"),
            name: "NodeConfig".to_string(),
            version: "v1".to_string(),
            canonical_collection: Some("node-configs".to_string()),
            validate_json: validate,
            proto3_zero_fn: proto3_zero,
            fields: vec![
                FieldDescriptor {
                    name: "id",
                    label: "ID",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "node_type",
                    label: "Node",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "capabilities",
                    label: "Capabilities",
                    format: FieldFormat::List {
                        item_format: Box::new(FieldFormat::Nested {
                            nested_type_id: TypeId::new("peat.capability.v1.Capability"),
                        }),
                    },
                },
                FieldDescriptor {
                    name: "comm_range_m",
                    label: "Comm Range",
                    format: FieldFormat::Number { unit: Some("m") },
                },
                FieldDescriptor {
                    name: "max_speed_mps",
                    label: "Max Speed",
                    format: FieldFormat::Number { unit: Some("m/s") },
                },
                // operator_binding is a nested `HumanMachinePair`. The
                // dedicated descriptor for that type lands in a follow-on
                // commit (no validator in `validation/core` today); until
                // then render as JSON so the renderer doesn't dangle on
                // a Nested reference the registry can't resolve.
                FieldDescriptor {
                    name: "operator_binding",
                    label: "Operator",
                    format: FieldFormat::JsonString,
                },
                FieldDescriptor {
                    name: "created_at",
                    label: "Created",
                    format: FieldFormat::Timestamp,
                },
            ],
        }
    }

    /// `peat.node.v1.NodeState`.
    pub fn node_state() -> TypeDescriptor {
        fn validate(value: &Value) -> ValidationResult<()> {
            let msg = crate::node::v1::NodeState::deserialize(value).map_err(|e| {
                ValidationError::InvalidValue(format!("{DESERIALISE_ERROR_PREFIX}NodeState: {e}"))
            })?;
            crate::validation::validate_node_state(&msg)
        }
        // Proto3 enum variants, indexed by wire integer.
        const HEALTH_STATUS_VARIANTS: &[&str] =
            &["Unspecified", "Nominal", "Degraded", "Critical", "Failed"];
        const PHASE_VARIANTS: &[&str] = &["Unspecified", "Discovery", "Cell", "Hierarchy"];
        fn proto3_zero() -> Value {
            // peat#953: prost Default + serde Serialize → canonical proto3 zero JSON.
            serde_json::to_value(crate::node::v1::NodeState::default())
                .expect("proto3 message serialises to JSON cleanly")
        }
        TypeDescriptor {
            id: TypeId::new("peat.node.v1.NodeState"),
            name: "NodeState".to_string(),
            version: "v1".to_string(),
            canonical_collection: Some("node-states".to_string()),
            validate_json: validate,
            proto3_zero_fn: proto3_zero,
            fields: vec![
                FieldDescriptor {
                    name: "position",
                    label: "Position",
                    format: FieldFormat::Position,
                },
                FieldDescriptor {
                    name: "fuel_minutes",
                    label: "Fuel",
                    format: FieldFormat::Number { unit: Some("min") },
                },
                FieldDescriptor {
                    name: "health",
                    label: "Health",
                    format: FieldFormat::Enum {
                        variants: HEALTH_STATUS_VARIANTS,
                    },
                },
                FieldDescriptor {
                    name: "phase",
                    label: "Phase",
                    format: FieldFormat::Enum {
                        variants: PHASE_VARIANTS,
                    },
                },
                FieldDescriptor {
                    name: "cell_id",
                    label: "Cell",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "zone_id",
                    label: "Zone",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "timestamp",
                    label: "Updated",
                    format: FieldFormat::Timestamp,
                },
            ],
        }
    }

    /// `peat.cell.v1.CellConfig`.
    pub fn cell_config() -> TypeDescriptor {
        fn validate(value: &Value) -> ValidationResult<()> {
            let msg = crate::cell::v1::CellConfig::deserialize(value).map_err(|e| {
                ValidationError::InvalidValue(format!("{DESERIALISE_ERROR_PREFIX}CellConfig: {e}"))
            })?;
            crate::validation::validate_cell_config(&msg)
        }
        fn proto3_zero() -> Value {
            // peat#953: prost Default + serde Serialize → canonical proto3 zero JSON.
            serde_json::to_value(crate::cell::v1::CellConfig::default())
                .expect("proto3 message serialises to JSON cleanly")
        }
        TypeDescriptor {
            id: TypeId::new("peat.cell.v1.CellConfig"),
            name: "CellConfig".to_string(),
            version: "v1".to_string(),
            canonical_collection: Some("cell-configs".to_string()),
            validate_json: validate,
            proto3_zero_fn: proto3_zero,
            fields: vec![
                FieldDescriptor {
                    name: "id",
                    label: "ID",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "max_size",
                    label: "Max Size",
                    format: FieldFormat::Number { unit: None },
                },
                FieldDescriptor {
                    name: "min_size",
                    label: "Min Size",
                    format: FieldFormat::Number { unit: None },
                },
                FieldDescriptor {
                    name: "created_at",
                    label: "Created",
                    format: FieldFormat::Timestamp,
                },
            ],
        }
    }

    /// `peat.cell.v1.CellState`.
    pub fn cell_state() -> TypeDescriptor {
        fn validate(value: &Value) -> ValidationResult<()> {
            let msg = crate::cell::v1::CellState::deserialize(value).map_err(|e| {
                ValidationError::InvalidValue(format!("{DESERIALISE_ERROR_PREFIX}CellState: {e}"))
            })?;
            crate::validation::validate_cell_state(&msg)
        }
        fn proto3_zero() -> Value {
            // peat#953: prost Default + serde Serialize → canonical proto3 zero JSON.
            serde_json::to_value(crate::cell::v1::CellState::default())
                .expect("proto3 message serialises to JSON cleanly")
        }
        TypeDescriptor {
            id: TypeId::new("peat.cell.v1.CellState"),
            name: "CellState".to_string(),
            version: "v1".to_string(),
            canonical_collection: Some("cell-states".to_string()),
            validate_json: validate,
            proto3_zero_fn: proto3_zero,
            fields: vec![
                FieldDescriptor {
                    name: "config",
                    label: "Config",
                    format: FieldFormat::Nested {
                        nested_type_id: TypeId::new("peat.cell.v1.CellConfig"),
                    },
                },
                FieldDescriptor {
                    name: "leader_id",
                    label: "Leader",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "members",
                    label: "Members",
                    format: FieldFormat::List {
                        item_format: Box::new(FieldFormat::Text),
                    },
                },
                FieldDescriptor {
                    name: "capabilities",
                    label: "Capabilities",
                    format: FieldFormat::List {
                        item_format: Box::new(FieldFormat::Nested {
                            nested_type_id: TypeId::new("peat.capability.v1.Capability"),
                        }),
                    },
                },
                FieldDescriptor {
                    name: "cohort_id",
                    label: "Cohort",
                    format: FieldFormat::Text,
                },
                FieldDescriptor {
                    name: "timestamp",
                    label: "Updated",
                    format: FieldFormat::Timestamp,
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typeid_display_and_str() {
        let id = TypeId::new("peat.capability.v1.Capability");
        assert_eq!(id.as_str(), "peat.capability.v1.Capability");
        assert_eq!(id.to_string(), "peat.capability.v1.Capability");
    }

    #[test]
    fn builtin_registry_resolves_core_types_by_id() {
        let r = BuiltinRegistry::with_peat_schema_types();
        for id in [
            "peat.capability.v1.Capability",
            "peat.node.v1.NodeConfig",
            "peat.node.v1.NodeState",
            "peat.cell.v1.CellConfig",
            "peat.cell.v1.CellState",
        ] {
            let desc = r.get(&TypeId::new(id));
            assert!(desc.is_some(), "missing core type: {id}");
            assert_eq!(desc.unwrap().id.as_str(), id);
        }
    }

    #[test]
    fn builtin_registry_resolves_by_canonical_collection() {
        let r = BuiltinRegistry::with_peat_schema_types();
        assert_eq!(
            r.for_collection("capabilities").map(|d| d.id.as_str()),
            Some("peat.capability.v1.Capability")
        );
        assert_eq!(
            r.for_collection("node-configs").map(|d| d.id.as_str()),
            Some("peat.node.v1.NodeConfig")
        );
        assert!(r.for_collection("unknown-collection").is_none());
    }

    #[test]
    fn iter_lists_all_registered_types() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let ids: Vec<&str> = r.iter().map(|d| d.id.as_str()).collect();
        // Order is HashMap-defined; just check membership.
        assert!(ids.contains(&"peat.capability.v1.Capability"));
        assert!(ids.contains(&"peat.node.v1.NodeConfig"));
        assert!(ids.contains(&"peat.cell.v1.CellState"));
        assert_eq!(ids.len(), 5);
    }

    /// Build a JSON object matching the Capability proto3 shape.
    /// (id, name, capability_type, confidence, metadata_json, registered_at.)
    fn capability_json(confidence: f32) -> serde_json::Value {
        json!({
            "id": "cap-1",
            "name": "thermal-sensor",
            "capability_type": 0,
            "confidence": confidence,
            "metadata_json": "{}",
            "registered_at": null,
        })
    }

    #[test]
    fn capability_json_validator_accepts_well_formed_input() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("capabilities").unwrap();
        let result = (desc.validate_json)(&capability_json(0.95));
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn capability_json_validator_rejects_invalid_confidence() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("capabilities").unwrap();
        let err =
            (desc.validate_json)(&capability_json(1.5)).expect_err("expected validation error");
        assert!(matches!(err, ValidationError::InvalidConfidence(_)));
    }

    #[test]
    fn capability_json_validator_rejects_malformed_json() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("capabilities").unwrap();

        // Missing all the typed fields the protobuf message expects;
        // serde_json::from_value should fail.
        let value = json!({"not_a_capability": true});
        let err = (desc.validate_json)(&value).expect_err("expected deserialise error");
        match err {
            ValidationError::InvalidValue(msg) => assert!(
                msg.contains("Capability"),
                "expected message naming Capability; got {msg}"
            ),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn consumer_can_extend_with_own_types() {
        // Construct an empty registry, register one type. Demonstrates the
        // extension surface for applications that ship their own document
        // shapes alongside peat-schema's.
        fn always_ok(_v: &Value) -> ValidationResult<()> {
            Ok(())
        }
        // Demonstrates the external-consumer pattern: use the constructor
        // (since `TypeDescriptor` is `#[non_exhaustive]` outside the
        // crate), then set optional fields via direct assignment.
        let mut custom = TypeDescriptor::new(
            TypeId::new("example.app.v1.Widget"),
            "Widget",
            "v1",
            always_ok,
        );
        custom.canonical_collection = Some("widgets".to_string());
        custom.fields = vec![FieldDescriptor::new("label", "Label", FieldFormat::Text)];
        let mut r = BuiltinRegistry::new();
        r.register(custom);
        assert!(r.get(&TypeId::new("example.app.v1.Widget")).is_some());
        assert_eq!(
            r.for_collection("widgets").map(|d| d.id.as_str()),
            Some("example.app.v1.Widget")
        );
    }

    #[test]
    fn core_types_carry_field_metadata() {
        let r = BuiltinRegistry::with_peat_schema_types();

        // Capability has the expected fields, in declaration order.
        let cap = r.for_collection("capabilities").unwrap();
        let field_names: Vec<&str> = cap.fields.iter().map(|f| f.name).collect();
        assert_eq!(
            field_names,
            vec![
                "id",
                "name",
                "capability_type",
                "confidence",
                "metadata_json",
                "registered_at"
            ]
        );

        // confidence carries the Percentage format hint.
        let confidence = cap.fields.iter().find(|f| f.name == "confidence").unwrap();
        assert_eq!(confidence.format, FieldFormat::Percentage);

        // capability_type is an Enum with the documented variants in
        // wire-integer order.
        let cap_type = cap
            .fields
            .iter()
            .find(|f| f.name == "capability_type")
            .unwrap();
        match &cap_type.format {
            FieldFormat::Enum { variants } => {
                assert_eq!(variants[0], "Unspecified");
                assert_eq!(variants[1], "Sensor");
                assert_eq!(variants[5], "Payload");
            }
            other => panic!("expected Enum format, got {other:?}"),
        }
    }

    #[test]
    fn node_state_position_and_units_are_typed() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let ns = r.for_collection("node-states").unwrap();

        let position = ns.fields.iter().find(|f| f.name == "position").unwrap();
        assert_eq!(position.format, FieldFormat::Position);

        let fuel = ns.fields.iter().find(|f| f.name == "fuel_minutes").unwrap();
        assert_eq!(fuel.format, FieldFormat::Number { unit: Some("min") });
    }

    #[test]
    fn nested_descriptors_link_via_type_id() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let nc = r.for_collection("node-configs").unwrap();

        let caps = nc.fields.iter().find(|f| f.name == "capabilities").unwrap();
        // capabilities is a List<Nested(Capability)>.
        match &caps.format {
            FieldFormat::List { item_format } => match item_format.as_ref() {
                FieldFormat::Nested { nested_type_id } => {
                    assert_eq!(nested_type_id.as_str(), "peat.capability.v1.Capability");
                    // The nested type is resolvable through the registry.
                    assert!(r.get(nested_type_id).is_some());
                }
                other => panic!("expected Nested item format, got {other:?}"),
            },
            other => panic!("expected List format, got {other:?}"),
        }
    }

    /// Build a minimal well-formed JSON for each of the four
    /// non-Capability core types. Each one matches its proto3
    /// message shape and satisfies the validator's field-level
    /// constraints (see `peat-schema/src/validation/core.rs`).
    /// Exercising these through the registry's `validate_json` fn
    /// pointer guards against descriptor wire-up mismatches —
    /// caught by peat#947 QA review (the wrong `validate_*` pointer
    /// or wrong target proto type would compile fine but surface
    /// only at downstream call sites).
    mod fixtures {
        use serde_json::{json, Value};

        pub fn node_config() -> Value {
            json!({
                "id": "node-1",
                "node_type": "UAV",
                "capabilities": [],
                "comm_range_m": 1000.0,
                "max_speed_mps": 10.0,
                "operator_binding": null,
                "created_at": null,
            })
        }

        pub fn node_state() -> Value {
            json!({
                "position": {
                    "latitude": 38.0,
                    "longitude": -122.0,
                    "altitude": 0.0,
                },
                "fuel_minutes": 60,
                "health": 1,  // HealthStatus::Nominal
                "phase": 1,   // Phase::Discovery
                "cell_id": null,
                "zone_id": null,
                "timestamp": null,
            })
        }

        pub fn cell_config() -> Value {
            json!({
                "id": "cell-1",
                "max_size": 8,
                "min_size": 2,
                "created_at": null,
            })
        }

        pub fn cell_state() -> Value {
            json!({
                "config": {
                    "id": "cell-1",
                    "max_size": 8,
                    "min_size": 2,
                    "created_at": null,
                },
                "leader_id": null,
                "members": [],
                "capabilities": [],
                "platoon_id": null,
                "timestamp": null,
            })
        }
    }

    #[test]
    fn node_config_validator_accepts_well_formed_input() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("node-configs").unwrap();
        let result = (desc.validate_json)(&fixtures::node_config());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn node_state_validator_accepts_well_formed_input() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("node-states").unwrap();
        let result = (desc.validate_json)(&fixtures::node_state());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn cell_config_validator_accepts_well_formed_input() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("cell-configs").unwrap();
        let result = (desc.validate_json)(&fixtures::cell_config());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn cell_state_validator_accepts_well_formed_input() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("cell-states").unwrap();
        let result = (desc.validate_json)(&fixtures::cell_state());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn node_state_validator_rejects_out_of_range_latitude() {
        // Spot-check that the non-Capability validators actually
        // dispatch to their respective field-constraint check (not
        // just deserialise). A latitude of 95.0 violates the
        // validate_node_state range check.
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("node-states").unwrap();
        let mut bad = fixtures::node_state();
        bad["position"]["latitude"] = serde_json::json!(95.0);
        let err = (desc.validate_json)(&bad).expect_err("expected validation error");
        match err {
            ValidationError::InvalidValue(msg) => {
                assert!(msg.contains("latitude"), "got {msg}");
            }
            other => panic!("expected InvalidValue (range), got {other:?}"),
        }
    }

    #[test]
    fn cell_config_validator_rejects_undersize_min() {
        // min_size < 2 is a ConstraintViolation per validate_cell_config.
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("cell-configs").unwrap();
        let mut bad = fixtures::cell_config();
        bad["min_size"] = serde_json::json!(1);
        let err = (desc.validate_json)(&bad).expect_err("expected validation error");
        assert!(
            matches!(err, ValidationError::ConstraintViolation(_)),
            "{err:?}"
        );
    }

    #[test]
    fn every_nested_reference_in_builtin_registry_resolves() {
        // Guard test (peat#947 QA finding): if any pre-registered
        // descriptor names a `Scope::Nested` (directly or wrapped in a
        // `List`), the referenced TypeId must also be registered.
        // Without this, a renderer that follows the link receives None
        // and silently drops the field. Adding a new descriptor that
        // points at an unregistered type fails this test loudly.
        let r = BuiltinRegistry::with_peat_schema_types();

        fn collect_nested_refs(fmt: &FieldFormat, out: &mut Vec<TypeId>) {
            match fmt {
                FieldFormat::Nested { nested_type_id } => out.push(nested_type_id.clone()),
                FieldFormat::List { item_format } => {
                    collect_nested_refs(item_format, out);
                }
                _ => {}
            }
        }

        let mut unresolved = Vec::new();
        for desc in r.iter() {
            for field in &desc.fields {
                let mut refs = Vec::new();
                collect_nested_refs(&field.format, &mut refs);
                for nested in refs {
                    if r.get(&nested).is_none() {
                        unresolved.push(format!(
                            "{}::{} → {} (unregistered)",
                            desc.id, field.name, nested
                        ));
                    }
                }
            }
        }
        assert!(
            unresolved.is_empty(),
            "Builtin registry has unresolved Nested references:\n  {}",
            unresolved.join("\n  ")
        );
    }

    #[test]
    fn registry_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BuiltinRegistry>();
        // Dyn-trait obj as well, to confirm consumers can hand it across threads.
        fn _accepts(_: &dyn TypeRegistry) {}
    }

    /// peat#953: `TypeDescriptor::proto3_zero()` must return a JSON
    /// Object (the proto3 wire-zero shape, never `null` or a scalar)
    /// for every type the builtin registry ships with.
    ///
    /// Per-type spot-check is in
    /// `proto3_zero_capability_matches_default_serialised`; this
    /// test pins the universal shape invariant across the whole
    /// registry so a future descriptor added without a
    /// `proto3_zero_fn` (which would fall back to the no-op
    /// returning `{}`) still satisfies the contract by accident
    /// of the fallback — but a descriptor whose underlying type
    /// somehow serialises to a non-object would surface here
    /// rather than only at the first consumer call site.
    #[test]
    fn proto3_zero_is_object_for_every_registered_type() {
        let r = BuiltinRegistry::with_peat_schema_types();
        for desc in r.iter() {
            let zero = desc.proto3_zero();
            assert!(
                zero.is_object(),
                "{}::proto3_zero() returned non-object value: {zero}",
                desc.id
            );
        }
    }

    /// peat#953: the round-trip property the API exists to provide.
    /// For every registered type, `desc.proto3_zero()` must
    /// deserialise cleanly through the type's strict prost-derived
    /// `Deserialize` impl — proven by feeding the output back into
    /// the descriptor's own `validate_json` adapter, which
    /// internally calls `<T>::deserialize(value)` before the
    /// business-rule validators.
    ///
    /// Note: the business-rule validators (e.g. "id must not be
    /// empty") MAY reject a proto3 zero, since "" is the proto3
    /// default for `string`. That's fine — we accept any error
    /// shape OTHER than a deserialise failure. The contract we're
    /// pinning is "the zero JSON is a valid wire-form for the
    /// type"; the business-rule layer is a separate concern.
    ///
    /// The deserialise-failure-detection guard matches on
    /// [`DESERIALISE_ERROR_PREFIX`] — the same `const` the five
    /// `validate_json` closures in [`mod@super::descriptors`]
    /// produce. Coupling producer and detector through a single
    /// `const` (rather than duplicating the string literal) means a
    /// future prefix rename surfaces as a compile error rather than
    /// as a silent test-coverage gap where the catch-all `_other`
    /// arm starts swallowing real deserialise failures.
    #[test]
    fn proto3_zero_deserialises_cleanly_for_every_registered_type() {
        let r = BuiltinRegistry::with_peat_schema_types();
        for desc in r.iter() {
            let zero = desc.proto3_zero();
            match (desc.validate_json)(&zero) {
                Ok(()) => {} // type-valid and business-valid (rare for zero values)
                Err(ValidationError::InvalidValue(msg))
                    if msg.starts_with(DESERIALISE_ERROR_PREFIX) =>
                {
                    panic!(
                        "{}::proto3_zero() produced a value that fails strict prost-derived \
                         deserialise — the round-trip-by-construction property is broken: {msg}",
                        desc.id
                    );
                }
                Err(_other) => {
                    // Business-rule validation rejected a zero-valued field
                    // (e.g. empty required ID string). That's expected and
                    // out of scope for this test.
                }
            }
        }
    }

    /// peat#953: spot-check the shape of one descriptor's proto3
    /// zero to catch any future refactor that detaches the
    /// `proto3_zero_fn` from the prost-generated `Default`
    /// (e.g. somebody manually constructs a `serde_json::json!({})`
    /// in a descriptor and the field list silently goes stale).
    ///
    /// For `Capability`, the proto3 zero must contain at least one
    /// known field name with its proto3 default value:
    /// `id` → `""`, `confidence` → `0.0`, `metadata_json` → `""`.
    /// If any of these go missing, the descriptor likely isn't
    /// wired to prost's `Default` anymore.
    #[test]
    fn proto3_zero_capability_matches_default_serialised() {
        let r = BuiltinRegistry::with_peat_schema_types();
        let desc = r.for_collection("capabilities").expect("registered");
        let zero = desc.proto3_zero();
        let obj = zero.as_object().expect("object");

        // Spot-check: these three fields must be present with their
        // proto3 zero values. If the descriptor stops being driven
        // by prost::Default, this test surfaces the drift.
        assert_eq!(
            obj.get("id"),
            Some(&Value::String(String::new())),
            "Capability::proto3_zero must contain id = \"\" (proto3 string default)"
        );
        assert_eq!(
            obj.get("confidence"),
            Some(&Value::Number(serde_json::Number::from_f64(0.0).unwrap())),
            "Capability::proto3_zero must contain confidence = 0.0 (proto3 float default)"
        );
        assert_eq!(
            obj.get("metadata_json"),
            Some(&Value::String(String::new())),
            "Capability::proto3_zero must contain metadata_json = \"\" (real string field, not optional)"
        );
    }

    /// peat#953 fallback: `TypeDescriptor::new(…)` without explicit
    /// `proto3_zero_fn` produces the no-op default that returns
    /// `{}`. Consumers that build their own descriptors via the
    /// public `new` API get the empty-object fallback (safe under
    /// the merge-on-top-of-defaults pattern) rather than a null
    /// or scalar that would break consumers.
    #[test]
    fn proto3_zero_fallback_for_descriptor_built_via_new_is_empty_object() {
        fn no_op_validate(_: &Value) -> ValidationResult<()> {
            Ok(())
        }
        let desc = TypeDescriptor::new(
            TypeId::new("test.fallback.v1.Anon"),
            "Anon",
            "v1",
            no_op_validate,
        );
        let zero = desc.proto3_zero();
        assert_eq!(
            zero,
            Value::Object(serde_json::Map::new()),
            "descriptor built via new(…) without proto3_zero_fn must fall back to {{}}"
        );
    }
}
