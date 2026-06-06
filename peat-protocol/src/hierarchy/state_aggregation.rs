//! Hierarchical State Aggregation (E12.1)
//!
//! This module implements hierarchical state aggregation across the four-tier
//! `Cell → Cohort → Federation → Coalition` model defined by ADR-066, enabling
//! O(log n) scaling instead of O(n²) full replication.
//!
//! ## Design Rationale
//!
//! **Problem**: In 24-node scenarios, full state replication requires 576 sync operations
//! (24 nodes × 24 states). At 96 nodes, this becomes 9,216 operations (O(n²) scaling).
//!
//! **Solution**: Hierarchical aggregation reduces this to O(n log n):
//! - 24 nodes publish NodeState to cell leaders (24 ops)
//! - 3 cell leaders publish CellSummary to cohort leader (3 ops)
//! - Total: 27 ops vs 576 ops (95% reduction)
//!
//! ## Integration with CapabilityAggregator
//!
//! This module handles **state aggregation** (position, fuel, health) and delegates
//! **capability aggregation** to the existing `CapabilityAggregator` in
//! `peat-protocol/src/cell/capability_aggregation.rs`.
//!
//! ## Usage
//!
//! ```ignore
//! use peat_protocol::hierarchy::StateAggregator;
//!
//! // Cell-level aggregation
//! let cell_summary = StateAggregator::aggregate_cell(
//!     "cell-1",
//!     "leader-1",
//!     member_states,
//! )?;
//!
//! // Cohort-level aggregation
//! let cohort_summary = StateAggregator::aggregate_cohort(
//!     "cohort-1",
//!     "cohort-leader",
//!     cell_summaries,
//! )?;
//! ```

use crate::cell::capability_aggregation::CapabilityAggregator;
use crate::models::{NodeConfig, NodeState, NodeStateExt};
use crate::{Error, Result};
use peat_schema::common::v1::{Position, Timestamp};
use peat_schema::hierarchy::v1::{
    BoundingBox, CellSummary, CoalitionSummary, CohortSummary, FederationSummary,
};
use peat_schema::node::v1::HealthStatus;
use std::time::SystemTime;

/// Hierarchical state aggregator
///
/// Provides functions to aggregate individual NodeState into CellSummary,
/// CellSummary into CohortSummary, CohortSummary into FederationSummary, and
/// FederationSummary into CoalitionSummary (top tier per ADR-066).
pub struct StateAggregator;

