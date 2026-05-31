//! Delta-based document updates for hierarchical aggregation
//!
//! This module implements field-level delta updates to replace full document
//! recreation, addressing the 20× bandwidth amplification issue identified in
//! ADR-021 E12 validation.
//!
//! # Core Principle
//!
//! Documents are created ONCE, then updated via deltas containing only changed
//! fields. This enables:
//! - CRDT delta propagation (not full document replication)
//! - 10-20× bandwidth reduction
//! - Proper document lifecycle (create-once, update-many pattern)

use peat_schema::capability::v1::Capability;
use peat_schema::common::v1::{Position, Timestamp};
use peat_schema::hierarchy::v1::BoundingBox;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// Field-level delta for CellSummary documents
///
/// Represents incremental changes to a cell summary, enabling CRDT-based
/// delta synchronization instead of full document recreation.
///
/// # Example
///
/// ```rust,no_run
/// use peat_protocol::hierarchy::deltas::*;
///
/// let delta = CellDelta {
///     cell_id: "cell-1A".to_string(),
///     timestamp_us: current_timestamp_us(),
///     sequence: 42,
///     updates: vec![
///         CellFieldUpdate::SetMemberCount(7),
///         CellFieldUpdate::SetOperationalCount(6),
///         CellFieldUpdate::AddMemberId("node-8".to_string()),
///     ],
/// };
///
/// // Delta is ~100 bytes vs ~2KB for full CellSummary
/// assert!(delta.size_bytes() < 200);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellDelta {
    /// Cell identifier
    pub cell_id: String,

    /// Timestamp when delta was generated (microseconds since epoch)
    pub timestamp_us: u64,

    /// Monotonic sequence number for ordering
    pub sequence: u64,

    /// Field-level updates
    pub updates: Vec<CellFieldUpdate>,
}

/// Individual field update for CellSummary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CellFieldUpdate {
    // Scalar fields (LWW - Last Write Wins semantics)
    /// Update cell leader ID
    SetLeaderId(String),

    /// Update total member count
    SetMemberCount(u32),

    /// Update operational member count (health >= DEGRADED)
    SetOperationalCount(u32),

    /// Update average fuel across cell members (minutes)
    SetAvgFuelMinutes(f32),

    /// Update worst health status in cell
    SetWorstHealth(i32),

    /// Update cell readiness score (0.0-1.0)
    SetReadinessScore(f32),

    // Position update (LWW for centroid)
    /// Update position centroid
    UpdatePositionCentroid(Position),

    // Array operations (OR-Set semantics - Add-Wins)
    /// Add member to cell
    AddMemberId(String),

    /// Remove member from cell
    RemoveMemberId(String),

    // Capability composition (additive)
    /// Add aggregated capability
    AddCapability(Capability),

    /// Remove capability by ID
    RemoveCapability(String),

    // Spatial updates
    /// Update bounding box
    UpdateBoundingBox(BoundingBox),

    /// Update aggregation timestamp
    UpdateAggregatedAt(Timestamp),
}

impl CellDelta {
    /// Convert delta to Ditto field update operations
    ///
    /// Maps delta field updates to JSON field paths and values for Ditto's
    /// CRDT update operations.
    ///
    /// # Returns
    ///
    /// Vector of (field_path, value) tuples for Ditto update operations
    pub fn into_ditto_updates(self) -> Vec<(String, serde_json::Value)> {
        let mut updates = Vec::new();

        for update in self.updates {
            match update {
                CellFieldUpdate::SetLeaderId(id) => {
                    updates.push(("leader_id".to_string(), json!(id)));
                }
                CellFieldUpdate::SetMemberCount(count) => {
                    updates.push(("member_count".to_string(), json!(count)));
                }
                CellFieldUpdate::SetOperationalCount(count) => {
                    updates.push(("operational_count".to_string(), json!(count)));
                }
                CellFieldUpdate::SetAvgFuelMinutes(fuel) => {
                    updates.push(("avg_fuel_minutes".to_string(), json!(fuel)));
                }
                CellFieldUpdate::SetWorstHealth(health) => {
                    updates.push(("worst_health".to_string(), json!(health)));
                }
                CellFieldUpdate::SetReadinessScore(score) => {
                    updates.push(("readiness_score".to_string(), json!(score)));
                }
                CellFieldUpdate::UpdatePositionCentroid(pos) => {
                    updates.push((
                        "position_centroid".to_string(),
                        serde_json::to_value(pos).unwrap_or(json!(null)),
                    ));
                }
                CellFieldUpdate::AddMemberId(id) => {
                    // OR-Set: add to array
                    updates.push(("member_ids.$add".to_string(), json!(id)));
                }
                CellFieldUpdate::RemoveMemberId(id) => {
                    // OR-Set: remove from array
                    updates.push(("member_ids.$remove".to_string(), json!(id)));
                }
                CellFieldUpdate::AddCapability(cap) => {
                    updates.push((
                        "aggregated_capabilities.$add".to_string(),
                        serde_json::to_value(cap).unwrap_or(json!(null)),
                    ));
                }
                CellFieldUpdate::RemoveCapability(cap_id) => {
                    updates.push(("aggregated_capabilities.$remove".to_string(), json!(cap_id)));
                }
                CellFieldUpdate::UpdateBoundingBox(bbox) => {
                    updates.push((
                        "bounding_box".to_string(),
                        serde_json::to_value(bbox).unwrap_or(json!(null)),
                    ));
                }
                CellFieldUpdate::UpdateAggregatedAt(ts) => {
                    updates.push((
                        "aggregated_at".to_string(),
                        serde_json::to_value(ts).unwrap_or(json!(null)),
                    ));
                }
            }
        }

        // Add metadata updates
        updates.push(("last_update_us".to_string(), json!(self.timestamp_us)));
        updates.push(("sequence".to_string(), json!(self.sequence)));

        updates
    }

