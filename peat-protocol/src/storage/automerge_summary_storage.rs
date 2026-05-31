//! Automerge implementation of SummaryStorage trait
//!
//! This module provides the Automerge backend implementation of the backend-agnostic
//! SummaryStorage trait, enabling hierarchical aggregation with Automerge's CRDT engine.

#[cfg(feature = "automerge-backend")]
use crate::hierarchy::deltas::{CellDelta, CoalitionDelta, CohortDelta, FederationDelta};
#[cfg(feature = "automerge-backend")]
use crate::hierarchy::storage_trait::{DocumentMetrics, SummaryStorage};
#[cfg(feature = "automerge-backend")]
use crate::hierarchy::CellFieldUpdate;
#[cfg(feature = "automerge-backend")]
use crate::storage::automerge_conversion::{
    automerge_to_message, automerge_to_message_if_complete, message_to_automerge,
    message_to_automerge_into,
};
#[cfg(feature = "automerge-backend")]
use crate::storage::automerge_store::AutomergeStore;
#[cfg(feature = "automerge-backend")]
use crate::Result;
#[cfg(feature = "automerge-backend")]
use async_trait::async_trait;
#[cfg(feature = "automerge-backend")]
use peat_schema::hierarchy::v1::{CellSummary, CoalitionSummary, CohortSummary, FederationSummary};
#[cfg(feature = "automerge-backend")]
use std::collections::HashMap;
#[cfg(feature = "automerge-backend")]
use std::sync::{Arc, RwLock};
#[cfg(feature = "automerge-backend")]
use std::time::{SystemTime, UNIX_EPOCH};

/// Automerge-backed implementation of SummaryStorage
///
/// This struct wraps an AutomergeStore and implements the SummaryStorage trait,
/// providing the Automerge-specific implementation of hierarchical aggregation.
///
/// # Design
///
/// This is a thin wrapper that delegates to AutomergeStore methods while implementing
/// the backend-agnostic trait. This enables:
/// - Easy backend switching (alternate SummaryStorage implementations)
/// - Testing with mock storage implementations
/// - Clean separation between protocol logic and storage backend
///
/// # Key Naming Convention
///
/// Documents are stored with prefixed keys to separate namespaces:
/// - Cell summaries: `cell-summary:{cell_id}`
/// - Cohort summaries: `cohort-summary:{cohort_id}`
/// - Federation summaries: `federation-summary:{federation_id}`
/// - Coalition summaries: `coalition-summary:{coalition_id}`
///
/// # Wire-format break (ADR-066, peat#904 Phases 1+2)
///
/// Prior releases used `squad-summary:*`, `platoon-summary:*`, and
/// `company-summary:*` prefixes. Per ADR-066's "discard old state, replay
/// from peers" decision, **no on-disk migration is performed**: any
/// pre-rename documents stored under the old prefixes are silently
/// orphaned by post-rename code (`get_cell_summary` returns `None` for a
/// pre-existing `squad-summary:` document).
///
/// Operators upgrading deployments must provision stores clean before
/// joining a post-rename mesh, then let peer state replay populate the
/// new keys. Phase 3 (peat-mesh) integration tests are responsible for
/// exercising this against fresh stores — running against a store with
/// pre-rename keys present will surface as missing summaries, not as a
/// decode error.
///
/// If a future deployment requires preserving pre-rename state, a
/// one-time migration step can be added to `AutomergeSummaryStorage::new()`
/// using the `cell_key`/`cohort_key`/`federation_key`/`coalition_key`
/// helpers below as the rewrite target. That path is intentionally not
/// shipped now per the ADR.
#[cfg(feature = "automerge-backend")]
pub struct AutomergeSummaryStorage {
    store: Arc<AutomergeStore>,
    /// Metrics tracking for each document (create_count, update_count, etc.)
    metrics: Arc<RwLock<HashMap<String, DocumentMetricsInternal>>>,
}

#[cfg(feature = "automerge-backend")]
struct DocumentMetricsInternal {
    created_at_us: u64,
    create_count: u64,
    update_count: u64,
    last_update_us: u64,
    total_delta_bytes: usize,
}

#[cfg(feature = "automerge-backend")]
impl AutomergeSummaryStorage {
    /// Create a new AutomergeSummaryStorage from an AutomergeStore
    pub fn new(store: Arc<AutomergeStore>) -> Self {
        Self {
            store,
            metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get access to underlying AutomergeStore (for Automerge-specific operations)
    pub fn store(&self) -> &Arc<AutomergeStore> {
        &self.store
    }

    pub(crate) fn cell_key(cell_id: &str) -> String {
        format!("cell-summary:{}", cell_id)
    }

    pub(crate) fn cohort_key(cohort_id: &str) -> String {
        format!("cohort-summary:{}", cohort_id)
    }

    pub(crate) fn federation_key(federation_id: &str) -> String {
        format!("federation-summary:{}", federation_id)
    }

    pub(crate) fn coalition_key(coalition_id: &str) -> String {
        format!("coalition-summary:{}", coalition_id)
    }

    fn now_us() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }

    fn record_create(&self, doc_id: &str) {
        let mut metrics = self.metrics.write().unwrap();
        let now = Self::now_us();
        metrics.insert(
            doc_id.to_string(),
            DocumentMetricsInternal {
                created_at_us: now,
                create_count: 1,
                update_count: 0,
                last_update_us: now,
                total_delta_bytes: 0,
            },
        );
    }

    fn record_update(&self, doc_id: &str, delta_bytes: usize) {
        let mut metrics = self.metrics.write().unwrap();
        if let Some(m) = metrics.get_mut(doc_id) {
            m.update_count += 1;
            m.last_update_us = Self::now_us();
            m.total_delta_bytes += delta_bytes;
        }
    }
}

#[cfg(feature = "automerge-backend")]
#[async_trait]
impl SummaryStorage for AutomergeSummaryStorage {
    // ========================================================================
    // Cell Summary Operations
    // ========================================================================