impl StateAggregator {
    /// Aggregate cell-level state from member NodeStates
    ///
    /// # Arguments
    ///
    /// * `cell_id` - Unique cell identifier
    /// * `leader_id` - Cell leader node ID
    /// * `members` - Vector of (NodeConfig, NodeState) for cell members
    ///
    /// # Returns
    ///
    /// `CellSummary` with aggregated position, health, fuel, and capabilities
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - No operational members in cell
    /// - Capability aggregation fails
    pub fn aggregate_cell(
        cell_id: &str,
        leader_id: &str,
        members: Vec<(NodeConfig, NodeState)>,
    ) -> Result<CellSummary> {
        // Filter to only operational members
        let operational: Vec<_> = members
            .into_iter()
            .filter(|(_, state)| state.is_operational())
            .collect();

        if operational.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "Cell has no operational members".to_string(),
                operation: "aggregate_cell".to_string(),
                source: None,
            });
        }

        // Extract member IDs
        let member_ids: Vec<String> = operational.iter().map(|(cfg, _)| cfg.id.clone()).collect();
        let member_count = operational.len() as u32;

        // Calculate position centroid
        let position_centroid = Self::calculate_position_centroid(&operational)?;

        // Calculate average fuel
        let avg_fuel_minutes = Self::calculate_avg_fuel(&operational);

        // Find worst health status
        let worst_health = Self::find_worst_health(&operational);

        // Count operational members (already filtered, so this is the count)
        let operational_count = member_count;

        // Aggregate capabilities using existing CapabilityAggregator
        let aggregated_capabilities_map =
            CapabilityAggregator::aggregate_capabilities(&operational)?;

        // Convert to proto capabilities
        let aggregated_capabilities = aggregated_capabilities_map
            .into_iter()
            .map(
                |(cap_type, agg_cap)| peat_schema::capability::v1::Capability {
                    id: format!("{}_{:?}", cell_id, cap_type),
                    name: format!("{:?}", cap_type),
                    capability_type: cap_type as i32,
                    confidence: agg_cap.confidence,
                    metadata_json: format!(
                        "{{\"contributors\":{},\"requires_oversight\":{}}}",
                        agg_cap.contributor_count, agg_cap.requires_oversight
                    ),
                    registered_at: Some(Self::current_timestamp()),
                },
            )
            .collect();

        // Calculate readiness score using CapabilityAggregator
        let readiness_score = CapabilityAggregator::calculate_readiness_score(
            &CapabilityAggregator::aggregate_capabilities(&operational)?,
        );

        // Calculate bounding box
        let bounding_box = Some(Self::calculate_bounding_box(&operational)?);

        // Create timestamp
        let aggregated_at = Some(Self::current_timestamp());

        Ok(CellSummary {
            cell_id: cell_id.to_string(),
            leader_id: leader_id.to_string(),
            member_ids,
            member_count,
            position_centroid: Some(position_centroid),
            avg_fuel_minutes,
            worst_health: worst_health as i32,
            operational_count,
            aggregated_capabilities,
            readiness_score,
            bounding_box,
            aggregated_at,
        })
    }

    /// Aggregate cohort-level state from cell summaries
    ///
    /// # Arguments
    ///
    /// * `cohort_id` - Unique cohort identifier
    /// * `leader_id` - Cohort coordinator node ID
    /// * `cells` - Vector of CellSummary from constituent cells
    ///
    /// # Returns
    ///
    /// `CohortSummary` with aggregated position, health, fuel, and capabilities
    pub fn aggregate_cohort(
        cohort_id: &str,
        leader_id: &str,
        cells: Vec<CellSummary>,
    ) -> Result<CohortSummary> {
        if cells.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "Cohort has no cells".to_string(),
                operation: "aggregate_cohort".to_string(),
                source: None,
            });
        }

        // Extract cell IDs
        let cell_ids: Vec<String> = cells.iter().map(|s| s.cell_id.clone()).collect();
        let cell_count = cells.len() as u32;

        // Sum total member count
        let total_member_count: u32 = cells.iter().map(|s| s.member_count).sum();

        // Calculate position centroid from cell centroids
        let position_centroid = Self::calculate_position_centroid_from_positions(
            &cells
                .iter()
                .filter_map(|s| s.position_centroid.as_ref())
                .cloned()
                .collect::<Vec<_>>(),
        )?;

        // Calculate average fuel across cells (weighted by member count)
        let avg_fuel_minutes = Self::calculate_weighted_avg_fuel(&cells);

        // Find worst health across cells
        let worst_health = Self::find_worst_health_from_cells(&cells);

        // Sum operational counts
        let operational_count: u32 = cells.iter().map(|s| s.operational_count).sum();

        // Aggregate capabilities across cells (union of capabilities)
        let aggregated_capabilities = Self::aggregate_capabilities_from_cells(&cells);

        // Calculate cohort readiness (weighted average of cell readiness)
        let readiness_score = Self::calculate_weighted_readiness(&cells);

        // Calculate cohort bounding box from cell bounding boxes
        let bounding_box = Some(Self::aggregate_bounding_boxes(&cells)?);

        // Create timestamp
        let aggregated_at = Some(Self::current_timestamp());

        Ok(CohortSummary {
            cohort_id: cohort_id.to_string(),
            leader_id: leader_id.to_string(),
            cell_ids,
            cell_count,
            total_member_count,
            position_centroid: Some(position_centroid),
            avg_fuel_minutes,
            worst_health: worst_health as i32,
            operational_count,
            aggregated_capabilities,
            readiness_score,
            bounding_box,
            aggregated_at,
        })
    }

    /// Aggregate federation-level state from cohort summaries
    ///
    /// # Arguments
    ///
    /// * `federation_id` - Unique federation identifier
    /// * `leader_id` - Federation coordinator node ID
    /// * `cohorts` - Vector of CohortSummary from constituent cohorts
    ///
    /// # Returns
    ///
    /// `FederationSummary` with aggregated position, health, fuel, and capabilities
    pub fn aggregate_federation(
        federation_id: &str,
        leader_id: &str,
        cohorts: Vec<CohortSummary>,
    ) -> Result<FederationSummary> {
        if cohorts.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "Federation has no cohorts".to_string(),
                operation: "aggregate_federation".to_string(),
                source: None,
            });
        }

        // Extract cohort IDs
        let cohort_ids: Vec<String> = cohorts.iter().map(|p| p.cohort_id.clone()).collect();
        let cohort_count = cohorts.len() as u32;

        // Sum total member count
        let total_member_count: u32 = cohorts.iter().map(|p| p.total_member_count).sum();

        // Calculate position centroid from cohort centroids
        let position_centroid = Self::calculate_position_centroid_from_positions(
            &cohorts
                .iter()
                .filter_map(|p| p.position_centroid.as_ref())
                .cloned()
                .collect::<Vec<_>>(),
        )?;

        // Calculate average fuel across cohorts (weighted by member count)
        let avg_fuel_minutes = Self::calculate_weighted_avg_fuel_from_cohorts(&cohorts);

        // Find worst health across cohorts
        let worst_health = Self::find_worst_health_from_cohorts(&cohorts);

        // Sum operational counts
        let operational_count: u32 = cohorts.iter().map(|p| p.operational_count).sum();

        // Aggregate capabilities across cohorts (union of capabilities)
        let aggregated_capabilities = Self::aggregate_capabilities_from_cohorts(&cohorts);

        // Calculate federation readiness (weighted average of cohort readiness)
        let readiness_score = Self::calculate_weighted_readiness_from_cohorts(&cohorts);

        // Calculate federation bounding box from cohort bounding boxes
        let bounding_box = Some(Self::aggregate_bounding_boxes_from_cohorts(&cohorts)?);

        // Create timestamp
        let aggregated_at = Some(Self::current_timestamp());

        Ok(FederationSummary {
            federation_id: federation_id.to_string(),
            leader_id: leader_id.to_string(),
            cohort_ids,
            cohort_count,
            total_member_count,
            position_centroid: Some(position_centroid),
            avg_fuel_minutes,
            worst_health: worst_health as i32,
            operational_count,
            aggregated_capabilities,
            readiness_score,
            bounding_box,
            aggregated_at,
        })
    }

    /// Aggregate coalition-level state from federation summaries
    ///
    /// Coalition is the top-tier aggregation under the four-tier rigid-schema
    /// model defined by ADR-066. It mirrors `aggregate_federation` one tier up.
    ///
    /// # Arguments
    ///
    /// * `coalition_id` - Unique coalition identifier
    /// * `leader_id` - Coalition coordinator node ID
    /// * `federations` - Vector of FederationSummary from constituent federations
    ///
    /// # Returns
    ///
    /// `CoalitionSummary` with aggregated position, health, fuel, and capabilities
    pub fn aggregate_coalition(
        coalition_id: &str,
        leader_id: &str,
        federations: Vec<FederationSummary>,
    ) -> Result<CoalitionSummary> {
        if federations.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "Coalition has no federations".to_string(),
                operation: "aggregate_coalition".to_string(),
                source: None,
            });
        }

        // Extract federation IDs
        let federation_ids: Vec<String> = federations
            .iter()
            .map(|f| f.federation_id.clone())
            .collect();
        let federation_count = federations.len() as u32;

        // Sum total member count
        let total_member_count: u32 = federations.iter().map(|f| f.total_member_count).sum();

        // Calculate position centroid from federation centroids
        let position_centroid = Self::calculate_position_centroid_from_positions(
            &federations
                .iter()
                .filter_map(|f| f.position_centroid.as_ref())
                .cloned()
                .collect::<Vec<_>>(),
        )?;

        // Calculate average fuel across federations (weighted by member count)
        let avg_fuel_minutes = Self::calculate_weighted_avg_fuel_from_federations(&federations);

        // Find worst health across federations
        let worst_health = Self::find_worst_health_from_federations(&federations);

        // Sum operational counts
        let operational_count: u32 = federations.iter().map(|f| f.operational_count).sum();

        // Aggregate capabilities across federations (union of capabilities)
        let aggregated_capabilities = Self::aggregate_capabilities_from_federations(&federations);

        // Calculate coalition readiness (weighted average of federation readiness)
        let readiness_score = Self::calculate_weighted_readiness_from_federations(&federations);

        // Calculate coalition bounding box from federation bounding boxes
        let bounding_box = Some(Self::aggregate_bounding_boxes_from_federations(
            &federations,
        )?);

        // Create timestamp
        let aggregated_at = Some(Self::current_timestamp());

        Ok(CoalitionSummary {
            coalition_id: coalition_id.to_string(),
            leader_id: leader_id.to_string(),
            federation_ids,
            federation_count,
            total_member_count,
            position_centroid: Some(position_centroid),
            avg_fuel_minutes,
            worst_health: worst_health as i32,
            operational_count,
            aggregated_capabilities,
            readiness_score,
            bounding_box,
            aggregated_at,
        })
    }

    // Helper functions

    /// Calculate position centroid from member states
    fn calculate_position_centroid(members: &[(NodeConfig, NodeState)]) -> Result<Position> {
        let positions: Vec<&Position> = members
            .iter()
            .filter_map(|(_, state)| state.position.as_ref())
            .collect();

        if positions.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "No valid positions to aggregate".to_string(),
                operation: "calculate_position_centroid".to_string(),
                source: None,
            });
        }

        let lat_sum: f64 = positions.iter().map(|p| p.latitude).sum();
        let lon_sum: f64 = positions.iter().map(|p| p.longitude).sum();
        let alt_sum: f64 = positions.iter().map(|p| p.altitude).sum();
        let count = positions.len() as f64;

        Ok(Position {
            latitude: lat_sum / count,
            longitude: lon_sum / count,
            altitude: alt_sum / count,
        })
    }

    /// Calculate position centroid from existing positions (for higher-tier aggregation)
    fn calculate_position_centroid_from_positions(positions: &[Position]) -> Result<Position> {
        if positions.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "No valid positions to aggregate".to_string(),
                operation: "calculate_position_centroid_from_positions".to_string(),
                source: None,
            });
        }

        let lat_sum: f64 = positions.iter().map(|p| p.latitude).sum();
        let lon_sum: f64 = positions.iter().map(|p| p.longitude).sum();
        let alt_sum: f64 = positions.iter().map(|p| p.altitude).sum();
        let count = positions.len() as f64;

        Ok(Position {
            latitude: lat_sum / count,
            longitude: lon_sum / count,
            altitude: alt_sum / count,
        })
    }

    /// Calculate average fuel from member states
    fn calculate_avg_fuel(members: &[(NodeConfig, NodeState)]) -> f32 {
        if members.is_empty() {
            return 0.0;
        }

        let sum: u32 = members.iter().map(|(_, state)| state.fuel_minutes).sum();
        sum as f32 / members.len() as f32
    }

    /// Calculate weighted average fuel from cell summaries
    fn calculate_weighted_avg_fuel(cells: &[CellSummary]) -> f32 {
        if cells.is_empty() {
            return 0.0;
        }

        let total_fuel: f32 = cells
            .iter()
            .map(|s| s.avg_fuel_minutes * s.member_count as f32)
            .sum();
        let total_members: u32 = cells.iter().map(|s| s.member_count).sum();

        if total_members == 0 {
            return 0.0;
        }

        total_fuel / total_members as f32
    }

    /// Find worst health status from member states
    ///
    /// HealthStatus enum values increase with severity:
    /// NOMINAL(1) < DEGRADED(2) < CRITICAL(3) < FAILED(4)
    /// So we use max_by_key to find the worst (highest severity).
    fn find_worst_health(members: &[(NodeConfig, NodeState)]) -> HealthStatus {
        members
            .iter()
            .map(|(_, state)| HealthStatus::try_from(state.health).unwrap_or(HealthStatus::Failed))
            .max_by_key(|h| *h as i32)
            .unwrap_or(HealthStatus::Nominal)
    }

    /// Find worst health status from cell summaries
    fn find_worst_health_from_cells(cells: &[CellSummary]) -> HealthStatus {
        cells
            .iter()
            .map(|s| HealthStatus::try_from(s.worst_health).unwrap_or(HealthStatus::Failed))
            .max_by_key(|h| *h as i32)
            .unwrap_or(HealthStatus::Nominal)
    }

    /// Calculate bounding box from member positions
    fn calculate_bounding_box(members: &[(NodeConfig, NodeState)]) -> Result<BoundingBox> {
        let positions: Vec<&Position> = members
            .iter()
            .filter_map(|(_, state)| state.position.as_ref())
            .collect();

        if positions.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "No valid positions for bounding box".to_string(),
                operation: "calculate_bounding_box".to_string(),
                source: None,
            });
        }

        let min_lat = positions
            .iter()
            .map(|p| p.latitude)
            .fold(f64::INFINITY, f64::min);
        let max_lat = positions
            .iter()
            .map(|p| p.latitude)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_lon = positions
            .iter()
            .map(|p| p.longitude)
            .fold(f64::INFINITY, f64::min);
        let max_lon = positions
            .iter()
            .map(|p| p.longitude)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_alt = positions
            .iter()
            .map(|p| p.altitude)
            .fold(f64::INFINITY, f64::min) as f32;
        let max_alt = positions
            .iter()
            .map(|p| p.altitude)
            .fold(f64::NEG_INFINITY, f64::max) as f32;

        // Calculate centroid for radius calculation
        let centroid_lat = (min_lat + max_lat) / 2.0;
        let centroid_lon = (min_lon + max_lon) / 2.0;

        // Calculate approximate radius (Haversine distance to furthest point)
        let radius_m = positions
            .iter()
            .map(|p| Self::haversine_distance(centroid_lat, centroid_lon, p.latitude, p.longitude))
            .fold(0.0, f32::max);

        Ok(BoundingBox {
            southwest: Some(Position {
                latitude: min_lat,
                longitude: min_lon,
                altitude: min_alt as f64,
            }),
            northeast: Some(Position {
                latitude: max_lat,
                longitude: max_lon,
                altitude: max_alt as f64,
            }),
            max_altitude: max_alt,
            min_altitude: min_alt,
            radius_m,
        })
    }

    /// Aggregate bounding boxes from cell summaries
    fn aggregate_bounding_boxes(cells: &[CellSummary]) -> Result<BoundingBox> {
        let boxes: Vec<&BoundingBox> = cells
            .iter()
            .filter_map(|s| s.bounding_box.as_ref())
            .collect();

        Self::aggregate_boxes(&boxes, "aggregate_bounding_boxes")
    }

    /// Aggregate capabilities from cell summaries (union of capabilities)
    fn aggregate_capabilities_from_cells(
        cells: &[CellSummary],
    ) -> Vec<peat_schema::capability::v1::Capability> {
        use std::collections::HashMap;

        let mut capability_map: HashMap<i32, peat_schema::capability::v1::Capability> =
            HashMap::new();

        for cell in cells {
            for cap in &cell.aggregated_capabilities {
                capability_map
                    .entry(cap.capability_type)
                    .and_modify(|existing| {
                        // Take the higher confidence score
                        if cap.confidence > existing.confidence {
                            existing.confidence = cap.confidence;
                        }
                    })
                    .or_insert_with(|| cap.clone());
            }
        }

        capability_map.into_values().collect()
    }

    /// Calculate weighted average readiness score from cell summaries
    fn calculate_weighted_readiness(cells: &[CellSummary]) -> f32 {
        if cells.is_empty() {
            return 0.0;
        }

        let total_readiness: f32 = cells
            .iter()
            .map(|s| s.readiness_score * s.member_count as f32)
            .sum();
        let total_members: u32 = cells.iter().map(|s| s.member_count).sum();

        if total_members == 0 {
            return 0.0;
        }

        total_readiness / total_members as f32
    }

    // ========================================================================
    // Federation-level helper functions (aggregate from CohortSummary)
    // ========================================================================

    /// Calculate weighted average fuel from cohort summaries
    fn calculate_weighted_avg_fuel_from_cohorts(cohorts: &[CohortSummary]) -> f32 {
        if cohorts.is_empty() {
            return 0.0;
        }

        let total_fuel: f32 = cohorts
            .iter()
            .map(|p| p.avg_fuel_minutes * p.total_member_count as f32)
            .sum();
        let total_members: u32 = cohorts.iter().map(|p| p.total_member_count).sum();

        if total_members == 0 {
            return 0.0;
        }

        total_fuel / total_members as f32
    }

    /// Find worst health status from cohort summaries
    fn find_worst_health_from_cohorts(cohorts: &[CohortSummary]) -> HealthStatus {
        cohorts
            .iter()
            .map(|p| HealthStatus::try_from(p.worst_health).unwrap_or(HealthStatus::Failed))
            .max_by_key(|h| *h as i32)
            .unwrap_or(HealthStatus::Nominal)
    }

    /// Aggregate capabilities from cohort summaries (union of capabilities)
    fn aggregate_capabilities_from_cohorts(
        cohorts: &[CohortSummary],
    ) -> Vec<peat_schema::capability::v1::Capability> {
        use std::collections::HashMap;

        let mut capability_map: HashMap<i32, peat_schema::capability::v1::Capability> =
            HashMap::new();

        for cohort in cohorts {
            for cap in &cohort.aggregated_capabilities {
                capability_map
                    .entry(cap.capability_type)
                    .and_modify(|existing| {
                        if cap.confidence > existing.confidence {
                            existing.confidence = cap.confidence;
                        }
                    })
                    .or_insert_with(|| cap.clone());
            }
        }

        capability_map.into_values().collect()
    }

    /// Calculate weighted average readiness score from cohort summaries
    fn calculate_weighted_readiness_from_cohorts(cohorts: &[CohortSummary]) -> f32 {
        if cohorts.is_empty() {
            return 0.0;
        }

        let total_readiness: f32 = cohorts
            .iter()
            .map(|p| p.readiness_score * p.total_member_count as f32)
            .sum();
        let total_members: u32 = cohorts.iter().map(|p| p.total_member_count).sum();

        if total_members == 0 {
            return 0.0;
        }

        total_readiness / total_members as f32
    }

    /// Aggregate bounding boxes from cohort summaries
    fn aggregate_bounding_boxes_from_cohorts(cohorts: &[CohortSummary]) -> Result<BoundingBox> {
        let boxes: Vec<&BoundingBox> = cohorts
            .iter()
            .filter_map(|p| p.bounding_box.as_ref())
            .collect();

        Self::aggregate_boxes(&boxes, "aggregate_bounding_boxes_from_cohorts")
    }

    // ========================================================================
    // Coalition-level helper functions (aggregate from FederationSummary)
    // ========================================================================

    /// Calculate weighted average fuel from federation summaries
    fn calculate_weighted_avg_fuel_from_federations(federations: &[FederationSummary]) -> f32 {
        if federations.is_empty() {
            return 0.0;
        }

        let total_fuel: f32 = federations
            .iter()
            .map(|f| f.avg_fuel_minutes * f.total_member_count as f32)
            .sum();
        let total_members: u32 = federations.iter().map(|f| f.total_member_count).sum();

        if total_members == 0 {
            return 0.0;
        }

        total_fuel / total_members as f32
    }

    /// Find worst health status from federation summaries
    fn find_worst_health_from_federations(federations: &[FederationSummary]) -> HealthStatus {
        federations
            .iter()
            .map(|f| HealthStatus::try_from(f.worst_health).unwrap_or(HealthStatus::Failed))
            .max_by_key(|h| *h as i32)
            .unwrap_or(HealthStatus::Nominal)
    }

    /// Aggregate capabilities from federation summaries (union of capabilities)
    fn aggregate_capabilities_from_federations(
        federations: &[FederationSummary],
    ) -> Vec<peat_schema::capability::v1::Capability> {
        use std::collections::HashMap;

        let mut capability_map: HashMap<i32, peat_schema::capability::v1::Capability> =
            HashMap::new();

        for federation in federations {
            for cap in &federation.aggregated_capabilities {
                capability_map
                    .entry(cap.capability_type)
                    .and_modify(|existing| {
                        if cap.confidence > existing.confidence {
                            existing.confidence = cap.confidence;
                        }
                    })
                    .or_insert_with(|| cap.clone());
            }
        }

        capability_map.into_values().collect()
    }

    /// Calculate weighted average readiness score from federation summaries
    fn calculate_weighted_readiness_from_federations(federations: &[FederationSummary]) -> f32 {
        if federations.is_empty() {
            return 0.0;
        }

        let total_readiness: f32 = federations
            .iter()
            .map(|f| f.readiness_score * f.total_member_count as f32)
            .sum();
        let total_members: u32 = federations.iter().map(|f| f.total_member_count).sum();

        if total_members == 0 {
            return 0.0;
        }

        total_readiness / total_members as f32
    }

    /// Aggregate bounding boxes from federation summaries
    fn aggregate_bounding_boxes_from_federations(
        federations: &[FederationSummary],
    ) -> Result<BoundingBox> {
        let boxes: Vec<&BoundingBox> = federations
            .iter()
            .filter_map(|f| f.bounding_box.as_ref())
            .collect();

        Self::aggregate_boxes(&boxes, "aggregate_bounding_boxes_from_federations")
    }

    /// Shared helper to fold bounding boxes from any tier into one enclosing box.
    fn aggregate_boxes(boxes: &[&BoundingBox], op_name: &str) -> Result<BoundingBox> {
        if boxes.is_empty() {
            return Err(Error::HierarchicalOp {
                message: "No valid bounding boxes to aggregate".to_string(),
                operation: op_name.to_string(),
                source: None,
            });
        }

        let min_lat = boxes
            .iter()
            .filter_map(|b| b.southwest.as_ref())
            .map(|p| p.latitude)
            .fold(f64::INFINITY, f64::min);
        let max_lat = boxes
            .iter()
            .filter_map(|b| b.northeast.as_ref())
            .map(|p| p.latitude)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_lon = boxes
            .iter()
            .filter_map(|b| b.southwest.as_ref())
            .map(|p| p.longitude)
            .fold(f64::INFINITY, f64::min);
        let max_lon = boxes
            .iter()
            .filter_map(|b| b.northeast.as_ref())
            .map(|p| p.longitude)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_alt = boxes
            .iter()
            .map(|b| b.min_altitude)
            .fold(f32::INFINITY, f32::min);
        let max_alt = boxes
            .iter()
            .map(|b| b.max_altitude)
            .fold(f32::NEG_INFINITY, f32::max);

        // Calculate radius from max of constituent radii
        let radius_m = boxes.iter().map(|b| b.radius_m).fold(0.0, f32::max);

        Ok(BoundingBox {
            southwest: Some(Position {
                latitude: min_lat,
                longitude: min_lon,
                altitude: min_alt as f64,
            }),
            northeast: Some(Position {
                latitude: max_lat,
                longitude: max_lon,
                altitude: max_alt as f64,
            }),
            max_altitude: max_alt,
            min_altitude: min_alt,
            radius_m,
        })
    }

    /// Haversine distance calculation (approximate)
    ///
    /// Returns distance in meters between two lat/lon points
    fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
        const EARTH_RADIUS_M: f64 = 6_371_000.0;

        let d_lat = (lat2 - lat1).to_radians();
        let d_lon = (lon2 - lon1).to_radians();

        let a = (d_lat / 2.0).sin().powi(2)
            + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);

        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

        (EARTH_RADIUS_M * c) as f32
    }

    /// Get current timestamp
    fn current_timestamp() -> Timestamp {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

        Timestamp {
            seconds: now.as_secs(),
            nanos: now.subsec_nanos(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Capability, CapabilityExt, CapabilityType, NodeConfigExt, NodeStateExt};
    use peat_schema::node::v1::HealthStatus as SchemaHealthStatus;

    fn create_test_member(
        id: &str,
        lat: f64,
        lon: f64,
        fuel: u32,
        health: SchemaHealthStatus,
    ) -> (NodeConfig, NodeState) {
        let mut config = NodeConfig::new("TestNode".to_string());
        config.id = id.to_string();
        config.add_capability(Capability::new(
            format!("{}_sensor", id),
            "Test sensor".to_string(),
            CapabilityType::Sensor,
            0.8,
        ));

        let mut state = NodeState::new((lat, lon, 100.0));
        state.fuel_minutes = fuel;
        state.health = health as i32;

        (config, state)
    }

    #[test]
    fn test_aggregate_cell_basic() {
        let members = vec![
            create_test_member(
                "node-1",
                37.7749,
                -122.4194,
                100,
                SchemaHealthStatus::Nominal,
            ),
            create_test_member(
                "node-2",
                37.7750,
                -122.4195,
                90,
                SchemaHealthStatus::Nominal,
            ),
            create_test_member(
                "node-3",
                37.7751,
                -122.4196,
                80,
                SchemaHealthStatus::Degraded,
            ),
        ];

        let result = StateAggregator::aggregate_cell("cell-1", "node-1", members);
        assert!(result.is_ok());

        let summary = result.unwrap();
        assert_eq!(summary.cell_id, "cell-1");
        assert_eq!(summary.leader_id, "node-1");
        assert_eq!(summary.member_count, 3);
        assert_eq!(summary.operational_count, 3);
        assert!(summary.avg_fuel_minutes > 85.0 && summary.avg_fuel_minutes < 95.0);
        assert_eq!(
            HealthStatus::try_from(summary.worst_health).unwrap(),
            HealthStatus::Degraded
        );
    }

    #[test]
    fn test_aggregate_cell_position_centroid() {
        let members = vec![
            create_test_member("node-1", 0.0, 0.0, 100, SchemaHealthStatus::Nominal),
            create_test_member("node-2", 10.0, 10.0, 100, SchemaHealthStatus::Nominal),
        ];

        let summary = StateAggregator::aggregate_cell("cell-1", "node-1", members).unwrap();
        let centroid = summary.position_centroid.unwrap();

        assert!((centroid.latitude - 5.0).abs() < 0.001);
        assert!((centroid.longitude - 5.0).abs() < 0.001);
    }

    fn sample_cell(cell_id: &str, members: u32, fuel: f32, worst: HealthStatus) -> CellSummary {
        CellSummary {
            cell_id: cell_id.to_string(),
            leader_id: format!("leader-{}", cell_id),
            member_ids: (0..members).map(|i| format!("n{}", i)).collect(),
            member_count: members,
            position_centroid: Some(Position {
                latitude: 37.7749,
                longitude: -122.4194,
                altitude: 100.0,
            }),
            avg_fuel_minutes: fuel,
            worst_health: worst as i32,
            operational_count: members,
            aggregated_capabilities: vec![],
            readiness_score: 0.8,
            bounding_box: Some(BoundingBox {
                southwest: Some(Position {
                    latitude: 37.7748,
                    longitude: -122.4195,
                    altitude: 100.0,
                }),
                northeast: Some(Position {
                    latitude: 37.7750,
                    longitude: -122.4193,
                    altitude: 100.0,
                }),
                max_altitude: 100.0,
                min_altitude: 100.0,
                radius_m: 50.0,
            }),
            aggregated_at: None,
        }
    }

    #[test]
    fn test_aggregate_cohort_basic() {
        let cells = vec![
            sample_cell("cell-1", 2, 100.0, HealthStatus::Nominal),
            sample_cell("cell-2", 2, 90.0, HealthStatus::Degraded),
        ];

        let result = StateAggregator::aggregate_cohort("cohort-1", "cohort-leader", cells);
        assert!(result.is_ok());

        let summary = result.unwrap();
        assert_eq!(summary.cohort_id, "cohort-1");
        assert_eq!(summary.leader_id, "cohort-leader");
        assert_eq!(summary.cell_count, 2);
        assert_eq!(summary.total_member_count, 4);
        assert_eq!(summary.operational_count, 4);
        assert!((summary.avg_fuel_minutes - 95.0).abs() < 0.1);
        assert_eq!(
            HealthStatus::try_from(summary.worst_health).unwrap(),
            HealthStatus::Degraded
        );
    }

    #[test]
    fn test_aggregate_coalition_basic() {
        // Build up a small federation -> coalition test.
        let cells_a = vec![sample_cell("cell-1", 3, 100.0, HealthStatus::Nominal)];
        let cells_b = vec![sample_cell("cell-2", 2, 80.0, HealthStatus::Degraded)];

        let cohort_a = StateAggregator::aggregate_cohort("cohort-a", "leader-a", cells_a).unwrap();
        let cohort_b = StateAggregator::aggregate_cohort("cohort-b", "leader-b", cells_b).unwrap();

        let federation = StateAggregator::aggregate_federation(
            "federation-alpha",
            "fed-leader",
            vec![cohort_a, cohort_b],
        )
        .unwrap();

        let coalition = StateAggregator::aggregate_coalition(
            "coalition-1",
            "coalition-leader",
            vec![federation],
        )
        .unwrap();

        assert_eq!(coalition.coalition_id, "coalition-1");
        assert_eq!(coalition.federation_count, 1);
        assert_eq!(coalition.total_member_count, 5);
        assert_eq!(coalition.operational_count, 5);
        assert_eq!(
            HealthStatus::try_from(coalition.worst_health).unwrap(),
            HealthStatus::Degraded
        );
    }

    #[test]
    fn test_haversine_distance() {
        // San Francisco to Oakland (approx 12-13 km)
        let distance = StateAggregator::haversine_distance(
            37.7749,   // SF lat
            -122.4194, // SF lon
            37.8044,   // Oakland lat
            -122.2712, // Oakland lon
        );

        // Should be approximately 12000-13000 meters
        assert!(distance > 11000.0 && distance < 14000.0);
    }
}