    /// Estimate size of delta in bytes
    ///
    /// Used for bandwidth metrics and efficiency validation.
    /// Target: delta should be <5% of full CellSummary size (~2KB).
    pub fn size_bytes(&self) -> usize {
        // Rough estimate based on field updates
        let base_overhead = 64; // cell_id, timestamp_us, sequence
        let per_update_overhead = 16; // field name + metadata

        let updates_size: usize = self
            .updates
            .iter()
            .map(|u| match u {
                CellFieldUpdate::SetLeaderId(s) => s.len() + per_update_overhead,
                CellFieldUpdate::SetMemberCount(_) => 4 + per_update_overhead,
                CellFieldUpdate::SetOperationalCount(_) => 4 + per_update_overhead,
                CellFieldUpdate::SetAvgFuelMinutes(_) => 4 + per_update_overhead,
                CellFieldUpdate::SetWorstHealth(_) => 4 + per_update_overhead,
                CellFieldUpdate::SetReadinessScore(_) => 4 + per_update_overhead,
                CellFieldUpdate::UpdatePositionCentroid(_) => 24 + per_update_overhead, // 3 floats
                CellFieldUpdate::AddMemberId(s) => s.len() + per_update_overhead,
                CellFieldUpdate::RemoveMemberId(s) => s.len() + per_update_overhead,
                CellFieldUpdate::AddCapability(_) => 128 + per_update_overhead, // capability ~128 bytes
                CellFieldUpdate::RemoveCapability(s) => s.len() + per_update_overhead,
                CellFieldUpdate::UpdateBoundingBox(_) => 64 + per_update_overhead,
                CellFieldUpdate::UpdateAggregatedAt(_) => 16 + per_update_overhead,
            })
            .sum();

        base_overhead + updates_size
    }

    /// Check if delta is empty (no updates)
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }
}

/// Field-level delta for CohortSummary documents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CohortDelta {
    /// Cohort identifier
    pub cohort_id: String,

    /// Timestamp when delta was generated (microseconds since epoch)
    pub timestamp_us: u64,

    /// Monotonic sequence number for ordering
    pub sequence: u64,

    /// Field-level updates
    pub updates: Vec<CohortFieldUpdate>,
}

/// Individual field update for CohortSummary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CohortFieldUpdate {
    // Scalar fields (LWW semantics)
    SetLeaderId(String),
    SetCellCount(u32),
    SetTotalMemberCount(u32),
    SetOperationalCount(u32),
    SetAvgFuelMinutes(f32),
    SetWorstHealth(i32),
    SetReadinessScore(f32),

    // Position update
    UpdatePositionCentroid(Position),

    // Array operations (OR-Set)
    AddCellId(String),
    RemoveCellId(String),

    // Capabilities
    AddCapability(Capability),
    RemoveCapability(String),

    // Spatial
    UpdateBoundingBox(BoundingBox),
    UpdateAggregatedAt(Timestamp),
}