    async fn create_cell_summary(
        &self,
        cell_id: &str,
        initial_state: &CellSummary,
    ) -> Result<String> {
        let key = Self::cell_key(cell_id);

        // Check if already exists (enforce create-once)
        if self.store.get(&key).ok().flatten().is_some() {
            return Err(crate::Error::storage_error(
                format!("Cell summary {} already exists", cell_id),
                "create_cell_summary",
                Some(key.clone()),
            ));
        }

        // Convert to Automerge document and store
        let doc = message_to_automerge(initial_state).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to convert CellSummary to Automerge: {}", e),
                "create_cell_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store cell summary: {}", e),
                "create_cell_summary",
                Some(key.clone()),
            )
        })?;

        self.record_create(&key);
        Ok(key)
    }

    async fn update_cell_summary(&self, cell_id: &str, delta: CellDelta) -> Result<()> {
        let key = Self::cell_key(cell_id);

        // Get existing document
        let doc = self.store.get(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to get cell summary: {}", e),
                "update_cell_summary",
                Some(key.clone()),
            )
        })?;

        let Some(doc) = doc else {
            return Err(crate::Error::storage_error(
                format!("Cell summary {} not found", cell_id),
                "update_cell_summary",
                Some(key.clone()),
            ));
        };

        // Convert to mutable summary, apply delta, convert back
        let mut summary: CellSummary = automerge_to_message(&doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to deserialize CellSummary: {}", e),
                "update_cell_summary",
                Some(key.clone()),
            )
        })?;

        // Apply delta fields
        let delta_bytes = apply_cell_delta(&mut summary, delta);

        // Convert back to Automerge and store
        // peat#903 / peat#896: pass the existing `doc` so the new write
        // forks from it (preserving CRDT history). Without `Some(&doc)`
        // the receiver sees a fresh actor lineage per update → snapshot
        // bytes stay constant → the CRDT merge resolves to a stale
        // value deterministically.
        let updated_doc = message_to_automerge_into(&summary, Some(&doc)).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to convert updated CellSummary to Automerge: {}", e),
                "update_cell_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &updated_doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store updated cell summary: {}", e),
                "update_cell_summary",
                Some(key.clone()),
            )
        })?;

        self.record_update(&key, delta_bytes);
        Ok(())
    }

    async fn get_cell_summary(&self, cell_id: &str) -> Result<Option<CellSummary>> {
        let key = Self::cell_key(cell_id);

        match self.store.get(&key) {
            Ok(Some(doc)) => {
                // Use automerge_to_message_if_complete to handle partial sync gracefully.
                // If "cell_id" field is missing, the document is incomplete - return None.
                let summary = automerge_to_message_if_complete(&doc, "cell_id").map_err(|e| {
                    crate::Error::storage_error(
                        format!("Failed to deserialize CellSummary: {}", e),
                        "get_cell_summary",
                        Some(key.clone()),
                    )
                })?;
                Ok(summary)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(crate::Error::storage_error(
                format!("Failed to get cell summary: {}", e),
                "get_cell_summary",
                Some(key),
            )),
        }
    }

    async fn delete_cell_summary(&self, cell_id: &str) -> Result<()> {
        let key = Self::cell_key(cell_id);
        self.store.delete(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to delete cell summary: {}", e),
                "delete_cell_summary",
                Some(key.clone()),
            )
        })?;

        // Clean up metrics
        self.metrics.write().unwrap().remove(&key);
        Ok(())
    }

    // ========================================================================
    // Cohort Summary Operations
    // ========================================================================

    async fn create_cohort_summary(
        &self,
        cohort_id: &str,
        initial_state: &CohortSummary,
    ) -> Result<String> {
        let key = Self::cohort_key(cohort_id);

        if self.store.get(&key).ok().flatten().is_some() {
            return Err(crate::Error::storage_error(
                format!("Cohort summary {} already exists", cohort_id),
                "create_cohort_summary",
                Some(key.clone()),
            ));
        }

        let doc = message_to_automerge(initial_state).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to convert CohortSummary to Automerge: {}", e),
                "create_cohort_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store cohort summary: {}", e),
                "create_cohort_summary",
                Some(key.clone()),
            )
        })?;

        self.record_create(&key);
        Ok(key)
    }

    async fn update_cohort_summary(&self, cohort_id: &str, delta: CohortDelta) -> Result<()> {
        let key = Self::cohort_key(cohort_id);

        let doc = self.store.get(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to get cohort summary: {}", e),
                "update_cohort_summary",
                Some(key.clone()),
            )
        })?;

        let Some(doc) = doc else {
            return Err(crate::Error::storage_error(
                format!("Cohort summary {} not found", cohort_id),
                "update_cohort_summary",
                Some(key.clone()),
            ));
        };

        let mut summary: CohortSummary = automerge_to_message(&doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to deserialize CohortSummary: {}", e),
                "update_cohort_summary",
                Some(key.clone()),
            )
        })?;

        let delta_bytes = apply_cohort_delta(&mut summary, delta);

        // peat#903 / peat#896: preserve CRDT history via existing-doc fork.
        let updated_doc = message_to_automerge_into(&summary, Some(&doc)).map_err(|e| {
            crate::Error::storage_error(
                format!(
                    "Failed to convert updated CohortSummary to Automerge: {}",
                    e
                ),
                "update_cohort_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &updated_doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store updated cohort summary: {}", e),
                "update_cohort_summary",
                Some(key.clone()),
            )
        })?;

        self.record_update(&key, delta_bytes);
        Ok(())
    }

    async fn get_cohort_summary(&self, cohort_id: &str) -> Result<Option<CohortSummary>> {
        let key = Self::cohort_key(cohort_id);

        match self.store.get(&key) {
            Ok(Some(doc)) => {
                // Use automerge_to_message_if_complete to handle partial sync gracefully.
                // If "cohort_id" field is missing, the document is incomplete - return None.
                // This fixes issue #509: Automerge partial sync causes deserialization errors.
                let summary = automerge_to_message_if_complete(&doc, "cohort_id").map_err(|e| {
                    crate::Error::storage_error(
                        format!("Failed to deserialize CohortSummary: {}", e),
                        "get_cohort_summary",
                        Some(key.clone()),
                    )
                })?;
                Ok(summary)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(crate::Error::storage_error(
                format!("Failed to get cohort summary: {}", e),
                "get_cohort_summary",
                Some(key),
            )),
        }
    }

    async fn delete_cohort_summary(&self, cohort_id: &str) -> Result<()> {
        let key = Self::cohort_key(cohort_id);
        self.store.delete(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to delete cohort summary: {}", e),
                "delete_cohort_summary",
                Some(key.clone()),
            )
        })?;

        self.metrics.write().unwrap().remove(&key);
        Ok(())
    }

    // ========================================================================
    // Federation Summary Operations
    // ========================================================================

    async fn create_federation_summary(
        &self,
        federation_id: &str,
        initial_state: &FederationSummary,
    ) -> Result<String> {
        let key = Self::federation_key(federation_id);

        if self.store.get(&key).ok().flatten().is_some() {
            return Err(crate::Error::storage_error(
                format!("Federation summary {} already exists", federation_id),
                "create_federation_summary",
                Some(key.clone()),
            ));
        }

        let doc = message_to_automerge(initial_state).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to convert FederationSummary to Automerge: {}", e),
                "create_federation_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store federation summary: {}", e),
                "create_federation_summary",
                Some(key.clone()),
            )
        })?;

        self.record_create(&key);
        Ok(key)
    }

    async fn update_federation_summary(
        &self,
        federation_id: &str,
        delta: FederationDelta,
    ) -> Result<()> {
        let key = Self::federation_key(federation_id);

        let doc = self.store.get(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to get federation summary: {}", e),
                "update_federation_summary",
                Some(key.clone()),
            )
        })?;

        let Some(doc) = doc else {
            return Err(crate::Error::storage_error(
                format!("Federation summary {} not found", federation_id),
                "update_federation_summary",
                Some(key.clone()),
            ));
        };

        let mut summary: FederationSummary = automerge_to_message(&doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to deserialize FederationSummary: {}", e),
                "update_federation_summary",
                Some(key.clone()),
            )
        })?;

        let delta_bytes = apply_federation_delta(&mut summary, delta);

        // peat#903 / peat#896: preserve CRDT history via existing-doc fork.
        let updated_doc = message_to_automerge_into(&summary, Some(&doc)).map_err(|e| {
            crate::Error::storage_error(
                format!(
                    "Failed to convert updated FederationSummary to Automerge: {}",
                    e
                ),
                "update_federation_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &updated_doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store updated federation summary: {}", e),
                "update_federation_summary",
                Some(key.clone()),
            )
        })?;

        self.record_update(&key, delta_bytes);
        Ok(())
    }

    async fn get_federation_summary(
        &self,
        federation_id: &str,
    ) -> Result<Option<FederationSummary>> {
        let key = Self::federation_key(federation_id);

        match self.store.get(&key) {
            Ok(Some(doc)) => {
                // Use automerge_to_message_if_complete to handle partial sync gracefully.
                // If "federation_id" field is missing, the document is incomplete - return None.
                let summary =
                    automerge_to_message_if_complete(&doc, "federation_id").map_err(|e| {
                        crate::Error::storage_error(
                            format!("Failed to deserialize FederationSummary: {}", e),
                            "get_federation_summary",
                            Some(key.clone()),
                        )
                    })?;
                Ok(summary)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(crate::Error::storage_error(
                format!("Failed to get federation summary: {}", e),
                "get_federation_summary",
                Some(key),
            )),
        }
    }

    async fn delete_federation_summary(&self, federation_id: &str) -> Result<()> {
        let key = Self::federation_key(federation_id);
        self.store.delete(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to delete federation summary: {}", e),
                "delete_federation_summary",
                Some(key.clone()),
            )
        })?;

        self.metrics.write().unwrap().remove(&key);
        Ok(())
    }

    // ========================================================================
    // Coalition Summary Operations (ADR-066 — top-tier aggregation)
    // ========================================================================

    async fn create_coalition_summary(
        &self,
        coalition_id: &str,
        initial_state: &CoalitionSummary,
    ) -> Result<String> {
        let key = Self::coalition_key(coalition_id);

        if self.store.get(&key).ok().flatten().is_some() {
            return Err(crate::Error::storage_error(
                format!("Coalition summary {} already exists", coalition_id),
                "create_coalition_summary",
                Some(key.clone()),
            ));
        }

        let doc = message_to_automerge(initial_state).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to convert CoalitionSummary to Automerge: {}", e),
                "create_coalition_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store coalition summary: {}", e),
                "create_coalition_summary",
                Some(key.clone()),
            )
        })?;

        self.record_create(&key);
        Ok(key)
    }

    async fn update_coalition_summary(
        &self,
        coalition_id: &str,
        delta: CoalitionDelta,
    ) -> Result<()> {
        let key = Self::coalition_key(coalition_id);

        let doc = self.store.get(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to get coalition summary: {}", e),
                "update_coalition_summary",
                Some(key.clone()),
            )
        })?;

        let Some(doc) = doc else {
            return Err(crate::Error::storage_error(
                format!("Coalition summary {} not found", coalition_id),
                "update_coalition_summary",
                Some(key.clone()),
            ));
        };

        let mut summary: CoalitionSummary = automerge_to_message(&doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to deserialize CoalitionSummary: {}", e),
                "update_coalition_summary",
                Some(key.clone()),
            )
        })?;

        let delta_bytes = apply_coalition_delta(&mut summary, delta);

        // peat#903 / peat#896: preserve CRDT history via existing-doc fork.
        let updated_doc = message_to_automerge_into(&summary, Some(&doc)).map_err(|e| {
            crate::Error::storage_error(
                format!(
                    "Failed to convert updated CoalitionSummary to Automerge: {}",
                    e
                ),
                "update_coalition_summary",
                Some(key.clone()),
            )
        })?;

        self.store.put(&key, &updated_doc).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to store updated coalition summary: {}", e),
                "update_coalition_summary",
                Some(key.clone()),
            )
        })?;

        self.record_update(&key, delta_bytes);
        Ok(())
    }

    async fn get_coalition_summary(&self, coalition_id: &str) -> Result<Option<CoalitionSummary>> {
        let key = Self::coalition_key(coalition_id);

        match self.store.get(&key) {
            Ok(Some(doc)) => {
                let summary =
                    automerge_to_message_if_complete(&doc, "coalition_id").map_err(|e| {
                        crate::Error::storage_error(
                            format!("Failed to deserialize CoalitionSummary: {}", e),
                            "get_coalition_summary",
                            Some(key.clone()),
                        )
                    })?;
                Ok(summary)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(crate::Error::storage_error(
                format!("Failed to get coalition summary: {}", e),
                "get_coalition_summary",
                Some(key),
            )),
        }
    }

    async fn delete_coalition_summary(&self, coalition_id: &str) -> Result<()> {
        let key = Self::coalition_key(coalition_id);
        self.store.delete(&key).map_err(|e| {
            crate::Error::storage_error(
                format!("Failed to delete coalition summary: {}", e),
                "delete_coalition_summary",
                Some(key.clone()),
            )
        })?;

        self.metrics.write().unwrap().remove(&key);
        Ok(())
    }

    // ========================================================================
    // Lifecycle Metrics
    // ========================================================================

    async fn get_document_metrics(&self, doc_id: &str) -> Result<DocumentMetrics> {
        let metrics = self.metrics.read().unwrap();

        let internal = metrics.get(doc_id).ok_or_else(|| {
            crate::Error::storage_error(
                format!("No metrics found for document {}", doc_id),
                "get_document_metrics",
                Some(doc_id.to_string()),
            )
        })?;

        // Get current document size
        let full_doc_size = self
            .store
            .get(doc_id)
            .ok()
            .flatten()
            .map(|doc| doc.save().len())
            .unwrap_or(0);

        let avg_delta_size = if internal.update_count > 0 {
            internal.total_delta_bytes / internal.update_count as usize
        } else {
            0
        };

        let compression_ratio = if avg_delta_size > 0 {
            full_doc_size as f32 / avg_delta_size as f32
        } else {
            0.0
        };

        Ok(DocumentMetrics {
            document_id: doc_id.to_string(),
            created_at_us: internal.created_at_us,
            create_count: internal.create_count,
            update_count: internal.update_count,
            last_update_us: internal.last_update_us,
            total_delta_bytes: internal.total_delta_bytes,
            full_doc_size,
            compression_ratio,
            sequence: internal.update_count,
        })
    }
}

