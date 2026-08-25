//! Transport-independent collection history contract (ADR-076).
//!
//! Wire-visible types and validation originate in `peat-schema`. Storage and
//! synchronization implementations consume these declarations but remain
//! responsible for enforcement and observable status.

pub use peat_schema::history::v1::*;
pub use peat_schema::validation::{
    conservative_causal_retention_mode, conservative_synchronization_mode,
    validate_collection_history_policy, validate_durability_progress,
    validate_history_epoch_status, validate_history_segment_descriptor,
    validate_history_segment_status, validate_migrated_collection_history_policy,
};
