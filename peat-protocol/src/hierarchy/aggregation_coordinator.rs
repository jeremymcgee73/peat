//! Hierarchical Aggregation Coordinator
//!
//! This module provides the coordinator layer for hierarchical state aggregation,
//! implementing ADR-021 document-oriented architecture principles.
//!
//! # Architecture
//!
//! The HierarchicalAggregator sits between the application (peat-sim) and the
//! storage backend, providing:
//! - Backend-agnostic API for hierarchical aggregation
//! - Document lifecycle management (create-once, update-many pattern)
//! - Proper separation of business logic from storage operations
//!
//! # Usage
//!
//! ```rust,ignore
//! use peat_protocol::hierarchy::{HierarchicalAggregator, storage_trait::SummaryStorage};
//! use std::sync::Arc;
//!
//! # async fn example(storage: Arc<dyn SummaryStorage>) -> Result<(), Box<dyn std::error::Error>> {
//! // Create coordinator with any SummaryStorage backend (Ditto, Automerge, etc.)
//! let coordinator = HierarchicalAggregator::new(storage);
//!
//! // Create cell summary once
//! let cell_summary = /* ... */;
//! coordinator.create_cell_summary("cell-1A", &cell_summary).await?;
//!
//! // Update with deltas
//! let delta = /* ... */;
//! coordinator.update_cell_summary("cell-1A", delta).await?;
//! # Ok(())
//! # }
//! ```

use crate::hierarchy::deltas::{CellDelta, CoalitionDelta, CohortDelta, FederationDelta};
use crate::hierarchy::storage_trait::{DocumentMetrics, SummaryStorage};
use crate::Result;
use peat_schema::hierarchy::v1::{CellSummary, CoalitionSummary, CohortSummary, FederationSummary};
use std::sync::Arc;
use tracing::instrument;

/// Hierarchical Aggregation Coordinator
///
/// Coordinates hierarchical state aggregation operations across the Peat Protocol,
/// providing a clean separation between application logic and storage backend.
///
/// # Design
///
/// This coordinator is backend-agnostic - it works with any implementation of
/// `SummaryStorage` trait (Ditto, Automerge/Iroh, etc.). This enables:
/// - Easy backend switching without changing application code
/// - Testing with mock storage implementations
/// - Future optimization opportunities (caching, batching, etc.)
///
/// # Responsibilities
///
/// - Cell/Cohort/Federation/Coalition summary lifecycle management
/// - Delta computation from state changes
/// - Lifecycle metrics validation
/// - Future: Document versioning and conflict resolution
pub struct HierarchicalAggregator {
    /// Storage backend (trait object for backend flexibility)
    storage: Arc<dyn SummaryStorage>,
}

impl HierarchicalAggregator {
    /// Create a new HierarchicalAggregator with the given storage backend
    ///
    /// # Arguments
    ///
    /// * `storage` - Any implementation of SummaryStorage trait
    pub fn new(storage: Arc<dyn SummaryStorage>) -> Self {
        Self { storage }
    }

    // ========================================================================
    // Cell Summary Operations
    // ========================================================================

    /// Create a cell summary document (called ONCE during cell formation)
    #[instrument(skip(self, initial_state), fields(cell_id))]
    pub async fn create_cell_summary(
        &self,
        cell_id: &str,
        initial_state: &CellSummary,
    ) -> Result<String> {
        self.storage
            .create_cell_summary(cell_id, initial_state)
            .await
    }

    /// Update cell summary with delta (called MANY times)
    #[instrument(skip(self, delta), fields(cell_id))]
    pub async fn update_cell_summary(&self, cell_id: &str, delta: CellDelta) -> Result<()> {
        self.storage.update_cell_summary(cell_id, delta).await
    }

    /// Retrieve cell summary
    #[instrument(skip(self), fields(cell_id))]
    pub async fn get_cell_summary(&self, cell_id: &str) -> Result<Option<CellSummary>> {
        self.storage.get_cell_summary(cell_id).await
    }

    // ========================================================================
    // Cohort Summary Operations
    // ========================================================================

    /// Create a cohort summary document (called ONCE during cohort formation)
    #[instrument(skip(self, initial_state), fields(cohort_id))]
    pub async fn create_cohort_summary(
        &self,
        cohort_id: &str,
        initial_state: &CohortSummary,
    ) -> Result<String> {
        self.storage
            .create_cohort_summary(cohort_id, initial_state)
            .await
    }

    /// Update cohort summary with delta (called MANY times)
    #[instrument(skip(self, delta), fields(cohort_id))]
    pub async fn update_cohort_summary(&self, cohort_id: &str, delta: CohortDelta) -> Result<()> {
        self.storage.update_cohort_summary(cohort_id, delta).await
    }

    /// Retrieve cohort summary
    #[instrument(skip(self), fields(cohort_id))]
    pub async fn get_cohort_summary(&self, cohort_id: &str) -> Result<Option<CohortSummary>> {
        self.storage.get_cohort_summary(cohort_id).await
    }

    // ========================================================================
    // Federation Summary Operations
    // ========================================================================