// ============================================================================
// Delta Application Helpers
// ============================================================================

#[cfg(feature = "automerge-backend")]
use crate::hierarchy::deltas::{CoalitionFieldUpdate, CohortFieldUpdate, FederationFieldUpdate};

/// Apply cell delta to summary, returns approximate delta size in bytes
#[cfg(feature = "automerge-backend")]
fn apply_cell_delta(summary: &mut CellSummary, delta: CellDelta) -> usize {
    let mut bytes = 0;

    for update in delta.updates {
        match update {
            CellFieldUpdate::SetLeaderId(id) => {
                summary.leader_id = id;
                bytes += 16;
            }
            CellFieldUpdate::SetMemberCount(count) => {
                summary.member_count = count;
                bytes += 4;
            }
            CellFieldUpdate::SetOperationalCount(count) => {
                summary.operational_count = count;
                bytes += 4;
            }
            CellFieldUpdate::SetAvgFuelMinutes(fuel) => {
                summary.avg_fuel_minutes = fuel;
                bytes += 4;
            }
            CellFieldUpdate::SetWorstHealth(health) => {
                summary.worst_health = health;
                bytes += 4;
            }
            CellFieldUpdate::SetReadinessScore(score) => {
                summary.readiness_score = score;
                bytes += 4;
            }
            CellFieldUpdate::UpdatePositionCentroid(pos) => {
                summary.position_centroid = Some(pos);
                bytes += 24;
            }
            CellFieldUpdate::AddMemberId(id) => {
                if !summary.member_ids.contains(&id) {
                    summary.member_ids.push(id);
                }
                bytes += 16;
            }
            CellFieldUpdate::RemoveMemberId(id) => {
                summary.member_ids.retain(|m| m != &id);
                bytes += 8;
            }
            CellFieldUpdate::AddCapability(cap) => {
                summary.aggregated_capabilities.push(cap);
                bytes += 100;
            }
            CellFieldUpdate::RemoveCapability(cap_id) => {
                summary.aggregated_capabilities.retain(|c| c.id != cap_id);
                bytes += 8;
            }
            CellFieldUpdate::UpdateBoundingBox(bbox) => {
                summary.bounding_box = Some(bbox);
                bytes += 40;
            }
            CellFieldUpdate::UpdateAggregatedAt(ts) => {
                summary.aggregated_at = Some(ts);
                bytes += 16;
            }
        }
    }

    bytes
}

