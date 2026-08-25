//! Validation and conservative compatibility rules for ADR-076 history policy.

use super::{ValidationError, ValidationResult};
use crate::common::v1::Timestamp;
use crate::history::v1::{
    CausalRetentionMode, CollectionHistoryPolicy, DurabilityPolicy, DurabilityProgress,
    DurabilityTarget, EpochLifecycle, HistoryEpochStatus, HistoryRequirement,
    HistorySegmentDescriptor, HistorySegmentStatus, HistorySourceReference, OverBudgetBehavior,
    ResurrectionBehavior, SegmentLifecycle, SegmentPolicy, StaleWriterBehavior,
    SynchronizationMode,
};

/// Resolve absent or unknown legacy synchronization values without authorizing
/// history loss.
pub fn conservative_synchronization_mode(value: i32) -> SynchronizationMode {
    match SynchronizationMode::try_from(value) {
        Ok(SynchronizationMode::LatestOnly) => SynchronizationMode::LatestOnly,
        Ok(SynchronizationMode::Windowed) => SynchronizationMode::Windowed,
        Ok(SynchronizationMode::FullHistory | SynchronizationMode::Unspecified) | Err(_) => {
            SynchronizationMode::FullHistory
        }
    }
}

/// Resolve absent or unknown causal-retention values to complete retention.
pub fn conservative_causal_retention_mode(value: i32) -> CausalRetentionMode {
    match CausalRetentionMode::try_from(value) {
        Ok(CausalRetentionMode::UntilDurableCheckpoint) => {
            CausalRetentionMode::UntilDurableCheckpoint
        }
        Ok(CausalRetentionMode::CurrentState) => CausalRetentionMode::CurrentState,
        Ok(CausalRetentionMode::Complete | CausalRetentionMode::Unspecified) | Err(_) => {
            CausalRetentionMode::Complete
        }
    }
}

/// Validate an explicit production collection-history declaration.
///
/// Legacy declarations may contain unspecified values while they are being
/// migrated. Callers must resolve those values conservatively rather than pass
/// them through this strict validator.
pub fn validate_collection_history_policy(
    policy: &CollectionHistoryPolicy,
) -> ValidationResult<()> {
    if policy.collection.trim().is_empty() {
        return Err(ValidationError::MissingField("collection".to_string()));
    }

    let synchronization = policy
        .synchronization
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("synchronization".to_string()))?;
    let sync_mode =
        parse_required_enum::<SynchronizationMode>(synchronization.mode, "synchronization.mode")?;
    match sync_mode {
        SynchronizationMode::Windowed => {
            require_positive(
                synchronization.window_seconds,
                "synchronization.window_seconds",
            )?;
        }
        SynchronizationMode::FullHistory | SynchronizationMode::LatestOnly => {
            if synchronization.window_seconds.is_some() {
                return Err(ValidationError::ConstraintViolation(
                    "synchronization.window_seconds is valid only for windowed synchronization"
                        .to_string(),
                ));
            }
        }
        SynchronizationMode::Unspecified => unreachable!("required enum rejects unspecified"),
    }

    let causal_retention = policy
        .causal_retention
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("causal_retention".to_string()))?;
    let causal_mode =
        parse_required_enum::<CausalRetentionMode>(causal_retention.mode, "causal_retention.mode")?;
    let history_requirement = parse_required_enum::<HistoryRequirement>(
        policy.history_requirement,
        "history_requirement",
    )?;
    let over_budget = parse_required_enum::<OverBudgetBehavior>(
        policy.over_budget_behavior,
        "over_budget_behavior",
    )?;
    parse_required_enum::<StaleWriterBehavior>(
        policy.stale_writer_behavior,
        "stale_writer_behavior",
    )?;
    parse_required_enum::<ResurrectionBehavior>(
        policy.resurrection_behavior,
        "resurrection_behavior",
    )?;

    let sync_retention_compatible = match sync_mode {
        SynchronizationMode::FullHistory => causal_mode == CausalRetentionMode::Complete,
        SynchronizationMode::Windowed => causal_mode == CausalRetentionMode::Complete,
        SynchronizationMode::LatestOnly => true,
        SynchronizationMode::Unspecified => unreachable!("required enum rejects unspecified"),
    };
    if !sync_retention_compatible {
        return Err(ValidationError::ConstraintViolation(
            "synchronization.mode requires enough causal history to produce that synchronization behavior"
                .to_string(),
        ));
    }

    validate_durability_policy(required_durability(policy)?)?;

    match history_requirement {
        HistoryRequirement::CurrentStateOnly => {
            if policy.history_source.is_some() {
                return Err(ValidationError::ConstraintViolation(
                    "history_source must be absent for current-state-only history".to_string(),
                ));
            }
            if policy.segment_policy.is_some() {
                return Err(ValidationError::ConstraintViolation(
                    "segment_policy must be absent for current-state-only history".to_string(),
                ));
            }
            if causal_mode == CausalRetentionMode::UntilDurableCheckpoint {
                return Err(ValidationError::ConstraintViolation(
                    "causal_retention requires a reconstructible history source before checkpointing"
                        .to_string(),
                ));
            }
        }
        HistoryRequirement::BoundedReconstructible
        | HistoryRequirement::CompleteReconstructible => {
            validate_history_source(required_history_source(policy)?)?;
            validate_segment_policy(required_segment_policy(policy)?, history_requirement)?;

            // All currently representable over-budget values preserve the
            // explicit contract. Parsing above excludes unspecified values.
            debug_assert!(matches!(
                over_budget,
                OverBudgetBehavior::RetainLocal
                    | OverBudgetBehavior::Backpressure
                    | OverBudgetBehavior::Reject
            ));
        }
        HistoryRequirement::Unspecified => unreachable!("required enum rejects unspecified"),
    }

    Ok(())
}