impl CohortDelta {
    /// Convert delta to Ditto field update operations
    pub fn into_ditto_updates(self) -> Vec<(String, serde_json::Value)> {
        let mut updates = Vec::new();

        for update in self.updates {
            match update {
                CohortFieldUpdate::SetLeaderId(id) => {
                    updates.push(("leader_id".to_string(), json!(id)));
                }
                CohortFieldUpdate::SetCellCount(count) => {
                    updates.push(("cell_count".to_string(), json!(count)));
                }
                CohortFieldUpdate::SetTotalMemberCount(count) => {
                    updates.push(("total_member_count".to_string(), json!(count)));
                }
                CohortFieldUpdate::SetOperationalCount(count) => {
                    updates.push(("operational_count".to_string(), json!(count)));
                }
                CohortFieldUpdate::SetAvgFuelMinutes(fuel) => {
                    updates.push(("avg_fuel_minutes".to_string(), json!(fuel)));
                }
                CohortFieldUpdate::SetWorstHealth(health) => {
                    updates.push(("worst_health".to_string(), json!(health)));
                }
                CohortFieldUpdate::SetReadinessScore(score) => {
                    updates.push(("readiness_score".to_string(), json!(score)));
                }
                CohortFieldUpdate::UpdatePositionCentroid(pos) => {
                    updates.push((
                        "position_centroid".to_string(),
                        serde_json::to_value(pos).unwrap_or(json!(null)),
                    ));
                }
                CohortFieldUpdate::AddCellId(id) => {
                    updates.push(("cell_ids.$add".to_string(), json!(id)));
                }
                CohortFieldUpdate::RemoveCellId(id) => {
                    updates.push(("cell_ids.$remove".to_string(), json!(id)));
                }
                CohortFieldUpdate::AddCapability(cap) => {
                    updates.push((
                        "aggregated_capabilities.$add".to_string(),
                        serde_json::to_value(cap).unwrap_or(json!(null)),
                    ));
                }
                CohortFieldUpdate::RemoveCapability(cap_id) => {
                    updates.push(("aggregated_capabilities.$remove".to_string(), json!(cap_id)));
                }
                CohortFieldUpdate::UpdateBoundingBox(bbox) => {
                    updates.push((
                        "bounding_box".to_string(),
                        serde_json::to_value(bbox).unwrap_or(json!(null)),
                    ));
                }
                CohortFieldUpdate::UpdateAggregatedAt(ts) => {
                    updates.push((
                        "aggregated_at".to_string(),
                        serde_json::to_value(ts).unwrap_or(json!(null)),
                    ));
                }
            }
        }

        updates.push(("last_update_us".to_string(), json!(self.timestamp_us)));
        updates.push(("sequence".to_string(), json!(self.sequence)));

        updates
    }

    /// Estimate size of delta in bytes
    pub fn size_bytes(&self) -> usize {
        let base_overhead = 64;
        let per_update_overhead = 16;

        let updates_size: usize = self
            .updates
            .iter()
            .map(|u| match u {
                CohortFieldUpdate::SetLeaderId(s) => s.len() + per_update_overhead,
                CohortFieldUpdate::SetCellCount(_) => 4 + per_update_overhead,
                CohortFieldUpdate::SetTotalMemberCount(_) => 4 + per_update_overhead,
                CohortFieldUpdate::SetOperationalCount(_) => 4 + per_update_overhead,
                CohortFieldUpdate::SetAvgFuelMinutes(_) => 4 + per_update_overhead,
                CohortFieldUpdate::SetWorstHealth(_) => 4 + per_update_overhead,
                CohortFieldUpdate::SetReadinessScore(_) => 4 + per_update_overhead,
                CohortFieldUpdate::UpdatePositionCentroid(_) => 24 + per_update_overhead,
                CohortFieldUpdate::AddCellId(s) => s.len() + per_update_overhead,
                CohortFieldUpdate::RemoveCellId(s) => s.len() + per_update_overhead,
                CohortFieldUpdate::AddCapability(_) => 128 + per_update_overhead,
                CohortFieldUpdate::RemoveCapability(s) => s.len() + per_update_overhead,
                CohortFieldUpdate::UpdateBoundingBox(_) => 64 + per_update_overhead,
                CohortFieldUpdate::UpdateAggregatedAt(_) => 16 + per_update_overhead,
            })
            .sum();

        base_overhead + updates_size
    }

    /// Check if delta is empty
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }
}

/// Field-level delta for FederationSummary documents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationDelta {
    /// Federation identifier
    pub federation_id: String,

    /// Timestamp when delta was generated (microseconds since epoch)
    pub timestamp_us: u64,

    /// Monotonic sequence number for ordering
    pub sequence: u64,

    /// Field-level updates
    pub updates: Vec<FederationFieldUpdate>,
}

/// Individual field update for FederationSummary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FederationFieldUpdate {
    // Scalar fields (LWW semantics)
    SetLeaderId(String),
    SetCohortCount(u32),
    SetTotalMemberCount(u32),
    SetOperationalCount(u32),
    SetAvgFuelMinutes(f32),
    SetWorstHealth(i32),
    SetReadinessScore(f32),

    // Position update
    UpdatePositionCentroid(Position),

    // Array operations (OR-Set)
    AddCohortId(String),
    RemoveCohortId(String),

    // Capabilities
    AddCapability(Capability),
    RemoveCapability(String),

    // Spatial
    UpdateBoundingBox(BoundingBox),
    UpdateAggregatedAt(Timestamp),
}