/// Apply cohort delta to summary, returns approximate delta size in bytes
#[cfg(feature = "automerge-backend")]
fn apply_cohort_delta(summary: &mut CohortSummary, delta: CohortDelta) -> usize {
    let mut bytes = 0;

    for update in delta.updates {
        match update {
            CohortFieldUpdate::SetLeaderId(id) => {
                summary.leader_id = id;
                bytes += 16;
            }
            CohortFieldUpdate::SetCellCount(count) => {
                summary.cell_count = count;
                bytes += 4;
            }
            CohortFieldUpdate::SetTotalMemberCount(count) => {
                summary.total_member_count = count;
                bytes += 4;
            }
            CohortFieldUpdate::SetOperationalCount(count) => {
                summary.operational_count = count;
                bytes += 4;
            }
            CohortFieldUpdate::SetAvgFuelMinutes(fuel) => {
                summary.avg_fuel_minutes = fuel;
                bytes += 4;
            }
            CohortFieldUpdate::SetWorstHealth(health) => {
                summary.worst_health = health;
                bytes += 4;
            }
            CohortFieldUpdate::SetReadinessScore(score) => {
                summary.readiness_score = score;
                bytes += 4;
            }
            CohortFieldUpdate::UpdatePositionCentroid(pos) => {
                summary.position_centroid = Some(pos);
                bytes += 24;
            }
            CohortFieldUpdate::AddCellId(id) => {
                if !summary.cell_ids.contains(&id) {
                    summary.cell_ids.push(id);
                }
                bytes += 16;
            }
            CohortFieldUpdate::RemoveCellId(id) => {
                summary.cell_ids.retain(|s| s != &id);
                bytes += 8;
            }
            CohortFieldUpdate::AddCapability(cap) => {
                summary.aggregated_capabilities.push(cap);
                bytes += 100;
            }
            CohortFieldUpdate::RemoveCapability(cap_id) => {
                summary.aggregated_capabilities.retain(|c| c.id != cap_id);
                bytes += 8;
            }
            CohortFieldUpdate::UpdateBoundingBox(bbox) => {
                summary.bounding_box = Some(bbox);
                bytes += 40;
            }
            CohortFieldUpdate::UpdateAggregatedAt(ts) => {
                summary.aggregated_at = Some(ts);
                bytes += 16;
            }
        }
    }

    bytes
}