/// Validate a policy produced from the legacy `SyncMode` surface.
///
/// Legacy values intentionally leave domain history, durability, and budget
/// behavior unspecified when they had no equivalent. This validator accepts
/// only that conservative incomplete shape; explicit production declarations
/// must use [`validate_collection_history_policy`].
pub fn validate_migrated_collection_history_policy(
    policy: &CollectionHistoryPolicy,
) -> ValidationResult<()> {
    if policy.collection.trim().is_empty() {
        return Err(ValidationError::MissingField("collection".to_string()));
    }
    if policy.history_source.is_some()
        || policy.segment_policy.is_some()
        || policy.durability.is_some()
    {
        return Err(ValidationError::ConstraintViolation(
            "migrated sync-mode policy cannot invent history, segmentation, or durability claims"
                .to_string(),
        ));
    }
    if policy.over_budget_behavior != OverBudgetBehavior::Unspecified as i32 {
        return Err(ValidationError::ConstraintViolation(
            "migrated sync-mode policy must leave over_budget_behavior unspecified".to_string(),
        ));
    }
    if policy.stale_writer_behavior != StaleWriterBehavior::Unspecified as i32
        || policy.resurrection_behavior != ResurrectionBehavior::Unspecified as i32
    {
        return Err(ValidationError::ConstraintViolation(
            "migrated sync-mode policy cannot invent stale-writer or resurrection behavior"
                .to_string(),
        ));
    }

    let synchronization = policy
        .synchronization
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("synchronization".to_string()))?;
    let sync_mode =
        parse_required_enum::<SynchronizationMode>(synchronization.mode, "synchronization.mode")?;
    match sync_mode {
        SynchronizationMode::Windowed => require_positive(
            synchronization.window_seconds,
            "synchronization.window_seconds",
        )?,
        SynchronizationMode::FullHistory | SynchronizationMode::LatestOnly => {
            if synchronization.window_seconds.is_some() {
                return Err(ValidationError::ConstraintViolation(
                    "synchronization.window_seconds is valid only for windowed synchronization"
                        .to_string(),
                ));
            }
        }
        SynchronizationMode::Unspecified => unreachable!("required enum rejects unspecified"),
    }

    let causal_retention = policy
        .causal_retention
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("causal_retention".to_string()))?;
    let causal_mode =
        parse_required_enum::<CausalRetentionMode>(causal_retention.mode, "causal_retention.mode")?;
    let history = HistoryRequirement::try_from(policy.history_requirement).map_err(|_| {
        ValidationError::InvalidValue(format!(
            "history_requirement contains unknown enum value {}",
            policy.history_requirement
        ))
    })?;

    let valid_mapping = matches!(
        (sync_mode, causal_mode, history),
        (
            SynchronizationMode::FullHistory,
            CausalRetentionMode::Complete,
            HistoryRequirement::Unspecified
        ) | (
            SynchronizationMode::LatestOnly,
            CausalRetentionMode::CurrentState,
            HistoryRequirement::CurrentStateOnly
        ) | (
            SynchronizationMode::Windowed,
            CausalRetentionMode::Complete,
            HistoryRequirement::Unspecified
        )
    );
    if !valid_mapping {
        return Err(ValidationError::ConstraintViolation(
            "migrated sync-mode policy contains an unsupported compatibility mapping".to_string(),
        ));
    }
    Ok(())
}