    /// Create a federation summary document (called ONCE during federation formation)
    #[instrument(skip(self, initial_state), fields(federation_id))]
    pub async fn create_federation_summary(
        &self,
        federation_id: &str,
        initial_state: &FederationSummary,
    ) -> Result<String> {
        self.storage
            .create_federation_summary(federation_id, initial_state)
            .await
    }

    /// Update federation summary with delta (called MANY times)
    #[instrument(skip(self, delta), fields(federation_id))]
    pub async fn update_federation_summary(
        &self,
        federation_id: &str,
        delta: FederationDelta,
    ) -> Result<()> {
        self.storage
            .update_federation_summary(federation_id, delta)
            .await
    }

    /// Retrieve federation summary
    #[instrument(skip(self), fields(federation_id))]
    pub async fn get_federation_summary(
        &self,
        federation_id: &str,
    ) -> Result<Option<FederationSummary>> {
        self.storage.get_federation_summary(federation_id).await
    }

    // ========================================================================
    // Coalition Summary Operations (ADR-066 — top-tier aggregation)
    // ========================================================================

    /// Create a coalition summary document (called ONCE during coalition formation)
    #[instrument(skip(self, initial_state), fields(coalition_id))]
    pub async fn create_coalition_summary(
        &self,
        coalition_id: &str,
        initial_state: &CoalitionSummary,
    ) -> Result<String> {
        self.storage
            .create_coalition_summary(coalition_id, initial_state)
            .await
    }

    /// Update coalition summary with delta (called MANY times)
    #[instrument(skip(self, delta), fields(coalition_id))]
    pub async fn update_coalition_summary(
        &self,
        coalition_id: &str,
        delta: CoalitionDelta,
    ) -> Result<()> {
        self.storage
            .update_coalition_summary(coalition_id, delta)
            .await
    }

    /// Retrieve coalition summary
    #[instrument(skip(self), fields(coalition_id))]
    pub async fn get_coalition_summary(
        &self,
        coalition_id: &str,
    ) -> Result<Option<CoalitionSummary>> {
        self.storage.get_coalition_summary(coalition_id).await
    }

    // ========================================================================
    // Lifecycle Metrics (for validation)
    // ========================================================================

    /// Get document lifecycle metrics for validation
    ///
    /// Returns metrics for validating ADR-021 architectural invariants.
    #[instrument(skip(self), fields(doc_id))]
    pub async fn get_document_metrics(&self, doc_id: &str) -> Result<DocumentMetrics> {
        self.storage.get_document_metrics(doc_id).await
    }

    /// Validate document lifecycle invariants
    ///
    /// Checks that:
    /// - Document created exactly once (create_count == 1)
    /// - Delta efficiency is good (compression_ratio > 10×)
    #[instrument(skip(self), fields(doc_id))]
    pub async fn validate_document(&self, doc_id: &str) -> Result<()> {
        let metrics = self.get_document_metrics(doc_id).await?;
        metrics.validate()
    }

    // ========================================================================
    // Backward Compatibility Methods (DEPRECATED - use create/update instead)
    // ========================================================================

    /// Upsert a cell summary (DEPRECATED - use create_cell_summary + update_cell_summary)
    ///
    /// This method exists for backward compatibility with existing code.
    /// New code should use create_cell_summary() once, then update_cell_summary() many times.
    #[deprecated(note = "Use create_cell_summary() once, then update_cell_summary() for updates")]
    #[instrument(skip(self, summary), fields(cell_id))]
    pub async fn upsert_cell_summary(
        &self,
        cell_id: &str,
        summary: &CellSummary,
    ) -> Result<String> {
        // Try to get existing document
        match self.get_cell_summary(cell_id).await? {
            Some(_existing) => {
                // Document exists - this would require creating a delta
                // For now, just return the doc ID
                // TODO: In the future, compute delta and call update
                Ok(format!("{}-summary", cell_id))
            }
            None => {
                // Document doesn't exist - create it
                self.create_cell_summary(cell_id, summary).await
            }
        }
    }

    /// Upsert a cohort summary (DEPRECATED - use create_cohort_summary + update_cohort_summary)
    #[deprecated(
        note = "Use create_cohort_summary() once, then update_cohort_summary() for updates"
    )]
    #[instrument(skip(self, summary), fields(cohort_id))]
    pub async fn upsert_cohort_summary(
        &self,
        cohort_id: &str,
        summary: &CohortSummary,
    ) -> Result<String> {
        match self.get_cohort_summary(cohort_id).await? {
            Some(_existing) => Ok(format!("{}-summary", cohort_id)),
            None => self.create_cohort_summary(cohort_id, summary).await,
        }
    }

    /// Get reference to underlying storage (for backend-specific operations)
    ///
    /// This is intentionally not part of the public API to maintain backend abstraction,
    /// but is needed for some legacy code paths.
    #[doc(hidden)]
    pub fn storage(&self) -> &Arc<dyn SummaryStorage> {
        &self.storage
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_coordinator_creation() {
        // Coordinator creation is tested in integration tests
        // since it requires Ditto SDK initialization
    }
}