/// Apply federation delta to summary, returns approximate delta size in bytes
#[cfg(feature = "automerge-backend")]
fn apply_federation_delta(summary: &mut FederationSummary, delta: FederationDelta) -> usize {
    let mut bytes = 0;

    for update in delta.updates {
        match update {
            FederationFieldUpdate::SetLeaderId(id) => {
                summary.leader_id = id;
                bytes += 16;
            }
            FederationFieldUpdate::SetCohortCount(count) => {
                summary.cohort_count = count;
                bytes += 4;
            }
            FederationFieldUpdate::SetTotalMemberCount(count) => {
                summary.total_member_count = count;
                bytes += 4;
            }
            FederationFieldUpdate::SetOperationalCount(count) => {
                summary.operational_count = count;
                bytes += 4;
            }
            FederationFieldUpdate::SetAvgFuelMinutes(fuel) => {
                summary.avg_fuel_minutes = fuel;
                bytes += 4;
            }
            FederationFieldUpdate::SetWorstHealth(health) => {
                summary.worst_health = health;
                bytes += 4;
            }
            FederationFieldUpdate::SetReadinessScore(score) => {
                summary.readiness_score = score;
                bytes += 4;
            }
            FederationFieldUpdate::UpdatePositionCentroid(pos) => {
                summary.position_centroid = Some(pos);
                bytes += 24;
            }
            FederationFieldUpdate::AddCohortId(id) => {
                if !summary.cohort_ids.contains(&id) {
                    summary.cohort_ids.push(id);
                }
                bytes += 16;
            }
            FederationFieldUpdate::RemoveCohortId(id) => {
                summary.cohort_ids.retain(|p| p != &id);
                bytes += 8;
            }
            FederationFieldUpdate::AddCapability(cap) => {
                summary.aggregated_capabilities.push(cap);
                bytes += 100;
            }
            FederationFieldUpdate::RemoveCapability(cap_id) => {
                summary.aggregated_capabilities.retain(|c| c.id != cap_id);
                bytes += 8;
            }
            FederationFieldUpdate::UpdateBoundingBox(bbox) => {
                summary.bounding_box = Some(bbox);
                bytes += 40;
            }
            FederationFieldUpdate::UpdateAggregatedAt(ts) => {
                summary.aggregated_at = Some(ts);
                bytes += 16;
            }
        }
    }

    bytes
}