impl FederationDelta {
    /// Convert delta to Ditto field update operations
    pub fn into_ditto_updates(self) -> Vec<(String, serde_json::Value)> {
        let mut updates = Vec::new();

        for update in self.updates {
            match update {
                FederationFieldUpdate::SetLeaderId(id) => {
                    updates.push(("leader_id".to_string(), json!(id)));
                }
                FederationFieldUpdate::SetCohortCount(count) => {
                    updates.push(("cohort_count".to_string(), json!(count)));
                }
                FederationFieldUpdate::SetTotalMemberCount(count) => {
                    updates.push(("total_member_count".to_string(), json!(count)));
                }
                FederationFieldUpdate::SetOperationalCount(count) => {
                    updates.push(("operational_count".to_string(), json!(count)));
                }
                FederationFieldUpdate::SetAvgFuelMinutes(fuel) => {
                    updates.push(("avg_fuel_minutes".to_string(), json!(fuel)));
                }
                FederationFieldUpdate::SetWorstHealth(health) => {
                    updates.push(("worst_health".to_string(), json!(health)));
                }
                FederationFieldUpdate::SetReadinessScore(score) => {
                    updates.push(("readiness_score".to_string(), json!(score)));
                }
                FederationFieldUpdate::UpdatePositionCentroid(pos) => {
                    updates.push((
                        "position_centroid".to_string(),
                        serde_json::to_value(pos).unwrap_or(json!(null)),
                    ));
                }
                FederationFieldUpdate::AddCohortId(id) => {
                    updates.push(("cohort_ids.$add".to_string(), json!(id)));
                }
                FederationFieldUpdate::RemoveCohortId(id) => {
                    updates.push(("cohort_ids.$remove".to_string(), json!(id)));
                }
                FederationFieldUpdate::AddCapability(cap) => {
                    updates.push((
                        "aggregated_capabilities.$add".to_string(),
                        serde_json::to_value(cap).unwrap_or(json!(null)),
                    ));
                }
                FederationFieldUpdate::RemoveCapability(cap_id) => {
                    updates.push(("aggregated_capabilities.$remove".to_string(), json!(cap_id)));
                }
                FederationFieldUpdate::UpdateBoundingBox(bbox) => {
                    updates.push((
                        "bounding_box".to_string(),
                        serde_json::to_value(bbox).unwrap_or(json!(null)),
                    ));
                }
                FederationFieldUpdate::UpdateAggregatedAt(ts) => {
                    updates.push((
                        "aggregated_at".to_string(),
                        serde_json::to_value(ts).unwrap_or(json!(null)),
                    ));
                }
            }
        }

        updates.push(("last_update_us".to_string(), json!(self.timestamp_us)));
        updates.push(("sequence".to_string(), json!(self.sequence)));

        updates
    }

    /// Estimate size of delta in bytes
    pub fn size_bytes(&self) -> usize {
        let base_overhead = 64;
        let per_update_overhead = 16;

        let updates_size: usize = self
            .updates
            .iter()
            .map(|u| match u {
                FederationFieldUpdate::SetLeaderId(s) => s.len() + per_update_overhead,
                FederationFieldUpdate::SetCohortCount(_) => 4 + per_update_overhead,
                FederationFieldUpdate::SetTotalMemberCount(_) => 4 + per_update_overhead,
                FederationFieldUpdate::SetOperationalCount(_) => 4 + per_update_overhead,
                FederationFieldUpdate::SetAvgFuelMinutes(_) => 4 + per_update_overhead,
                FederationFieldUpdate::SetWorstHealth(_) => 4 + per_update_overhead,
                FederationFieldUpdate::SetReadinessScore(_) => 4 + per_update_overhead,
                FederationFieldUpdate::UpdatePositionCentroid(_) => 24 + per_update_overhead,
                FederationFieldUpdate::AddCohortId(s) => s.len() + per_update_overhead,
                FederationFieldUpdate::RemoveCohortId(s) => s.len() + per_update_overhead,
                FederationFieldUpdate::AddCapability(_) => 128 + per_update_overhead,
                FederationFieldUpdate::RemoveCapability(s) => s.len() + per_update_overhead,
                FederationFieldUpdate::UpdateBoundingBox(_) => 64 + per_update_overhead,
                FederationFieldUpdate::UpdateAggregatedAt(_) => 16 + per_update_overhead,
            })
            .sum();

        base_overhead + updates_size
    }

    /// Check if delta is empty
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }
}

/// Field-level delta for CoalitionSummary documents
///
/// Coalition is the top-tier aggregation under the four-tier rigid-schema
/// model defined by ADR-066. A coalition is an alliance of federations for
/// combined action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoalitionDelta {
    /// Coalition identifier
    pub coalition_id: String,

    /// Timestamp when delta was generated (microseconds since epoch)
    pub timestamp_us: u64,

    /// Monotonic sequence number for ordering
    pub sequence: u64,

    /// Field-level updates
    pub updates: Vec<CoalitionFieldUpdate>,
}

/// Individual field update for CoalitionSummary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CoalitionFieldUpdate {
    // Scalar fields (LWW semantics)
    SetLeaderId(String),
    SetFederationCount(u32),
    SetTotalMemberCount(u32),
    SetOperationalCount(u32),
    SetAvgFuelMinutes(f32),
    SetWorstHealth(i32),
    SetReadinessScore(f32),

    // Position update
    UpdatePositionCentroid(Position),

    // Array operations (OR-Set)
    AddFederationId(String),
    RemoveFederationId(String),

    // Capabilities
    AddCapability(Capability),
    RemoveCapability(String),

    // Spatial
    UpdateBoundingBox(BoundingBox),
    UpdateAggregatedAt(Timestamp),
}