/// Validate an observable history-segment lifecycle and durability report.
pub fn validate_history_segment_status(status: &HistorySegmentStatus) -> ValidationResult<()> {
    let lifecycle = parse_required_enum::<SegmentLifecycle>(status.lifecycle, "lifecycle")?;
    let segment = status
        .segment
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("segment".to_string()))?;
    validate_history_segment_descriptor(segment)?;
    let durability = status
        .durability
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("durability".to_string()))?;
    validate_durability_progress(durability)?;
    let evaluated_at = status
        .evaluated_at
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("evaluated_at".to_string()))?;
    validate_timestamp(evaluated_at, "evaluated_at")?;

    match lifecycle {
        SegmentLifecycle::Active => {
            if durability.target_met
                || status.retention_eligible
                || segment.content_sha256.is_some()
                || segment.content_encoding.is_some()
            {
                return Err(ValidationError::ConstraintViolation(
                    "active segment cannot be durably acknowledged or carry sealed content identity"
                        .to_string(),
                ));
            }
        }
        SegmentLifecycle::Sealed => {
            require_sealed_content_identity(segment)?;
            if durability.target_met || status.retention_eligible {
                return Err(ValidationError::ConstraintViolation(
                    "sealed segment cannot be retention-eligible and must advance when durability is met"
                        .to_string(),
                ));
            }
        }
        SegmentLifecycle::DurablyAcknowledged => {
            require_sealed_content_identity(segment)?;
            if !durability.target_met || status.retention_eligible {
                return Err(ValidationError::ConstraintViolation(
                    "durably acknowledged lifecycle requires met durability and must precede retention eligibility"
                        .to_string(),
                ));
            }
        }
        SegmentLifecycle::RetentionEligible | SegmentLifecycle::Removed => {
            require_sealed_content_identity(segment)?;
            if !durability.target_met || !status.retention_eligible {
                return Err(ValidationError::ConstraintViolation(
                    "retention-eligible or removed lifecycle requires met durability and retention eligibility"
                        .to_string(),
                ));
            }
        }
        SegmentLifecycle::Unspecified => unreachable!("required enum rejects unspecified"),
    }

    if status.retention_eligible && durability.target == DurabilityTarget::Local as i32 {
        return Err(ValidationError::ConstraintViolation(
            "local-only durability cannot make its sole durable copy retention-eligible"
                .to_string(),
        ));
    }

    if let Some(not_before) = &status.retention_not_before {
        validate_timestamp(not_before, "retention_not_before")?;
    }
    if status.retention_eligible {
        let not_before = status
            .retention_not_before
            .as_ref()
            .ok_or_else(|| ValidationError::MissingField("retention_not_before".to_string()))?;
        if timestamp_key(evaluated_at) < timestamp_key(not_before) {
            return Err(ValidationError::ConstraintViolation(
                "retention eligibility cannot precede retention_not_before".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate stable identity, ordering, and content identity for a segment.
pub fn validate_history_segment_descriptor(
    segment: &HistorySegmentDescriptor,
) -> ValidationResult<()> {
    for (field, value) in [
        ("segment.collection", segment.collection.as_str()),
        ("segment.source_id", segment.source_id.as_str()),
        ("segment.epoch_id", segment.epoch_id.as_str()),
        ("segment.segment_id", segment.segment_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ValidationError::MissingField(field.to_string()));
        }
    }
    if segment.sequence_end < segment.sequence_start {
        return Err(ValidationError::ConstraintViolation(
            "segment.sequence_end must be greater than or equal to sequence_start".to_string(),
        ));
    }
    if let Some(starts_at) = &segment.starts_at {
        validate_timestamp(starts_at, "segment.starts_at")?;
    }
    if let Some(ends_at) = &segment.ends_at {
        validate_timestamp(ends_at, "segment.ends_at")?;
    }
    if let (Some(starts_at), Some(ends_at)) = (&segment.starts_at, &segment.ends_at) {
        if timestamp_key(starts_at) > timestamp_key(ends_at) {
            return Err(ValidationError::ConstraintViolation(
                "segment.starts_at must not be later than ends_at".to_string(),
            ));
        }
    }
    if segment
        .content_sha256
        .as_ref()
        .is_some_and(|digest| digest.len() != 32)
    {
        return Err(ValidationError::InvalidValue(
            "segment.content_sha256 must contain exactly 32 bytes".to_string(),
        ));
    }
    if segment
        .content_encoding
        .as_ref()
        .is_some_and(|encoding| encoding.trim().is_empty())
    {
        return Err(ValidationError::InvalidValue(
            "segment.content_encoding must be non-empty when present".to_string(),
        ));
    }
    for (field, value) in [
        (
            "segment.predecessor_segment_id",
            segment.predecessor_segment_id.as_ref(),
        ),
        ("segment.checkpoint_id", segment.checkpoint_id.as_ref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(ValidationError::InvalidValue(format!(
                "{field} must be non-empty when present"
            )));
        }
    }
    Ok(())
}

/// Validate an epoch transition used to reject writes from stale replicas.
pub fn validate_history_epoch_status(status: &HistoryEpochStatus) -> ValidationResult<()> {
    for (field, value) in [
        ("collection", status.collection.as_str()),
        ("source_id", status.source_id.as_str()),
        ("epoch_id", status.epoch_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ValidationError::MissingField(field.to_string()));
        }
    }

    let lifecycle = parse_required_enum::<EpochLifecycle>(status.lifecycle, "lifecycle")?;
    match lifecycle {
        EpochLifecycle::Active => {
            if status.successor_epoch_id.is_some() {
                return Err(ValidationError::ConstraintViolation(
                    "active epoch cannot declare a successor_epoch_id".to_string(),
                ));
            }
        }
        EpochLifecycle::Closed => {
            if status
                .successor_epoch_id
                .as_ref()
                .map_or(true, |value| value.trim().is_empty())
            {
                return Err(ValidationError::MissingField(
                    "successor_epoch_id".to_string(),
                ));
            }
            if status.successor_epoch_id.as_deref() == Some(status.epoch_id.as_str()) {
                return Err(ValidationError::ConstraintViolation(
                    "successor_epoch_id must differ from the closed epoch_id".to_string(),
                ));
            }
        }
        EpochLifecycle::Unspecified => unreachable!("required enum rejects unspecified"),
    }
    if status
        .closing_checkpoint_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ValidationError::InvalidValue(
            "closing_checkpoint_id must be non-empty when present".to_string(),
        ));
    }
    Ok(())
}