/// Apply coalition delta to summary, returns approximate delta size in bytes
///
/// Mirrors `apply_federation_delta` for the top tier introduced by ADR-066.
#[cfg(feature = "automerge-backend")]
fn apply_coalition_delta(summary: &mut CoalitionSummary, delta: CoalitionDelta) -> usize {
    let mut bytes = 0;

    for update in delta.updates {
        match update {
            CoalitionFieldUpdate::SetLeaderId(id) => {
                summary.leader_id = id;
                bytes += 16;
            }
            CoalitionFieldUpdate::SetFederationCount(count) => {
                summary.federation_count = count;
                bytes += 4;
            }
            CoalitionFieldUpdate::SetTotalMemberCount(count) => {
                summary.total_member_count = count;
                bytes += 4;
            }
            CoalitionFieldUpdate::SetOperationalCount(count) => {
                summary.operational_count = count;
                bytes += 4;
            }
            CoalitionFieldUpdate::SetAvgFuelMinutes(fuel) => {
                summary.avg_fuel_minutes = fuel;
                bytes += 4;
            }
            CoalitionFieldUpdate::SetWorstHealth(health) => {
                summary.worst_health = health;
                bytes += 4;
            }
            CoalitionFieldUpdate::SetReadinessScore(score) => {
                summary.readiness_score = score;
                bytes += 4;
            }
            CoalitionFieldUpdate::UpdatePositionCentroid(pos) => {
                summary.position_centroid = Some(pos);
                bytes += 24;
            }
            CoalitionFieldUpdate::AddFederationId(id) => {
                if !summary.federation_ids.contains(&id) {
                    summary.federation_ids.push(id);
                }
                bytes += 16;
            }
            CoalitionFieldUpdate::RemoveFederationId(id) => {
                summary.federation_ids.retain(|p| p != &id);
                bytes += 8;
            }
            CoalitionFieldUpdate::AddCapability(cap) => {
                summary.aggregated_capabilities.push(cap);
                bytes += 100;
            }
            CoalitionFieldUpdate::RemoveCapability(cap_id) => {
                summary.aggregated_capabilities.retain(|c| c.id != cap_id);
                bytes += 8;
            }
            CoalitionFieldUpdate::UpdateBoundingBox(bbox) => {
                summary.bounding_box = Some(bbox);
                bytes += 40;
            }
            CoalitionFieldUpdate::UpdateAggregatedAt(ts) => {
                summary.aggregated_at = Some(ts);
                bytes += 16;
            }
        }
    }

    bytes
}

#[cfg(all(test, feature = "automerge-backend"))]
mod tests {
    use super::*;
    use peat_schema::common::v1::{Position, Timestamp};
    use tempfile::TempDir;

    fn create_test_storage() -> (AutomergeSummaryStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(AutomergeStore::open(temp_dir.path()).unwrap());
        (AutomergeSummaryStorage::new(store), temp_dir)
    }