impl CoalitionDelta {
    /// Convert delta to Ditto field update operations
    pub fn into_ditto_updates(self) -> Vec<(String, serde_json::Value)> {
        let mut updates = Vec::new();

        for update in self.updates {
            match update {
                CoalitionFieldUpdate::SetLeaderId(id) => {
                    updates.push(("leader_id".to_string(), json!(id)));
                }
                CoalitionFieldUpdate::SetFederationCount(count) => {
                    updates.push(("federation_count".to_string(), json!(count)));
                }
                CoalitionFieldUpdate::SetTotalMemberCount(count) => {
                    updates.push(("total_member_count".to_string(), json!(count)));
                }
                CoalitionFieldUpdate::SetOperationalCount(count) => {
                    updates.push(("operational_count".to_string(), json!(count)));
                }
                CoalitionFieldUpdate::SetAvgFuelMinutes(fuel) => {
                    updates.push(("avg_fuel_minutes".to_string(), json!(fuel)));
                }
                CoalitionFieldUpdate::SetWorstHealth(health) => {
                    updates.push(("worst_health".to_string(), json!(health)));
                }
                CoalitionFieldUpdate::SetReadinessScore(score) => {
                    updates.push(("readiness_score".to_string(), json!(score)));
                }
                CoalitionFieldUpdate::UpdatePositionCentroid(pos) => {
                    updates.push((
                        "position_centroid".to_string(),
                        serde_json::to_value(pos).unwrap_or(json!(null)),
                    ));
                }
                CoalitionFieldUpdate::AddFederationId(id) => {
                    updates.push(("federation_ids.$add".to_string(), json!(id)));
                }
                CoalitionFieldUpdate::RemoveFederationId(id) => {
                    updates.push(("federation_ids.$remove".to_string(), json!(id)));
                }
                CoalitionFieldUpdate::AddCapability(cap) => {
                    updates.push((
                        "aggregated_capabilities.$add".to_string(),
                        serde_json::to_value(cap).unwrap_or(json!(null)),
                    ));
                }
                CoalitionFieldUpdate::RemoveCapability(cap_id) => {
                    updates.push(("aggregated_capabilities.$remove".to_string(), json!(cap_id)));
                }
                CoalitionFieldUpdate::UpdateBoundingBox(bbox) => {
                    updates.push((
                        "bounding_box".to_string(),
                        serde_json::to_value(bbox).unwrap_or(json!(null)),
                    ));
                }
                CoalitionFieldUpdate::UpdateAggregatedAt(ts) => {
                    updates.push((
                        "aggregated_at".to_string(),
                        serde_json::to_value(ts).unwrap_or(json!(null)),
                    ));
                }
            }
        }

        updates.push(("last_update_us".to_string(), json!(self.timestamp_us)));
        updates.push(("sequence".to_string(), json!(self.sequence)));

        updates
    }

    /// Estimate size of delta in bytes
    pub fn size_bytes(&self) -> usize {
        let base_overhead = 64;
        let per_update_overhead = 16;

        let updates_size: usize = self
            .updates
            .iter()
            .map(|u| match u {
                CoalitionFieldUpdate::SetLeaderId(s) => s.len() + per_update_overhead,
                CoalitionFieldUpdate::SetFederationCount(_) => 4 + per_update_overhead,
                CoalitionFieldUpdate::SetTotalMemberCount(_) => 4 + per_update_overhead,
                CoalitionFieldUpdate::SetOperationalCount(_) => 4 + per_update_overhead,
                CoalitionFieldUpdate::SetAvgFuelMinutes(_) => 4 + per_update_overhead,
                CoalitionFieldUpdate::SetWorstHealth(_) => 4 + per_update_overhead,
                CoalitionFieldUpdate::SetReadinessScore(_) => 4 + per_update_overhead,
                CoalitionFieldUpdate::UpdatePositionCentroid(_) => 24 + per_update_overhead,
                CoalitionFieldUpdate::AddFederationId(s) => s.len() + per_update_overhead,
                CoalitionFieldUpdate::RemoveFederationId(s) => s.len() + per_update_overhead,
                CoalitionFieldUpdate::AddCapability(_) => 128 + per_update_overhead,
                CoalitionFieldUpdate::RemoveCapability(s) => s.len() + per_update_overhead,
                CoalitionFieldUpdate::UpdateBoundingBox(_) => 64 + per_update_overhead,
                CoalitionFieldUpdate::UpdateAggregatedAt(_) => 16 + per_update_overhead,
            })
            .sum();

        base_overhead + updates_size
    }

    /// Check if delta is empty
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }
}

/// Get current timestamp in microseconds since Unix epoch
pub fn current_timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}

