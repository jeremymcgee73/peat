//! Legacy sync-mode compatibility and ADR-076 policy migration.
//!
//! `SyncMode` and `SyncModeRegistry` remain re-exported from peat-mesh for
//! compatibility. New policy declarations use [`crate::history`]. Sync mode
//! describes synchronization only; it cannot establish domain-history or
//! durability guarantees.

pub use peat_mesh::qos::sync_mode::*;

use peat_schema::history::v1::{
    CausalRetentionMode, CausalRetentionPolicy, CollectionHistoryPolicy, HistoryRequirement,
    OverBudgetBehavior, ResurrectionBehavior, StaleWriterBehavior, SynchronizationMode,
    SynchronizationPolicy,
};

/// Convert an existing mesh sync mode into a conservative ADR-076 policy.
///
/// Legacy modes do not carry a durability target or a general domain-history
/// requirement. The conversion preserves those unknowns rather than inventing
/// a stronger guarantee. `LatestOnly` does explicitly preserve current state
/// only unless a separate history source is configured later.
pub fn collection_history_policy_from_sync_mode(
    collection: impl Into<String>,
    mode: SyncMode,
) -> CollectionHistoryPolicy {
    let (synchronization, causal_retention, history_requirement) = match mode {
        SyncMode::FullHistory => (
            SynchronizationPolicy {
                mode: SynchronizationMode::FullHistory as i32,
                window_seconds: None,
            },
            CausalRetentionPolicy {
                mode: CausalRetentionMode::Complete as i32,
            },
            HistoryRequirement::Unspecified,
        ),
        SyncMode::LatestOnly => (
            SynchronizationPolicy {
                mode: SynchronizationMode::LatestOnly as i32,
                window_seconds: None,
            },
            CausalRetentionPolicy {
                mode: CausalRetentionMode::CurrentState as i32,
            },
            HistoryRequirement::CurrentStateOnly,
        ),
        SyncMode::WindowedHistory { window_seconds } => (
            SynchronizationPolicy {
                mode: SynchronizationMode::Windowed as i32,
                window_seconds: Some(window_seconds),
            },
            // The current mesh implementation limits synchronization but
            // retains the complete local Automerge graph.
            CausalRetentionPolicy {
                mode: CausalRetentionMode::Complete as i32,
            },
            HistoryRequirement::Unspecified,
        ),
    };

    CollectionHistoryPolicy {
        collection: collection.into(),
        synchronization: Some(synchronization),
        causal_retention: Some(causal_retention),
        history_requirement: history_requirement as i32,
        history_source: None,
        segment_policy: None,
        durability: None,
        over_budget_behavior: OverBudgetBehavior::Unspecified as i32,
        stale_writer_behavior: StaleWriterBehavior::Unspecified as i32,
        resurrection_behavior: ResurrectionBehavior::Unspecified as i32,
    }
}

/// Convert optional legacy configuration, using FullHistory for an undeclared
/// collection so absence never authorizes history loss.
pub fn collection_history_policy_from_optional_sync_mode(
    collection: impl Into<String>,
    mode: Option<SyncMode>,
) -> CollectionHistoryPolicy {
    collection_history_policy_from_sync_mode(collection, mode.unwrap_or(SyncMode::FullHistory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use peat_schema::validation::validate_migrated_collection_history_policy;

    #[test]
    fn full_history_migration_makes_no_domain_history_claim() {
        let policy = collection_history_policy_from_sync_mode("commands", SyncMode::FullHistory);
        assert_eq!(
            policy.synchronization.unwrap().mode,
            SynchronizationMode::FullHistory as i32
        );
        assert_eq!(
            policy.causal_retention.unwrap().mode,
            CausalRetentionMode::Complete as i32
        );
        assert_eq!(
            policy.history_requirement,
            HistoryRequirement::Unspecified as i32
        );
        assert!(policy.durability.is_none());
        assert!(validate_migrated_collection_history_policy(&policy).is_ok());
    }

    #[test]
    fn latest_only_migration_claims_current_state_only() {
        let policy = collection_history_policy_from_sync_mode("tracks", SyncMode::LatestOnly);
        assert_eq!(
            policy.synchronization.unwrap().mode,
            SynchronizationMode::LatestOnly as i32
        );
        assert_eq!(
            policy.history_requirement,
            HistoryRequirement::CurrentStateOnly as i32
        );
        assert!(validate_migrated_collection_history_policy(&policy).is_ok());
    }

    #[test]
    fn windowed_migration_does_not_claim_bounded_local_retention() {
        let policy = collection_history_policy_from_sync_mode(
            "track-history",
            SyncMode::WindowedHistory {
                window_seconds: 300,
            },
        );
        let synchronization = policy.synchronization.unwrap();
        assert_eq!(synchronization.mode, SynchronizationMode::Windowed as i32);
        assert_eq!(synchronization.window_seconds, Some(300));
        assert_eq!(
            policy.causal_retention.unwrap().mode,
            CausalRetentionMode::Complete as i32
        );
        assert!(policy.segment_policy.is_none());
        assert!(validate_migrated_collection_history_policy(&policy).is_ok());
    }

    #[test]
    fn absent_legacy_mode_defaults_to_full_history() {
        let policy = collection_history_policy_from_optional_sync_mode("custom", None);
        assert_eq!(
            policy.synchronization.unwrap().mode,
            SynchronizationMode::FullHistory as i32
        );
        assert!(validate_migrated_collection_history_policy(&policy).is_ok());
    }
}
