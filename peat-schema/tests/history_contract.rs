//! Wire compatibility and semantic coverage for the ADR-076 contract.

use peat_schema::common::v1::Timestamp;
use peat_schema::history::v1::{
    CausalRetentionMode, CausalRetentionPolicy, CollectionHistoryPolicy, DurabilityPolicy,
    DurabilityProgress, DurabilityTarget, EffectiveCollectionHistoryPolicy, EnforcementState,
    EpochLifecycle, HistoryEpochStatus, HistoryRequirement, HistorySegmentDescriptor,
    HistorySegmentStatus, HistorySourceReference, OverBudgetBehavior, PolicySource,
    ResurrectionBehavior, SegmentLifecycle, SegmentPolicy, StaleWriterBehavior,
    SynchronizationMode, SynchronizationPolicy,
};
use peat_schema::validation::{
    conservative_causal_retention_mode, conservative_synchronization_mode,
    validate_collection_history_policy, validate_history_epoch_status,
    validate_history_segment_status,
};
use prost::Message;

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
            max_bytes: 1_048_576,
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

fn current_state_policy() -> CollectionHistoryPolicy {
    CollectionHistoryPolicy {
        collection: "beacons".to_string(),
        synchronization: Some(SynchronizationPolicy {
            mode: SynchronizationMode::LatestOnly as i32,
            window_seconds: None,
        }),
        causal_retention: Some(CausalRetentionPolicy {
            mode: CausalRetentionMode::CurrentState as i32,
        }),
        history_requirement: HistoryRequirement::CurrentStateOnly as i32,
        history_source: None,
        segment_policy: None,
        durability: Some(DurabilityPolicy {
            target: DurabilityTarget::Local as i32,
            minimum_copies: 1,
        }),
        over_budget_behavior: OverBudgetBehavior::Reject as i32,
        stale_writer_behavior: StaleWriterBehavior::RejectWithActiveEpoch as i32,
        resurrection_behavior: ResurrectionBehavior::Quarantine as i32,
    }
}

fn complete_policy() -> CollectionHistoryPolicy {
    let mut policy = bounded_policy();
    policy.collection = "audit-log".to_string();
    policy.synchronization.as_mut().unwrap().mode = SynchronizationMode::FullHistory as i32;
    policy.causal_retention.as_mut().unwrap().mode = CausalRetentionMode::Complete as i32;
    policy.history_requirement = HistoryRequirement::CompleteReconstructible as i32;
    policy.segment_policy.as_mut().unwrap().retention_seconds = None;
    policy
}

#[test]
fn policy_round_trips_through_protobuf_and_json() {
    let original = bounded_policy();

    let bytes = original.encode_to_vec();
    let binary = CollectionHistoryPolicy::decode(bytes.as_slice()).unwrap();
    assert_eq!(binary, original);

    // This is the crate's implementation-local Serde representation. Protobuf
    // binary remains the canonical cross-language encoding; JSON bindings must
    // apply the standard Protobuf JSON mapping at their transport boundary.
    let json = serde_json::to_string(&original).unwrap();
    let decoded: CollectionHistoryPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, original);
    assert!(validate_collection_history_policy(&decoded).is_ok());
}

#[test]
fn every_history_requirement_round_trips_and_validates() {
    for original in [current_state_policy(), bounded_policy(), complete_policy()] {
        let bytes = original.encode_to_vec();
        let decoded = CollectionHistoryPolicy::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded, original);
        assert!(validate_collection_history_policy(&decoded).is_ok());
    }
}