    #[tokio::test]
    async fn test_cell_summary_crud() {
        let (storage, _temp) = create_test_storage();

        // Create
        let summary = CellSummary {
            cell_id: "cell-1".to_string(),
            leader_id: "leader-1".to_string(),
            member_ids: vec!["m1".to_string(), "m2".to_string()],
            member_count: 2,
            position_centroid: Some(Position {
                latitude: 37.0,
                longitude: -122.0,
                altitude: 100.0,
            }),
            avg_fuel_minutes: 60.0,
            worst_health: 0,
            operational_count: 2,
            aggregated_capabilities: vec![],
            readiness_score: 0.9,
            bounding_box: None,
            aggregated_at: Some(Timestamp {
                seconds: 1234567890,
                nanos: 0,
            }),
        };

        let doc_id = storage
            .create_cell_summary("cell-1", &summary)
            .await
            .expect("create should succeed");
        assert!(doc_id.contains("cell-1"));

        // Read
        let retrieved = storage
            .get_cell_summary("cell-1")
            .await
            .expect("get should succeed")
            .expect("summary should exist");
        assert_eq!(retrieved.cell_id, "cell-1");
        assert_eq!(retrieved.member_count, 2);

        // Update
        let delta = CellDelta {
            cell_id: "cell-1".to_string(),
            timestamp_us: 0,
            sequence: 1,
            updates: vec![
                CellFieldUpdate::SetAvgFuelMinutes(50.0),
                CellFieldUpdate::SetOperationalCount(1),
            ],
        };
        storage
            .update_cell_summary("cell-1", delta)
            .await
            .expect("update should succeed");

        let updated = storage.get_cell_summary("cell-1").await.unwrap().unwrap();
        assert_eq!(updated.avg_fuel_minutes, 50.0);
        assert_eq!(updated.operational_count, 1);

        // Delete
        storage
            .delete_cell_summary("cell-1")
            .await
            .expect("delete should succeed");
        assert!(storage.get_cell_summary("cell-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_create_once_enforcement() {
        let (storage, _temp) = create_test_storage();

        let summary = CellSummary {
            cell_id: "cell-1".to_string(),
            ..Default::default()
        };

        // First create should succeed
        storage
            .create_cell_summary("cell-1", &summary)
            .await
            .expect("first create should succeed");

        // Second create should fail (create-once enforcement)
        let result = storage.create_cell_summary("cell-1", &summary).await;
        assert!(result.is_err());
    }

    /// Regression test for peat#903 (sibling of peat#896).
    ///
    /// `update_cell_summary` previously called
    /// `message_to_automerge(&summary)`, which returned a fresh
    /// `Automerge::new()` doc with a brand-new actor lineage on every
    /// call. The post-`save()` byte sequence stayed constant across
    /// sequential writes, and receivers' CRDT merge resolved to a
    /// deterministic but **stale** actor's value.
    ///
    /// Post-fix uses `message_to_automerge_into(&summary, Some(&doc))`
    /// so the new write forks from the prior doc and history grows.
    /// This test pins both invariants: saved bytes must strictly grow
    /// (with at least one distinct snapshot length) and the final
    /// read-back must equal the most recent write.
    #[tokio::test]
    async fn test_update_cell_summary_preserves_history_peat903() {
        let (storage, _temp) = create_test_storage();
        let store = storage.store().clone();
        let key = AutomergeSummaryStorage::cell_key("cell-1");

        let initial = CellSummary {
            cell_id: "cell-1".to_string(),
            avg_fuel_minutes: 100.0,
            ..Default::default()
        };
        storage
            .create_cell_summary("cell-1", &initial)
            .await
            .expect("create should succeed");

        let mut saved_lens = vec![];
        for counter in (90u32..=100).rev() {
            let delta = CellDelta {
                cell_id: "cell-1".to_string(),
                timestamp_us: crate::hierarchy::deltas::current_timestamp_us(),
                sequence: (100 - counter) as u64 + 1,
                updates: vec![CellFieldUpdate::SetOperationalCount(counter)],
            };
            storage.update_cell_summary("cell-1", delta).await.unwrap();

            let doc = store.get(&key).unwrap().expect("doc must exist");
            saved_lens.push(doc.save().len());
        }

        let unique: std::collections::BTreeSet<_> = saved_lens.iter().copied().collect();
        assert!(
            unique.len() > 1,
            "all {} sequential summary updates produced byte-identical \
             snapshots ({:?}); peat#903 says history is being dropped",
            saved_lens.len(),
            saved_lens.first()
        );
        for w in saved_lens.windows(2) {
            assert!(
                w[1] >= w[0],
                "saved bytes shrunk across sequential summary writes: {w:?}"
            );
        }

        let final_state = storage.get_cell_summary("cell-1").await.unwrap().unwrap();
        assert_eq!(
            final_state.operational_count, 90,
            "expected most recent write (90), got {}",
            final_state.operational_count
        );
    }

    /// peat#903 regression for `update_cohort_summary` — same shape as
    /// the cell test above.
    #[tokio::test]
    async fn test_update_cohort_summary_preserves_history_peat903() {
        let (storage, _temp) = create_test_storage();
        let store = storage.store().clone();
        let key = AutomergeSummaryStorage::cohort_key("cohort-1");

        let initial = CohortSummary {
            cohort_id: "cohort-1".to_string(),
            avg_fuel_minutes: 100.0,
            ..Default::default()
        };
        storage
            .create_cohort_summary("cohort-1", &initial)
            .await
            .expect("create should succeed");

        let mut saved_lens = vec![];
        for counter in (90u32..=100).rev() {
            let delta = CohortDelta {
                cohort_id: "cohort-1".to_string(),
                timestamp_us: crate::hierarchy::deltas::current_timestamp_us(),
                sequence: (100 - counter) as u64 + 1,
                updates: vec![CohortFieldUpdate::SetOperationalCount(counter)],
            };
            storage
                .update_cohort_summary("cohort-1", delta)
                .await
                .unwrap();

            let doc = store.get(&key).unwrap().expect("doc must exist");
            saved_lens.push(doc.save().len());
        }

        let unique: std::collections::BTreeSet<_> = saved_lens.iter().copied().collect();
        assert!(
            unique.len() > 1,
            "cohort summary upserts produced byte-identical snapshots \
             ({:?}); peat#903 history-dropping regression",
            saved_lens.first()
        );
        for w in saved_lens.windows(2) {
            assert!(w[1] >= w[0], "cohort snapshot bytes shrunk: {w:?}");
        }

        let final_state = storage
            .get_cohort_summary("cohort-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_state.operational_count, 90);
    }

    /// peat#903 regression for `update_federation_summary` — same shape.
    #[tokio::test]
    async fn test_update_federation_summary_preserves_history_peat903() {
        let (storage, _temp) = create_test_storage();
        let store = storage.store().clone();
        let key = AutomergeSummaryStorage::federation_key("federation-1");

        let initial = FederationSummary {
            federation_id: "federation-1".to_string(),
            avg_fuel_minutes: 100.0,
            ..Default::default()
        };
        storage
            .create_federation_summary("federation-1", &initial)
            .await
            .expect("create should succeed");

        let mut saved_lens = vec![];
        for counter in (90u32..=100).rev() {
            let delta = FederationDelta {
                federation_id: "federation-1".to_string(),
                timestamp_us: crate::hierarchy::deltas::current_timestamp_us(),
                sequence: (100 - counter) as u64 + 1,
                updates: vec![FederationFieldUpdate::SetOperationalCount(counter)],
            };
            storage
                .update_federation_summary("federation-1", delta)
                .await
                .unwrap();

            let doc = store.get(&key).unwrap().expect("doc must exist");
            saved_lens.push(doc.save().len());
        }

        let unique: std::collections::BTreeSet<_> = saved_lens.iter().copied().collect();
        assert!(
            unique.len() > 1,
            "federation summary upserts produced byte-identical snapshots \
             ({:?}); peat#903 history-dropping regression",
            saved_lens.first()
        );
        for w in saved_lens.windows(2) {
            assert!(w[1] >= w[0], "federation snapshot bytes shrunk: {w:?}");
        }

        let final_state = storage
            .get_federation_summary("federation-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_state.operational_count, 90);
    }

    /// peat#903 regression for `update_coalition_summary` — same shape.
    /// Coalition is the top-tier aggregation introduced by ADR-066.
    #[tokio::test]
    async fn test_update_coalition_summary_preserves_history_peat903() {
        let (storage, _temp) = create_test_storage();
        let store = storage.store().clone();
        let key = AutomergeSummaryStorage::coalition_key("coalition-1");

        let initial = CoalitionSummary {
            coalition_id: "coalition-1".to_string(),
            avg_fuel_minutes: 100.0,
            ..Default::default()
        };
        storage
            .create_coalition_summary("coalition-1", &initial)
            .await
            .expect("create should succeed");

        let mut saved_lens = vec![];
        for counter in (90u32..=100).rev() {
            let delta = CoalitionDelta {
                coalition_id: "coalition-1".to_string(),
                timestamp_us: crate::hierarchy::deltas::current_timestamp_us(),
                sequence: (100 - counter) as u64 + 1,
                updates: vec![CoalitionFieldUpdate::SetOperationalCount(counter)],
            };
            storage
                .update_coalition_summary("coalition-1", delta)
                .await
                .unwrap();

            let doc = store.get(&key).unwrap().expect("doc must exist");
            saved_lens.push(doc.save().len());
        }

        let unique: std::collections::BTreeSet<_> = saved_lens.iter().copied().collect();
        assert!(
            unique.len() > 1,
            "coalition summary upserts produced byte-identical snapshots \
             ({:?}); peat#903 history-dropping regression",
            saved_lens.first()
        );
        for w in saved_lens.windows(2) {
            assert!(w[1] >= w[0], "coalition snapshot bytes shrunk: {w:?}");
        }

        let final_state = storage
            .get_coalition_summary("coalition-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_state.operational_count, 90);
    }

    #[tokio::test]
    async fn test_document_metrics() {
        let (storage, _temp) = create_test_storage();

        let summary = CellSummary {
            cell_id: "cell-1".to_string(),
            avg_fuel_minutes: 60.0,
            ..Default::default()
        };

        let doc_id = storage
            .create_cell_summary("cell-1", &summary)
            .await
            .unwrap();

        // Apply some updates
        for i in 0..5 {
            let delta = CellDelta {
                cell_id: "cell-1".to_string(),
                timestamp_us: crate::hierarchy::deltas::current_timestamp_us(),
                sequence: i + 1,
                updates: vec![CellFieldUpdate::SetAvgFuelMinutes(55.0)],
            };
            storage.update_cell_summary("cell-1", delta).await.unwrap();
        }

        let metrics = storage.get_document_metrics(&doc_id).await.unwrap();
        assert_eq!(metrics.create_count, 1);
        assert_eq!(metrics.update_count, 5);
        assert!(metrics.total_delta_bytes > 0);
    }
}