// ============================================================================
// Delta Generation Helpers
// ============================================================================

use peat_schema::hierarchy::v1::{CellSummary, CoalitionSummary, CohortSummary, FederationSummary};

impl CellDelta {
    /// Create a delta from a CellSummary
    ///
    /// Generates field updates representing the complete state of the summary.
    /// This is used for both:
    /// - Initial creation (all fields as updates)
    /// - Subsequent updates (Ditto will only propagate changed fields)
    ///
    /// The bandwidth savings come from Ditto's CRDT delta propagation, which only
    /// sends changed fields across the network even if we send all field updates.
    #[allow(clippy::vec_init_then_push, clippy::clone_on_copy)]
    pub fn from_summary(summary: &CellSummary, sequence: u64) -> Self {
        let mut updates: Vec<CellFieldUpdate> = Vec::new();

        // Scalar fields (LWW semantics)
        updates.push(CellFieldUpdate::SetLeaderId(summary.leader_id.clone()));
        updates.push(CellFieldUpdate::SetMemberCount(summary.member_count));
        updates.push(CellFieldUpdate::SetOperationalCount(
            summary.operational_count,
        ));
        updates.push(CellFieldUpdate::SetAvgFuelMinutes(summary.avg_fuel_minutes));
        updates.push(CellFieldUpdate::SetWorstHealth(summary.worst_health));
        updates.push(CellFieldUpdate::SetReadinessScore(summary.readiness_score));

        // Position centroid
        if let Some(pos) = &summary.position_centroid {
            updates.push(CellFieldUpdate::UpdatePositionCentroid(pos.clone()));
        }

        // Member IDs (OR-Set semantics - send current set)
        for member_id in &summary.member_ids {
            updates.push(CellFieldUpdate::AddMemberId(member_id.clone()));
        }

        // Aggregated capabilities
        for capability in &summary.aggregated_capabilities {
            updates.push(CellFieldUpdate::AddCapability(capability.clone()));
        }

        // Bounding box
        if let Some(bbox) = &summary.bounding_box {
            updates.push(CellFieldUpdate::UpdateBoundingBox(bbox.clone()));
        }

        // Aggregated timestamp
        if let Some(ts) = &summary.aggregated_at {
            updates.push(CellFieldUpdate::UpdateAggregatedAt(ts.clone()));
        }

        Self {
            cell_id: summary.cell_id.clone(),
            timestamp_us: current_timestamp_us(),
            sequence,
            updates,
        }
    }
}

impl CohortDelta {
    /// Create a delta from a CohortSummary
    ///
    /// Similar to CellDelta::from_summary, generates field updates representing
    /// the complete state. Ditto handles delta compression during CRDT sync.
    #[allow(clippy::vec_init_then_push, clippy::clone_on_copy)]
    pub fn from_summary(summary: &CohortSummary, sequence: u64) -> Self {
        let mut updates: Vec<CohortFieldUpdate> = Vec::new();

        // Scalar fields
        updates.push(CohortFieldUpdate::SetLeaderId(summary.leader_id.clone()));
        updates.push(CohortFieldUpdate::SetCellCount(summary.cell_count));
        updates.push(CohortFieldUpdate::SetTotalMemberCount(
            summary.total_member_count,
        ));
        updates.push(CohortFieldUpdate::SetOperationalCount(
            summary.operational_count,
        ));
        updates.push(CohortFieldUpdate::SetAvgFuelMinutes(
            summary.avg_fuel_minutes,
        ));
        updates.push(CohortFieldUpdate::SetWorstHealth(summary.worst_health));
        updates.push(CohortFieldUpdate::SetReadinessScore(
            summary.readiness_score,
        ));

        // Position centroid
        if let Some(pos) = &summary.position_centroid {
            updates.push(CohortFieldUpdate::UpdatePositionCentroid(pos.clone()));
        }

        // Cell IDs
        for cell_id in &summary.cell_ids {
            updates.push(CohortFieldUpdate::AddCellId(cell_id.clone()));
        }

        // Aggregated capabilities
        for capability in &summary.aggregated_capabilities {
            updates.push(CohortFieldUpdate::AddCapability(capability.clone()));
        }

        // Bounding box
        if let Some(bbox) = &summary.bounding_box {
            updates.push(CohortFieldUpdate::UpdateBoundingBox(bbox.clone()));
        }

        // Aggregated timestamp
        if let Some(ts) = &summary.aggregated_at {
            updates.push(CohortFieldUpdate::UpdateAggregatedAt(ts.clone()));
        }

        Self {
            cohort_id: summary.cohort_id.clone(),
            timestamp_us: current_timestamp_us(),
            sequence,
            updates,
        }
    }
}