fn require_sealed_content_identity(segment: &HistorySegmentDescriptor) -> ValidationResult<()> {
    if segment.content_sha256.is_none() {
        return Err(ValidationError::MissingField(
            "segment.content_sha256".to_string(),
        ));
    }
    if segment
        .content_encoding
        .as_ref()
        .map_or(true, |encoding| encoding.trim().is_empty())
    {
        return Err(ValidationError::MissingField(
            "segment.content_encoding".to_string(),
        ));
    }
    Ok(())
}

fn validate_timestamp(timestamp: &Timestamp, field: &str) -> ValidationResult<()> {
    if timestamp.nanos >= 1_000_000_000 {
        return Err(ValidationError::InvalidValue(format!(
            "{field}.nanos must be less than 1000000000"
        )));
    }
    Ok(())
}

fn timestamp_key(timestamp: &Timestamp) -> (u64, u32) {
    (timestamp.seconds, timestamp.nanos)
}

/// Validate reported acknowledgement progress against its durability target.
pub fn validate_durability_progress(progress: &DurabilityProgress) -> ValidationResult<()> {
    let target = parse_required_enum::<DurabilityTarget>(progress.target, "durability.target")?;
    validate_copy_count(
        target,
        progress.required_copies,
        "durability.required_copies",
    )?;

    let computed_target_met = progress.acknowledged_copies >= progress.required_copies;
    if progress.target_met != computed_target_met {
        return Err(ValidationError::ConstraintViolation(
            "durability.target_met must match acknowledged versus required copies".to_string(),
        ));
    }
    Ok(())
}

