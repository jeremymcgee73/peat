//! Storage abstraction for hierarchical summaries
//!
//! This module defines the backend-agnostic storage interface for hierarchical
//! aggregation, allowing different CRDT backends (Ditto, Automerge/Iroh) to be
//! used interchangeably.

use crate::hierarchy::deltas::{CellDelta, CoalitionDelta, CohortDelta, FederationDelta};
use crate::Result;
use async_trait::async_trait;
use peat_schema::hierarchy::v1::{CellSummary, CoalitionSummary, CohortSummary, FederationSummary};

/// Backend-agnostic storage interface for hierarchical summaries
///
/// This trait abstracts over different CRDT storage backends (Ditto, Automerge/Iroh)
/// and provides the core operations needed for hierarchical aggregation.
///
/// # Design Principles
///
/// 1. **Create-Once Pattern**: Each entity has exactly one living document
/// 2. **Update-Many Pattern**: Updates use deltas, never full recreation
/// 3. **Backend Flexibility**: Implementations handle CRDT semantics differently
///
/// # Implementation Notes
///
/// - **Ditto**: Uses DQL INSERT/UPDATE with JSON documents
/// - **Automerge/Iroh**: Uses CRDT operations on Automerge documents
/// - Both must enforce create-once invariant at their layer
#[async_trait]
pub trait SummaryStorage: Send + Sync {
    // ========================================================================
    // Cell Summary Operations
    // ========================================================================

    /// Create a cell summary document (called ONCE during cell formation)
    ///
    /// # Arguments
    ///
    /// * `cell_id` - Unique cell identifier
    /// * `initial_state` - Initial cell summary state
    ///
    /// # Returns
    ///
    /// Document ID on success
    ///
    /// # Errors
    ///
    /// Returns error if document already exists (enforces create-once)
    async fn create_cell_summary(
        &self,
        cell_id: &str,
        initial_state: &CellSummary,
    ) -> Result<String>;

    /// Update cell summary with delta (called MANY times)
    ///
    /// # Arguments
    ///
    /// * `cell_id` - Unique cell identifier
    /// * `delta` - Field-level delta updates
    ///
    /// # Errors
    ///
    /// Returns error if document does not exist (must create first)
    async fn update_cell_summary(&self, cell_id: &str, delta: CellDelta) -> Result<()>;

    /// Retrieve cell summary
    ///
    /// # Returns
    ///
    /// Some(CellSummary) if found, None if not found
    async fn get_cell_summary(&self, cell_id: &str) -> Result<Option<CellSummary>>;

    /// Delete cell summary (called when cell disbands)
    async fn delete_cell_summary(&self, cell_id: &str) -> Result<()>;

    // ========================================================================
    // Cohort Summary Operations
    // ========================================================================

    /// Create a cohort summary document (called ONCE during cohort formation)
    async fn create_cohort_summary(
        &self,
        cohort_id: &str,
        initial_state: &CohortSummary,
    ) -> Result<String>;

    /// Update cohort summary with delta (called MANY times)
    async fn update_cohort_summary(&self, cohort_id: &str, delta: CohortDelta) -> Result<()>;

    /// Retrieve cohort summary
    async fn get_cohort_summary(&self, cohort_id: &str) -> Result<Option<CohortSummary>>;

    /// Delete cohort summary (called when cohort disbands)
    async fn delete_cohort_summary(&self, cohort_id: &str) -> Result<()>;

    // ========================================================================
    // Federation Summary Operations
    // ========================================================================

    /// Create a federation summary document (called ONCE during federation formation)
    async fn create_federation_summary(
        &self,
        federation_id: &str,
        initial_state: &FederationSummary,
    ) -> Result<String>;

    /// Update federation summary with delta (called MANY times)
    async fn update_federation_summary(
        &self,
        federation_id: &str,
        delta: FederationDelta,
    ) -> Result<()>;

    /// Retrieve federation summary
    async fn get_federation_summary(
        &self,
        federation_id: &str,
    ) -> Result<Option<FederationSummary>>;

    /// Delete federation summary (called when federation disbands)
    async fn delete_federation_summary(&self, federation_id: &str) -> Result<()>;

    // ========================================================================
    // Coalition Summary Operations (ADR-066 — top-tier aggregation)
    // ========================================================================

    /// Create a coalition summary document (called ONCE during coalition formation)
    async fn create_coalition_summary(
        &self,
        coalition_id: &str,
        initial_state: &CoalitionSummary,
    ) -> Result<String>;