impl FederationDelta {
    /// Create a delta from a FederationSummary
    ///
    /// Similar to Cell/Cohort delta generation, represents complete state
    /// while relying on Ditto's CRDT delta compression for bandwidth efficiency.
    #[allow(clippy::vec_init_then_push, clippy::clone_on_copy)]
    pub fn from_summary(summary: &FederationSummary, sequence: u64) -> Self {
        let mut updates: Vec<FederationFieldUpdate> = Vec::new();

        // Scalar fields
        updates.push(FederationFieldUpdate::SetLeaderId(
            summary.leader_id.clone(),
        ));
        updates.push(FederationFieldUpdate::SetCohortCount(summary.cohort_count));
        updates.push(FederationFieldUpdate::SetTotalMemberCount(
            summary.total_member_count,
        ));
        updates.push(FederationFieldUpdate::SetOperationalCount(
            summary.operational_count,
        ));
        updates.push(FederationFieldUpdate::SetAvgFuelMinutes(
            summary.avg_fuel_minutes,
        ));
        updates.push(FederationFieldUpdate::SetWorstHealth(summary.worst_health));
        updates.push(FederationFieldUpdate::SetReadinessScore(
            summary.readiness_score,
        ));

        // Position centroid
        if let Some(pos) = &summary.position_centroid {
            updates.push(FederationFieldUpdate::UpdatePositionCentroid(pos.clone()));
        }

        // Cohort IDs
        for cohort_id in &summary.cohort_ids {
            updates.push(FederationFieldUpdate::AddCohortId(cohort_id.clone()));
        }

        // Aggregated capabilities
        for capability in &summary.aggregated_capabilities {
            updates.push(FederationFieldUpdate::AddCapability(capability.clone()));
        }

        // Bounding box
        if let Some(bbox) = &summary.bounding_box {
            updates.push(FederationFieldUpdate::UpdateBoundingBox(bbox.clone()));
        }

        // Aggregated timestamp
        if let Some(ts) = &summary.aggregated_at {
            updates.push(FederationFieldUpdate::UpdateAggregatedAt(ts.clone()));
        }

        Self {
            federation_id: summary.federation_id.clone(),
            timestamp_us: current_timestamp_us(),
            sequence,
            updates,
        }
    }
}