fn validate_history_source(source: &HistorySourceReference) -> ValidationResult<()> {
    if source.collection.trim().is_empty() {
        return Err(ValidationError::MissingField(
            "history_source.collection".to_string(),
        ));
    }
    if source.source_id.trim().is_empty() {
        return Err(ValidationError::MissingField(
            "history_source.source_id".to_string(),
        ));
    }
    if source
        .checkpoint_collection
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ValidationError::InvalidValue(
            "history_source.checkpoint_collection must be non-empty when present".to_string(),
        ));
    }
    if source
        .catalog_collection
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ValidationError::InvalidValue(
            "history_source.catalog_collection must be non-empty when present".to_string(),
        ));
    }
    Ok(())
}

fn validate_segment_policy(
    segment: &SegmentPolicy,
    requirement: HistoryRequirement,
) -> ValidationResult<()> {
    if segment.max_age_seconds == 0
        && segment.max_events == 0
        && segment.max_bytes == 0
        && segment.max_revisions == 0
    {
        return Err(ValidationError::ConstraintViolation(
            "segment_policy must declare at least one positive rotation limit".to_string(),
        ));
    }

    match requirement {
        HistoryRequirement::BoundedReconstructible => {
            require_positive(
                segment.retention_seconds,
                "segment_policy.retention_seconds",
            )?;
        }
        HistoryRequirement::CompleteReconstructible => {
            if segment.retention_seconds.is_some() {
                return Err(ValidationError::ConstraintViolation(
                    "complete reconstructible history cannot declare a retention expiry"
                        .to_string(),
                ));
            }
        }
        _ => unreachable!("segment policy is validated only for reconstructible history"),
    }
    Ok(())
}

fn validate_durability_policy(durability: &DurabilityPolicy) -> ValidationResult<()> {
    let target = parse_required_enum::<DurabilityTarget>(durability.target, "durability.target")?;
    validate_copy_count(
        target,
        durability.minimum_copies,
        "durability.minimum_copies",
    )
}