    /// Update coalition summary with delta (called MANY times)
    async fn update_coalition_summary(
        &self,
        coalition_id: &str,
        delta: CoalitionDelta,
    ) -> Result<()>;

    /// Retrieve coalition summary
    async fn get_coalition_summary(&self, coalition_id: &str) -> Result<Option<CoalitionSummary>>;

    /// Delete coalition summary (called when coalition disbands)
    async fn delete_coalition_summary(&self, coalition_id: &str) -> Result<()>;

    // ========================================================================
    // Lifecycle Metrics (for validation)
    // ========================================================================

    /// Get document lifecycle metrics for validation
    ///
    /// Returns metrics like create_count, update_count, total_delta_bytes
    /// for validating ADR-021 architectural invariants.
    async fn get_document_metrics(&self, doc_id: &str) -> Result<DocumentMetrics>;
}

/// Document lifecycle metrics for validation
///
/// Used to validate ADR-021 architectural invariants:
/// - create_count must equal 1
/// - update_count should be >> 1
/// - compression_ratio should be > 10×
#[derive(Debug, Clone)]
pub struct DocumentMetrics {
    /// Document identifier
    pub document_id: String,

    /// When document was created (microseconds since epoch)
    pub created_at_us: u64,

    /// Number of times document was created (MUST be 1)
    pub create_count: u64,

    /// Number of times document was updated (should be many)
    pub update_count: u64,

    /// Last update timestamp (microseconds since epoch)
    pub last_update_us: u64,

    /// Total bytes across all deltas
    pub total_delta_bytes: usize,

    /// Current full document size
    pub full_doc_size: usize,

    /// Compression ratio (full_doc_size / avg_delta_size)
    pub compression_ratio: f32,

    /// Current sequence number
    pub sequence: u64,
}

impl DocumentMetrics {
    /// Validate ADR-021 architectural invariants
    ///
    /// # Returns
    ///
    /// Ok(()) if invariants satisfied, Err otherwise
    pub fn validate(&self) -> Result<()> {
        // Invariant 1: Created exactly once
        if self.create_count != 1 {
            return Err(crate::Error::storage_error(
                format!(
                    "Document {} violated create-once invariant: create_count={}",
                    self.document_id, self.create_count
                ),
                "validate_metrics",
                Some(self.document_id.clone()),
            ));
        }

        // Invariant 2: Delta efficiency (if updates exist)
        if self.update_count > 0 && self.compression_ratio < 10.0 {
            tracing::warn!(
                document_id = %self.document_id,
                compression_ratio = self.compression_ratio,
                "Delta efficiency below target (should be >10×)"
            );
        }

        Ok(())
    }

    /// Calculate average delta size
    pub fn avg_delta_size(&self) -> usize {
        if self.update_count == 0 {
            0
        } else {
            self.total_delta_bytes / self.update_count as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_validation_success() {
        let metrics = DocumentMetrics {
            document_id: "cell-1A-summary".to_string(),
            created_at_us: 1234567890,
            create_count: 1, // ✓ Created once
            update_count: 20,
            last_update_us: 1234567900,
            total_delta_bytes: 1000,
            full_doc_size: 2000,
            compression_ratio: 40.0, // ✓ Good compression
            sequence: 20,
        };

        assert!(metrics.validate().is_ok());
    }

    #[test]
    fn test_metrics_validation_create_count_violation() {
        let metrics = DocumentMetrics {
            document_id: "cell-1A-summary".to_string(),
            created_at_us: 1234567890,
            create_count: 21, // ✗ Recreated 21 times (E12 violation)
            update_count: 0,
            last_update_us: 1234567900,
            total_delta_bytes: 0,
            full_doc_size: 2000,
            compression_ratio: 0.0,
            sequence: 0,
        };

        assert!(metrics.validate().is_err());
    }

    #[test]
    fn test_avg_delta_size() {
        let metrics = DocumentMetrics {
            document_id: "cell-1A-summary".to_string(),
            created_at_us: 1234567890,
            create_count: 1,
            update_count: 20,
            last_update_us: 1234567900,
            total_delta_bytes: 1000,
            full_doc_size: 2000,
            compression_ratio: 40.0,
            sequence: 20,
        };

        assert_eq!(metrics.avg_delta_size(), 50); // 1000 / 20
    }
}