impl CoalitionDelta {
    /// Create a delta from a CoalitionSummary
    ///
    /// Mirrors `FederationDelta::from_summary` for the top tier introduced by
    /// ADR-066. A coalition aggregates over federations.
    #[allow(clippy::vec_init_then_push, clippy::clone_on_copy)]
    pub fn from_summary(summary: &CoalitionSummary, sequence: u64) -> Self {
        let mut updates: Vec<CoalitionFieldUpdate> = Vec::new();

        // Scalar fields
        updates.push(CoalitionFieldUpdate::SetLeaderId(summary.leader_id.clone()));
        updates.push(CoalitionFieldUpdate::SetFederationCount(
            summary.federation_count,
        ));
        updates.push(CoalitionFieldUpdate::SetTotalMemberCount(
            summary.total_member_count,
        ));
        updates.push(CoalitionFieldUpdate::SetOperationalCount(
            summary.operational_count,
        ));
        updates.push(CoalitionFieldUpdate::SetAvgFuelMinutes(
            summary.avg_fuel_minutes,
        ));
        updates.push(CoalitionFieldUpdate::SetWorstHealth(summary.worst_health));
        updates.push(CoalitionFieldUpdate::SetReadinessScore(
            summary.readiness_score,
        ));

        // Position centroid
        if let Some(pos) = &summary.position_centroid {
            updates.push(CoalitionFieldUpdate::UpdatePositionCentroid(pos.clone()));
        }

        // Federation IDs
        for federation_id in &summary.federation_ids {
            updates.push(CoalitionFieldUpdate::AddFederationId(federation_id.clone()));
        }

        // Aggregated capabilities
        for capability in &summary.aggregated_capabilities {
            updates.push(CoalitionFieldUpdate::AddCapability(capability.clone()));
        }

        // Bounding box
        if let Some(bbox) = &summary.bounding_box {
            updates.push(CoalitionFieldUpdate::UpdateBoundingBox(bbox.clone()));
        }

        // Aggregated timestamp
        if let Some(ts) = &summary.aggregated_at {
            updates.push(CoalitionFieldUpdate::UpdateAggregatedAt(ts.clone()));
        }

        Self {
            coalition_id: summary.coalition_id.clone(),
            timestamp_us: current_timestamp_us(),
            sequence,
            updates,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_delta_serialization() {
        let delta = CellDelta {
            cell_id: "cell-1A".to_string(),
            timestamp_us: 1234567890,
            sequence: 42,
            updates: vec![
                CellFieldUpdate::SetMemberCount(7),
                CellFieldUpdate::SetOperationalCount(6),
                CellFieldUpdate::AddMemberId("node-8".to_string()),
            ],
        };

        // Should serialize to JSON
        let json = serde_json::to_string(&delta).unwrap();
        assert!(json.contains("cell-1A"));
        assert!(json.contains("SetMemberCount"));

        // Should deserialize back
        let deserialized: CellDelta = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.cell_id, "cell-1A");
        assert_eq!(deserialized.updates.len(), 3);
    }

    #[test]
    fn test_cell_delta_into_ditto_updates() {
        let delta = CellDelta {
            cell_id: "cell-1A".to_string(),
            timestamp_us: 1234567890,
            sequence: 42,
            updates: vec![
                CellFieldUpdate::SetLeaderId("leader-1".to_string()),
                CellFieldUpdate::SetMemberCount(8),
                CellFieldUpdate::AddMemberId("node-9".to_string()),
            ],
        };

        let ditto_updates = delta.into_ditto_updates();

        // Should have field updates + metadata
        assert!(ditto_updates.len() >= 3);

        // Check specific updates
        let leader_update = ditto_updates
            .iter()
            .find(|(path, _)| path == "leader_id")
            .unwrap();
        assert_eq!(leader_update.1, json!("leader-1"));

        let member_count_update = ditto_updates
            .iter()
            .find(|(path, _)| path == "member_count")
            .unwrap();
        assert_eq!(member_count_update.1, json!(8));

        let add_member_update = ditto_updates
            .iter()
            .find(|(path, _)| path == "member_ids.$add")
            .unwrap();
        assert_eq!(add_member_update.1, json!("node-9"));
    }

    #[test]
    fn test_delta_size_estimation() {
        let small_delta = CellDelta {
            cell_id: "cell-1A".to_string(),
            timestamp_us: 1234567890,
            sequence: 1,
            updates: vec![CellFieldUpdate::SetMemberCount(7)],
        };

        let large_delta = CellDelta {
            cell_id: "cell-1A".to_string(),
            timestamp_us: 1234567890,
            sequence: 1,
            updates: vec![
                CellFieldUpdate::SetMemberCount(7),
                CellFieldUpdate::SetOperationalCount(6),
                CellFieldUpdate::AddMemberId("node-123456789".to_string()),
                CellFieldUpdate::UpdatePositionCentroid(Position {
                    latitude: 37.7749,
                    longitude: -122.4194,
                    altitude: 100.0,
                }),
            ],
        };

        // Small delta should be ~100 bytes
        assert!(small_delta.size_bytes() < 150);

        // Large delta should still be much smaller than full CellSummary (~2KB)
        assert!(large_delta.size_bytes() < 500);
        assert!(large_delta.size_bytes() > small_delta.size_bytes());
    }

    #[test]
    fn test_empty_delta() {
        let delta = CellDelta {
            cell_id: "cell-1A".to_string(),
            timestamp_us: 1234567890,
            sequence: 1,
            updates: vec![],
        };

        assert!(delta.is_empty());
        assert_eq!(delta.updates.len(), 0);
    }

    #[test]
    fn test_cohort_delta_basic() {
        let delta = CohortDelta {
            cohort_id: "cohort-1".to_string(),
            timestamp_us: 1234567890,
            sequence: 10,
            updates: vec![
                CohortFieldUpdate::SetCellCount(3),
                CohortFieldUpdate::AddCellId("cell-1A".to_string()),
            ],
        };

        let ditto_updates = delta.into_ditto_updates();
        assert!(ditto_updates.len() >= 2);

        let cell_count = ditto_updates
            .iter()
            .find(|(path, _)| path == "cell_count")
            .unwrap();
        assert_eq!(cell_count.1, json!(3));
    }

    #[test]
    fn test_federation_delta_basic() {
        let delta = FederationDelta {
            federation_id: "federation-alpha".to_string(),
            timestamp_us: 1234567890,
            sequence: 5,
            updates: vec![
                FederationFieldUpdate::SetCohortCount(4),
                FederationFieldUpdate::SetTotalMemberCount(96),
            ],
        };

        assert!(!delta.is_empty());
        assert_eq!(delta.updates.len(), 2);
    }

    #[test]
    fn test_coalition_delta_basic() {
        let delta = CoalitionDelta {
            coalition_id: "coalition-1".to_string(),
            timestamp_us: 1234567890,
            sequence: 7,
            updates: vec![
                CoalitionFieldUpdate::SetFederationCount(3),
                CoalitionFieldUpdate::AddFederationId("federation-alpha".to_string()),
                CoalitionFieldUpdate::SetTotalMemberCount(300),
            ],
        };

        assert!(!delta.is_empty());
        assert_eq!(delta.updates.len(), 3);

        let ditto_updates = delta.into_ditto_updates();
        let federation_count = ditto_updates
            .iter()
            .find(|(path, _)| path == "federation_count")
            .unwrap();
        assert_eq!(federation_count.1, json!(3));
    }

    #[test]
    fn test_current_timestamp_us() {
        let ts1 = current_timestamp_us();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let ts2 = current_timestamp_us();

        // Timestamp should increase
        assert!(ts2 > ts1);

        // Should be reasonable microseconds since epoch (after 2020)
        assert!(ts1 > 1_600_000_000_000_000);
    }
}