fn validate_copy_count(target: DurabilityTarget, copies: u32, field: &str) -> ValidationResult<()> {
    let valid = match target {
        DurabilityTarget::Local | DurabilityTarget::Archive => copies == 1,
        DurabilityTarget::Replicated => copies >= 2,
        DurabilityTarget::Unspecified => unreachable!("required enum rejects unspecified"),
    };
    if !valid {
        return Err(ValidationError::InvalidValue(format!(
            "{field} must be 1 for local/archive durability or at least 2 for replicated durability"
        )));
    }
    Ok(())
}

fn required_history_source(
    policy: &CollectionHistoryPolicy,
) -> ValidationResult<&HistorySourceReference> {
    policy
        .history_source
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("history_source".to_string()))
}

fn required_segment_policy(policy: &CollectionHistoryPolicy) -> ValidationResult<&SegmentPolicy> {
    policy
        .segment_policy
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("segment_policy".to_string()))
}

fn required_durability(policy: &CollectionHistoryPolicy) -> ValidationResult<&DurabilityPolicy> {
    policy
        .durability
        .as_ref()
        .ok_or_else(|| ValidationError::MissingField("durability".to_string()))
}

fn require_positive(value: Option<u64>, field: &str) -> ValidationResult<()> {
    if value.map_or(true, |value| value == 0) {
        return Err(ValidationError::InvalidValue(format!(
            "{field} must be present and positive"
        )));
    }
    Ok(())
}

trait ProtoEnum: TryFrom<i32> + Copy {
    fn is_unspecified(self) -> bool;
}

macro_rules! impl_proto_enum {
    ($type:ty, $unspecified:path) => {
        impl ProtoEnum for $type {
            fn is_unspecified(self) -> bool {
                self == $unspecified
            }
        }
    };
}

impl_proto_enum!(SynchronizationMode, SynchronizationMode::Unspecified);
impl_proto_enum!(CausalRetentionMode, CausalRetentionMode::Unspecified);
impl_proto_enum!(HistoryRequirement, HistoryRequirement::Unspecified);
impl_proto_enum!(DurabilityTarget, DurabilityTarget::Unspecified);
impl_proto_enum!(OverBudgetBehavior, OverBudgetBehavior::Unspecified);
impl_proto_enum!(SegmentLifecycle, SegmentLifecycle::Unspecified);
impl_proto_enum!(StaleWriterBehavior, StaleWriterBehavior::Unspecified);
impl_proto_enum!(ResurrectionBehavior, ResurrectionBehavior::Unspecified);
impl_proto_enum!(EpochLifecycle, EpochLifecycle::Unspecified);