#[test]
fn projection_to_history_source_reference_round_trips() {
    let original = bounded_policy().history_source.unwrap();
    let bytes = original.encode_to_vec();
    let decoded = HistorySourceReference::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn old_minimal_payload_resolves_conservatively() {
    let old = CollectionHistoryPolicy {
        collection: "legacy-custom".to_string(),
        ..Default::default()
    };
    let bytes = old.encode_to_vec();
    let decoded = CollectionHistoryPolicy::decode(bytes.as_slice()).unwrap();

    assert_eq!(
        conservative_synchronization_mode(decoded.synchronization.unwrap_or_default().mode),
        SynchronizationMode::FullHistory
    );
    assert_eq!(
        conservative_causal_retention_mode(decoded.causal_retention.unwrap_or_default().mode),
        CausalRetentionMode::Complete
    );
    assert!(validate_collection_history_policy(&decoded).is_err());
}

#[test]
fn decoder_accepts_newer_unknown_fields() {
    let original = bounded_policy();
    let mut bytes = original.encode_to_vec();

    // Unknown field 100, varint wire type, value 1. Prost accepts and drops
    // unknown fields rather than preserving them on re-encode.
    bytes.extend_from_slice(&[0xa0, 0x06, 0x01]);
    let decoded = CollectionHistoryPolicy::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn unknown_enum_numbers_survive_decode_and_fail_strict_validation() {
    let mut original = bounded_policy();
    original.history_requirement = 999;
    let bytes = original.encode_to_vec();
    let decoded = CollectionHistoryPolicy::decode(bytes.as_slice()).unwrap();

    assert_eq!(decoded.history_requirement, 999);
    let error = validate_collection_history_policy(&decoded).unwrap_err();
    assert!(error.to_string().contains("unknown enum value 999"));
}

#[test]
fn effective_policy_and_segment_status_round_trip() {
    let effective = EffectiveCollectionHistoryPolicy {
        policy: Some(bounded_policy()),
        source: PolicySource::Explicit as i32,
        enforcement_state: EnforcementState::Enforced as i32,
        detail: None,
    };
    let json = serde_json::to_string(&effective).unwrap();
    let decoded: EffectiveCollectionHistoryPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, effective);

    let segment = HistorySegmentStatus {
        segment: Some(HistorySegmentDescriptor {
            collection: "track-history".to_string(),
            source_id: "track-1".to_string(),
            epoch_id: "epoch-1".to_string(),
            segment_id: "epoch-1-segment-4".to_string(),
            sequence_start: 3000,
            sequence_end: 3999,
            starts_at: None,
            ends_at: None,
            predecessor_segment_id: Some("epoch-1-segment-3".to_string()),
            content_sha256: Some(vec![0x5a; 32]),
            checkpoint_id: Some("epoch-1-checkpoint".to_string()),
            content_encoding: Some("application/vnd.peat.history-segment.v1".to_string()),
        }),
        lifecycle: SegmentLifecycle::RetentionEligible as i32,
        durability: Some(DurabilityProgress {
            target: DurabilityTarget::Replicated as i32,
            required_copies: 2,
            acknowledged_copies: 2,
            target_met: true,
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
        detail: Some("retention interval elapsed".to_string()),
    };
    let bytes = segment.encode_to_vec();
    let decoded = HistorySegmentStatus::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, segment);
    assert!(validate_history_segment_status(&decoded).is_ok());
}

#[test]
fn local_only_durability_cannot_report_sole_copy_removed() {
    let segment = HistorySegmentStatus {
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
        lifecycle: SegmentLifecycle::Removed as i32,
        durability: Some(DurabilityProgress {
            target: DurabilityTarget::Local as i32,
            required_copies: 1,
            acknowledged_copies: 1,
            target_met: true,
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

    let error = validate_history_segment_status(&segment).unwrap_err();
    assert!(error.to_string().contains("sole durable copy"));
}

#[test]
fn current_state_policy_rejects_malformed_supplied_durability() {
    let policy = CollectionHistoryPolicy {
        collection: "beacons".to_string(),
        synchronization: Some(SynchronizationPolicy {
            mode: SynchronizationMode::LatestOnly as i32,
            window_seconds: None,
        }),
        causal_retention: Some(CausalRetentionPolicy {
            mode: CausalRetentionMode::CurrentState as i32,
        }),
        history_requirement: HistoryRequirement::CurrentStateOnly as i32,
        history_source: None,
        segment_policy: None,
        durability: Some(DurabilityPolicy {
            target: DurabilityTarget::Replicated as i32,
            minimum_copies: 1,
        }),
        over_budget_behavior: OverBudgetBehavior::Reject as i32,
        stale_writer_behavior: StaleWriterBehavior::RejectWithActiveEpoch as i32,
        resurrection_behavior: ResurrectionBehavior::Quarantine as i32,
    };

    assert!(validate_collection_history_policy(&policy).is_err());
}

#[test]
fn current_state_policy_requires_explicit_durability() {
    let mut policy = current_state_policy();
    policy.durability = None;

    let error = validate_collection_history_policy(&policy).unwrap_err();
    assert!(error.to_string().contains("durability"));
}

#[test]
fn full_history_sync_rejects_replaceable_causal_retention() {
    let mut policy = complete_policy();
    policy.causal_retention.as_mut().unwrap().mode = CausalRetentionMode::CurrentState as i32;

    let error = validate_collection_history_policy(&policy).unwrap_err();
    assert!(error.to_string().contains("enough causal history"));
}

#[test]
fn windowed_sync_requires_complete_causal_retention() {
    let mut policy = bounded_policy();
    let synchronization = policy.synchronization.as_mut().unwrap();
    synchronization.mode = SynchronizationMode::Windowed as i32;
    synchronization.window_seconds = Some(300);
    policy.causal_retention.as_mut().unwrap().mode =
        CausalRetentionMode::UntilDurableCheckpoint as i32;

    assert!(validate_collection_history_policy(&policy).is_err());
}

#[test]
fn retention_eligibility_requires_elapsed_not_before_time() {
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
            acknowledged_copies: 2,
            target_met: true,
        }),
        retention_eligible: true,
        evaluated_at: Some(Timestamp {
            seconds: 99,
            nanos: 0,
        }),
        retention_not_before: Some(Timestamp {
            seconds: 100,
            nanos: 0,
        }),
        detail: None,
    };

    let error = validate_history_segment_status(&status).unwrap_err();
    assert!(error.to_string().contains("retention_not_before"));
}

#[test]
fn active_segment_rejects_sealed_content_or_met_durability() {
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
        lifecycle: SegmentLifecycle::Active as i32,
        durability: Some(DurabilityProgress {
            target: DurabilityTarget::Replicated as i32,
            required_copies: 2,
            acknowledged_copies: 2,
            target_met: true,
        }),
        retention_eligible: false,
        evaluated_at: Some(Timestamp {
            seconds: 100,
            nanos: 0,
        }),
        retention_not_before: None,
        detail: None,
    };

    assert!(validate_history_segment_status(&status).is_err());
}

#[test]
fn closed_epoch_identifies_successor_for_stale_writer_rejection() {
    let status = HistoryEpochStatus {
        collection: "track-history".to_string(),
        source_id: "track-1".to_string(),
        epoch_id: "epoch-1".to_string(),
        lifecycle: EpochLifecycle::Closed as i32,
        successor_epoch_id: Some("epoch-2".to_string()),
        closing_checkpoint_id: Some("epoch-1-checkpoint".to_string()),
        next_segment_sequence: 4,
    };

    let bytes = status.encode_to_vec();
    let decoded = HistoryEpochStatus::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, status);
    assert!(validate_history_epoch_status(&decoded).is_ok());

    let mut self_referencing = status;
    self_referencing.successor_epoch_id = Some(self_referencing.epoch_id.clone());
    assert!(validate_history_epoch_status(&self_referencing).is_err());
}