fn parse_required_enum<E>(value: i32, field: &str) -> ValidationResult<E>
where
    E: ProtoEnum,
{
    let parsed = E::try_from(value).map_err(|_| {
        ValidationError::InvalidValue(format!("{field} contains unknown enum value {value}"))
    })?;
    if parsed.is_unspecified() {
        return Err(ValidationError::MissingField(field.to_string()));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::v1::{CausalRetentionPolicy, SynchronizationPolicy};

    fn bounded_policy() -> CollectionHistoryPolicy {
        CollectionHistoryPolicy {
            collection: "tracks-current".to_string(),
            synchronization: Some(SynchronizationPolicy {
                mode: SynchronizationMode::LatestOnly as i32,
                window_seconds: None,
            }),
            causal_retention: Some(CausalRetentionPolicy {
                mode: CausalRetentionMode::CurrentState as i32,
            }),
            history_requirement: HistoryRequirement::BoundedReconstructible as i32,
            history_source: Some(HistorySourceReference {
                collection: "track-history".to_string(),
                source_id: "track-1".to_string(),
                checkpoint_collection: Some("track-checkpoints".to_string()),
                catalog_collection: Some("track-history-catalog".to_string()),
            }),
            segment_policy: Some(SegmentPolicy {
                max_age_seconds: 300,
                max_events: 1000,
                max_bytes: 0,
                max_revisions: 0,
                retention_seconds: Some(86_400),
            }),
            durability: Some(DurabilityPolicy {
                target: DurabilityTarget::Replicated as i32,
                minimum_copies: 2,
            }),
            over_budget_behavior: OverBudgetBehavior::Backpressure as i32,
            stale_writer_behavior: StaleWriterBehavior::RejectWithActiveEpoch as i32,
            resurrection_behavior: ResurrectionBehavior::Quarantine as i32,
        }
    }

    #[test]
    fn bounded_reconstructible_policy_is_valid() {
        assert!(validate_collection_history_policy(&bounded_policy()).is_ok());
    }

    #[test]
    fn reconstructible_policy_requires_history_source() {
        let mut policy = bounded_policy();
        policy.history_source = None;
        let error = validate_collection_history_policy(&policy).unwrap_err();
        assert!(error.to_string().contains("history_source"));
    }

    #[test]
    fn bounded_policy_requires_positive_retention() {
        let mut policy = bounded_policy();
        policy.segment_policy.as_mut().unwrap().retention_seconds = Some(0);
        let error = validate_collection_history_policy(&policy).unwrap_err();
        assert!(error.to_string().contains("retention_seconds"));
    }

    #[test]
    fn complete_policy_rejects_expiry() {
        let mut policy = bounded_policy();
        policy.history_requirement = HistoryRequirement::CompleteReconstructible as i32;
        let error = validate_collection_history_policy(&policy).unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot declare a retention expiry"));
    }

    #[test]
    fn complete_reconstructible_policy_without_expiry_is_valid() {
        let mut policy = bounded_policy();
        policy.history_requirement = HistoryRequirement::CompleteReconstructible as i32;
        policy.segment_policy.as_mut().unwrap().retention_seconds = None;
        assert!(validate_collection_history_policy(&policy).is_ok());
    }

    #[test]
    fn reconstructible_policy_requires_finite_rotation() {
        let mut policy = bounded_policy();
        policy.segment_policy.as_mut().unwrap().max_age_seconds = 0;
        policy.segment_policy.as_mut().unwrap().max_events = 0;
        let error = validate_collection_history_policy(&policy).unwrap_err();
        assert!(error.to_string().contains("positive rotation limit"));
    }

    #[test]
    fn unknown_values_resolve_conservatively() {
        assert_eq!(
            conservative_synchronization_mode(999),
            SynchronizationMode::FullHistory
        );
        assert_eq!(
            conservative_causal_retention_mode(999),
            CausalRetentionMode::Complete
        );
    }

    #[test]
    fn retention_eligibility_requires_acknowledged_durability() {
        let status = HistorySegmentStatus {
            segment: Some(HistorySegmentDescriptor {
                collection: "track-history".to_string(),
                source_id: "track-1".to_string(),
                epoch_id: "epoch-1".to_string(),
                segment_id: "segment-1".to_string(),
                sequence_start: 0,
                sequence_end: 99,
                starts_at: None,
                ends_at: None,
                predecessor_segment_id: None,
                content_sha256: Some(vec![0x5a; 32]),
                checkpoint_id: None,
                content_encoding: Some("application/vnd.peat.history-segment.v1".to_string()),
            }),
            lifecycle: SegmentLifecycle::RetentionEligible as i32,
            durability: Some(DurabilityProgress {
                target: DurabilityTarget::Replicated as i32,
                required_copies: 2,
                acknowledged_copies: 1,
                target_met: false,
            }),
            retention_eligible: true,
            evaluated_at: Some(Timestamp {
                seconds: 200,
                nanos: 0,
            }),
            retention_not_before: Some(Timestamp {
                seconds: 100,
                nanos: 0,
            }),
            detail: None,
        };
        assert!(validate_history_segment_status(&status).is_err());
    }
}
