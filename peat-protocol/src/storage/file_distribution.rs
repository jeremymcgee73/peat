//! File distribution API (ADR-025 Phase 3)
//!
//! Higher-level API for targeted file delivery and progress monitoring.
//! Builds on `BlobStore` and `BlobDocumentIntegration` to provide
//! formation-aware file distribution with status tracking.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │         FileDistribution Trait          │
//! │  distribute() / status() / cancel()     │
//! └──────────────────┬──────────────────────┘
//!                    │
//!                    ▼
//!            ┌──────────────────┐
//!            │IrohFileDistrib.  │
//!            │ (Direct push)    │
//!            └──────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use peat_protocol::storage::{
//!     FileDistribution, IrohFileDistribution,
//!     DistributionScope, TransferPriority,
//! };
//!
//! // Distribute AI model to all nodes in a formation
//! let handle = distribution.distribute(
//!     &model_token,
//!     DistributionScope::Formation { formation_id: "alpha-cell".into() },
//!     TransferPriority::High,
//! ).await?;
//!
//! // Wait for completion with timeout
//! let status = distribution.wait_for_completion(
//!     &handle,
//!     Duration::from_secs(300),
//! ).await?;
//!
//! println!("Completed: {}/{}", status.completed, status.total_targets);
//! ```

use super::blob_traits::{BlobHash, BlobMetadata, BlobStore, BlobToken};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(feature = "automerge-backend")]
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
#[cfg(feature = "automerge-backend")]
use tokio::sync::RwLock;
#[cfg(feature = "automerge-backend")]
use tracing::{debug, info, warn};
use uuid::Uuid;

// ============================================================================
// Types
// ============================================================================

/// Priority levels for file distribution
///
/// Higher priority transfers are scheduled first and may preempt lower priority
/// transfers when bandwidth is limited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransferPriority {
    /// ROE updates, safety-critical fixes - immediate transfer
    Critical,
    /// Operational model updates - next available window
    High,
    /// Routine updates - best effort
    #[default]
    Normal,
    /// Non-urgent - defer to low-bandwidth periods
    Low,
}

impl TransferPriority {
    /// Get numeric priority (higher = more urgent)
    pub fn as_numeric(&self) -> u8 {
        match self {
            Self::Critical => 4,
            Self::High => 3,
            Self::Normal => 2,
            Self::Low => 1,
        }
    }
}

/// Target scope for file distribution
///
/// Determines which nodes receive the distributed file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum DistributionScope {
    /// All connected nodes in the mesh
    #[default]
    AllNodes,

    /// Specific formation (cell, cohort, federation, coalition)
    Formation {
        /// Formation identifier (e.g., "alpha-cell", "1st-cohort")
        formation_id: String,
    },

    /// Specific nodes by ID
    Nodes {
        /// List of target node IDs
        node_ids: Vec<String>,
    },

    /// Nodes with specific hardware capabilities
    Capable {
        /// Minimum GPU memory in GB (for model deployment)
        #[serde(skip_serializing_if = "Option::is_none")]
        min_gpu_gb: Option<f64>,

        /// Required CPU architecture (e.g., "x86_64", "aarch64")
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_arch: Option<String>,

        /// Minimum available storage in MB
        #[serde(skip_serializing_if = "Option::is_none")]
        min_storage_mb: Option<u64>,
    },
}

/// State of a transfer to a single node
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Transfer not yet started
    #[default]
    Pending,
    /// Establishing connection to node
    Connecting,
    /// Actively transferring data
    Transferring,
    /// Transfer completed successfully
    Completed,
    /// Transfer failed
    Failed,
}

/// Status of transfer to a single node
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeTransferStatus {
    /// Node identifier
    pub node_id: String,
    /// Current transfer state
    pub status: TransferState,
    /// Bytes transferred so far
    pub progress_bytes: u64,
    /// Total bytes to transfer
    pub total_bytes: u64,
    /// When transfer started (if started)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When transfer completed (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NodeTransferStatus {
    /// Create new pending status for a node
    pub fn new(node_id: String, total_bytes: u64) -> Self {
        Self {
            node_id,
            status: TransferState::Pending,
            progress_bytes: 0,
            total_bytes,
            started_at: None,
            completed_at: None,
            error: None,
        }
    }

    /// Calculate progress percentage (0.0 to 1.0)
    pub fn progress_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 1.0;
        }
        self.progress_bytes as f64 / self.total_bytes as f64
    }
}

/// Handle to track a distribution operation
///
/// Returned from `distribute()` and used to query status, cancel, or wait.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DistributionHandle {
    /// Unique distribution ID
    pub distribution_id: String,
    /// Hash of the blob being distributed
    pub blob_hash: BlobHash,
    /// Target scope
    pub scope: DistributionScope,
    /// Transfer priority
    pub priority: TransferPriority,
    /// When distribution was initiated
    pub started_at: DateTime<Utc>,
}

impl DistributionHandle {
    /// Create a new distribution handle
    pub fn new(blob_hash: BlobHash, scope: DistributionScope, priority: TransferPriority) -> Self {
        Self {
            distribution_id: Uuid::new_v4().to_string(),
            blob_hash,
            scope,
            priority,
            started_at: Utc::now(),
        }
    }
}

/// Overall distribution status
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DistributionStatus {
    /// The distribution handle
    pub handle: DistributionHandle,
    /// Total number of target nodes
    pub total_targets: usize,
    /// Number completed successfully
    pub completed: usize,
    /// Number currently in progress
    pub in_progress: usize,
    /// Number failed
    pub failed: usize,
    /// Per-node status
    pub node_statuses: HashMap<String, NodeTransferStatus>,
}

impl DistributionStatus {
    /// Create initial status for a distribution
    pub fn new(handle: DistributionHandle, target_nodes: Vec<String>, total_bytes: u64) -> Self {
        let node_statuses: HashMap<String, NodeTransferStatus> = target_nodes
            .into_iter()
            .map(|id| (id.clone(), NodeTransferStatus::new(id, total_bytes)))
            .collect();

        let total_targets = node_statuses.len();

        Self {
            handle,
            total_targets,
            completed: 0,
            in_progress: 0,
            failed: 0,
            node_statuses,
        }
    }

    /// Check if distribution is complete (all nodes done or failed)
    pub fn is_complete(&self) -> bool {
        self.completed + self.failed >= self.total_targets
    }

    /// Check if distribution succeeded (all targets completed)
    pub fn is_success(&self) -> bool {
        self.completed >= self.total_targets && self.failed == 0
    }

    /// Calculate overall progress fraction
    pub fn overall_progress(&self) -> f64 {
        if self.total_targets == 0 {
            return 1.0;
        }
        let total_bytes: u64 = self.node_statuses.values().map(|s| s.total_bytes).sum();
        let progress_bytes: u64 = self.node_statuses.values().map(|s| s.progress_bytes).sum();
        if total_bytes == 0 {
            return 1.0;
        }
        progress_bytes as f64 / total_bytes as f64
    }

    /// Recalculate counts from node statuses
    pub fn recalculate_counts(&mut self) {
        self.completed = 0;
        self.in_progress = 0;
        self.failed = 0;

        for status in self.node_statuses.values() {
            match status.status {
                TransferState::Completed => self.completed += 1,
                TransferState::Failed => self.failed += 1,
                TransferState::Transferring | TransferState::Connecting => self.in_progress += 1,
                TransferState::Pending => {}
            }
        }
    }
}

// ============================================================================
// FileDistribution Trait
// ============================================================================

/// File distribution service for targeted delivery
///
/// Provides higher-level API for distributing blobs to specific targets
/// with progress tracking and status monitoring.
#[async_trait::async_trait]
pub trait FileDistribution: Send + Sync {
    /// Distribute blob to target nodes
    ///
    /// Initiates distribution of a blob to nodes matching the scope.
    /// Returns a handle for tracking progress.
    ///
    /// # Distribution Behavior by Backend
    ///
    /// **Ditto**: Creates document with blob reference in a distribution
    /// collection. Target nodes subscribe to this collection and fetch
    /// the blob via attachment protocol when they see the reference.
    ///
    /// **Iroh**: Connects directly to target nodes and pushes blob.
    ///
    /// # Arguments
    ///
    /// * `blob_token` - Token identifying the blob to distribute
    /// * `scope` - Target scope (all nodes, formation, specific nodes, capable)
    /// * `priority` - Transfer priority level
    ///
    /// # Returns
    ///
    /// Handle for tracking distribution progress
    async fn distribute(
        &self,
        blob_token: &BlobToken,
        scope: DistributionScope,
        priority: TransferPriority,
    ) -> Result<DistributionHandle>;

    /// Get current distribution status
    ///
    /// Returns the current status of all transfers in a distribution.
    async fn status(&self, handle: &DistributionHandle) -> Result<DistributionStatus>;

    /// Cancel an in-progress distribution
    ///
    /// Stops any pending or in-progress transfers. Completed transfers
    /// are not rolled back.
    async fn cancel(&self, handle: &DistributionHandle) -> Result<()>;

    /// Wait for distribution to complete (or fail)
    ///
    /// Blocks until all targets complete or the timeout expires.
    ///
    /// # Arguments
    ///
    /// * `handle` - Distribution handle
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    ///
    /// Final distribution status, or error if timeout or other failure
    async fn wait_for_completion(
        &self,
        handle: &DistributionHandle,
        timeout: Duration,
    ) -> Result<DistributionStatus>;

    /// Subscribe to distribution progress updates
    ///
    /// Returns a broadcast receiver that emits status updates as
    /// transfers progress.
    async fn subscribe_progress(
        &self,
        handle: &DistributionHandle,
    ) -> Result<broadcast::Receiver<DistributionStatus>>;
}

// ============================================================================
// IrohFileDistribution Implementation (Issue #379, ADR-025)
// ============================================================================

#[cfg(feature = "automerge-backend")]
use super::automerge_store::AutomergeStore;
#[cfg(feature = "automerge-backend")]
use super::iroh_blob_store::NetworkedIrohBlobStore;

/// Distribution collection for Iroh backend.
///
/// Exposed publicly so receiver-side consumers (e.g. peat-node's attachment
/// inbox) can address the same Automerge collection when writing back
/// per-node transfer status — see issue #864.
#[cfg(feature = "automerge-backend")]
pub const IROH_DISTRIBUTION_COLLECTION: &str = "file_distributions";

/// Wire-format of a distribution document stored in
/// [`IROH_DISTRIBUTION_COLLECTION`].
///
/// Sender writes this on `distribute()`; CRDT-syncs to receivers; receivers
/// read it to know whether they're a target, and write back their own
/// [`NodeTransferStatus`] into `node_statuses` keyed by their short endpoint
/// id. The sender's progress watcher (issue #864) then re-reads the doc on
/// each change and publishes a [`DistributionStatus`] frame.
///
/// `node_statuses` is `#[serde(default)]` so legacy documents written before
/// the schema extension still deserialize.
#[cfg(feature = "automerge-backend")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DistributionDocument {
    pub distribution_id: String,
    /// Hex-encoded blob hash.
    pub blob_hash: String,
    pub blob_size: u64,
    pub blob_metadata: BlobMetadata,
    pub scope: DistributionScope,
    pub priority: TransferPriority,
    pub target_nodes: Vec<String>,
    pub started_at: DateTime<Utc>,
    /// Free-form status string: `"distributing"`, `"cancelled"`, …
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<DateTime<Utc>>,
    /// Per-target-node transfer status, keyed by the same short endpoint id
    /// used in `target_nodes`. Receivers append/update their own entry.
    #[serde(default)]
    pub node_statuses: HashMap<String, NodeTransferStatus>,
}

/// Internal: the immutable-by-the-sender half of `DistributionDocument`
/// (everything except `node_statuses`). On the Automerge wire this is
/// serialized as a single JSON byte-scalar at `ROOT.metadata` — only the
/// sender ever writes it, so the wholesale-scalar replacement semantics
/// don't cause contention. Receivers' per-node status entries live as
/// their own keyed entries under the `ROOT.node_statuses` Automerge map
/// (one key per receiver short-id), so multiple receivers writing
/// concurrently never compete for the same Automerge field.
///
/// This split is what closes the substrate race that
/// [defenseunicorns/peat#864](https://github.com/defenseunicorns/peat/issues/864)
/// surfaced: the pre-rc.9 schema embedded `node_statuses` inside a single
/// wholesale-scalar `ROOT.data` blob, so concurrent sender + receiver
/// writes (or, on resource-constrained CI runners, even the sender's
/// initial `data` op vs the receiver's `Transferring` op being treated
/// as concurrent by Automerge's actor-id tiebreak after a load-modify-
/// write cycle) raced at the merge-tiebreak layer, leaving the
/// receiver's local doc stuck at a stale state.
#[cfg(feature = "automerge-backend")]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct DistributionMetadata {
    distribution_id: String,
    blob_hash: String,
    blob_size: u64,
    blob_metadata: BlobMetadata,
    scope: DistributionScope,
    priority: TransferPriority,
    target_nodes: Vec<String>,
    started_at: DateTime<Utc>,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancelled_at: Option<DateTime<Utc>>,
}

/// Automerge field on the distribution document holding the sender's
/// immutable metadata as a JSON byte-scalar. Only the sender writes this
/// field; receivers never touch it.
#[cfg(feature = "automerge-backend")]
const METADATA_FIELD: &str = "metadata";

/// Automerge field on the distribution document holding the per-receiver
/// `NodeTransferStatus` entries as a typed `ObjType::Map`. Each receiver
/// writes only its own key (`peer.fmt_short()`), so concurrent writes from
/// different receivers never collide.
#[cfg(feature = "automerge-backend")]
const NODE_STATUSES_FIELD: &str = "node_statuses";

/// Pre-rc.9 wholesale-scalar field. Read-only support kept so a rc.8
/// document that synced before this node upgraded still deserializes;
/// rc.9 nodes never write into this field.
#[cfg(feature = "automerge-backend")]
const LEGACY_DATA_FIELD: &str = "data";

#[cfg(feature = "automerge-backend")]
fn distribution_doc_key(distribution_id: &str) -> String {
    format!("{IROH_DISTRIBUTION_COLLECTION}:{distribution_id}")
}

/// Read a single distribution document from the store, reconstructing the
/// in-memory [`DistributionDocument`] from the on-wire typed Automerge
/// structure (or the legacy wholesale-scalar format if this doc was
/// written by a pre-rc.9 peer that hasn't seen a rc.9 write yet).
#[cfg(feature = "automerge-backend")]
pub fn read_distribution_document(
    store: &AutomergeStore,
    distribution_id: &str,
) -> Result<Option<DistributionDocument>> {
    let key = distribution_doc_key(distribution_id);
    match store.get(&key)? {
        Some(doc) => Ok(Some(distribution_document_from_automerge(&doc)?)),
        None => Ok(None),
    }
}

/// Scan every distribution document in the collection. Used by peat-node's
/// `attachments::inbox` to discover docs targeting this peer; replaces the
/// pre-rc.9 `Collection::scan` + `serde_json::from_slice` pattern.
#[cfg(feature = "automerge-backend")]
pub fn scan_distribution_documents(
    store: &AutomergeStore,
) -> Result<Vec<(String, DistributionDocument)>> {
    let prefix = format!("{IROH_DISTRIBUTION_COLLECTION}:");
    let raw = store.scan_prefix(&prefix)?;
    let mut out = Vec::with_capacity(raw.len());
    for (full_key, doc) in raw {
        let Some(dist_id) = full_key.strip_prefix(&prefix) else {
            continue;
        };
        match distribution_document_from_automerge(&doc) {
            Ok(d) => out.push((dist_id.to_string(), d)),
            Err(e) => {
                debug!(
                    full_key = %full_key,
                    error = %e,
                    "skipping malformed distribution document during scan"
                );
            }
        }
    }
    Ok(out)
}

/// Scan distribution document keys without loading any document bodies.
///
/// Uses `keys_with_prefix` so no Automerge payloads are loaded or
/// decrypted. Intended as the first step of a scan-then-load loop
/// where the caller can filter by its `handled` set before paying the
/// per-document deserialization cost — see peat#980.
#[cfg(feature = "automerge-backend")]
pub fn scan_distribution_document_ids(store: &AutomergeStore) -> Result<Vec<String>> {
    let prefix = format!("{IROH_DISTRIBUTION_COLLECTION}:");
    Ok(store
        .keys_with_prefix(&prefix)?
        .into_iter()
        .filter_map(|full_key| full_key.strip_prefix(&prefix).map(str::to_string))
        .collect())
}

/// Write one receiver's `NodeTransferStatus` into the distribution
/// document's `node_statuses` Automerge map at the receiver's own
/// `peer.fmt_short()` key.
///
/// **This is the only write peat-node's `attachments::inbox` makes to the
/// distribution document** — and the only path the rc.9 schema is designed
/// to support concurrently. Per-receiver writes go to per-receiver keys
/// inside the typed `node_statuses` map, so two receivers writing at the
/// same instant never compete for the same Automerge field, and a single
/// receiver writing sequentially (Transferring → Completed) replaces its
/// own key's prior value via the normal causally-ordered `put` semantics.
///
/// Returns `Ok(())` if the parent distribution document doesn't exist
/// (synthetic write before sync delivers the metadata). Errors only on
/// JSON serialization or backing-store I/O.
#[cfg(feature = "automerge-backend")]
pub fn write_receiver_node_status(
    store: &AutomergeStore,
    distribution_id: &str,
    receiver_short_id: &str,
    status: &NodeTransferStatus,
) -> Result<()> {
    use automerge::transaction::Transactable;
    use automerge::{ObjType, ReadDoc, ScalarValue, Value, ROOT};

    let key = distribution_doc_key(distribution_id);
    // Serialize the get → transact → put sequence on this doc key.
    // `AutomergeStore::put` is wholesale-replace at the byte level, so
    // two parallel load-modify-write cycles for the same key would
    // silently drop one writer's changes (whichever `put` ran last
    // wins). The striped per-key lock makes the read-modify-write
    // atomic against other writers on the same key (including the
    // sender's own metadata writes and any concurrent receivers).
    let _guard = store.lock_doc(&key);
    let Some(mut doc) = store.get(&key)? else {
        return Ok(());
    };
    let status_bytes = serde_json::to_vec(status)
        .map_err(|e| anyhow::anyhow!("Failed to serialize NodeTransferStatus: {}", e))?;
    doc.transact::<_, _, automerge::AutomergeError>(|tx| {
        let map_id = match tx.get(ROOT, NODE_STATUSES_FIELD)? {
            Some((Value::Object(ObjType::Map), id)) => id,
            _ => tx.put_object(ROOT, NODE_STATUSES_FIELD, ObjType::Map)?,
        };
        tx.put(&map_id, receiver_short_id, ScalarValue::Bytes(status_bytes))?;
        Ok(())
    })
    .map_err(|e| anyhow::anyhow!("Automerge transact failed: {:?}", e))?;
    store.put(&key, &doc)?;
    Ok(())
}

/// Read a [`DistributionDocument`] out of an Automerge document, supporting
/// both the rc.9+ typed schema and the legacy rc.7/rc.8 wholesale-scalar
/// schema (read-only — rc.9 writes never produce the legacy shape).
///
/// Returns an error if neither schema's required fields are present.
#[cfg(feature = "automerge-backend")]
fn distribution_document_from_automerge(
    doc: &automerge::Automerge,
) -> Result<DistributionDocument> {
    use automerge::{ObjType, ReadDoc, ScalarValue, Value, ROOT};

    // Read the `ROOT.node_statuses` typed Automerge Map (if present)
    // into a HashMap. Shared by both the rc.9 path and the legacy
    // path: a rc.9 receiver's `write_receiver_node_status` always
    // lands in this typed Map regardless of whether the document's
    // metadata is still in the legacy `ROOT.data` shape, so BOTH read
    // paths must consult it or cross-version receiver writes are
    // silently dropped (the #864 failure mode, re-introduced).
    let typed_node_statuses =
        |doc: &automerge::Automerge| -> Result<HashMap<String, NodeTransferStatus>> {
            let mut out = HashMap::new();
            if let Some((Value::Object(ObjType::Map), map_id)) =
                doc.get(ROOT, NODE_STATUSES_FIELD)?
            {
                for receiver_key in doc.keys(&map_id) {
                    if let Some((Value::Scalar(scalar), _)) = doc.get(&map_id, &receiver_key)? {
                        if let ScalarValue::Bytes(status_bytes) = scalar.as_ref() {
                            match serde_json::from_slice::<NodeTransferStatus>(status_bytes) {
                                Ok(ns) => {
                                    out.insert(receiver_key, ns);
                                }
                                Err(e) => {
                                    debug!(
                                        receiver = %receiver_key,
                                        error = %e,
                                        "skipping malformed NodeTransferStatus entry"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Ok(out)
        };

    // rc.9+: read the typed metadata + node_statuses map.
    if let Some((Value::Scalar(scalar), _)) = doc.get(ROOT, METADATA_FIELD)? {
        let bytes = match scalar.as_ref() {
            ScalarValue::Bytes(b) => b.clone(),
            other => {
                return Err(anyhow::anyhow!(
                    "{METADATA_FIELD} field has unexpected scalar type {:?}",
                    other
                ));
            }
        };
        let metadata: DistributionMetadata = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {}", e))?;
        let node_statuses = typed_node_statuses(doc)?;
        return Ok(DistributionDocument {
            distribution_id: metadata.distribution_id,
            blob_hash: metadata.blob_hash,
            blob_size: metadata.blob_size,
            blob_metadata: metadata.blob_metadata,
            scope: metadata.scope,
            priority: metadata.priority,
            target_nodes: metadata.target_nodes,
            started_at: metadata.started_at,
            status: metadata.status,
            cancelled_at: metadata.cancelled_at,
            node_statuses,
        });
    }

    // Pre-rc.9 legacy: the sender's metadata + its (empty-at-publish)
    // node_statuses are JSON-serialized into a single `ROOT.data`
    // byte scalar. Read-only support for cross-version sync during an
    // rc-cycle upgrade.
    //
    // CRITICAL: a rc.9 receiver writing against a not-yet-migrated
    // legacy doc lands its `NodeTransferStatus` in the typed
    // `ROOT.node_statuses` Map (next to the legacy `ROOT.data`), NOT
    // inside `ROOT.data`. So the legacy read must overlay the typed
    // map on top of whatever `node_statuses` the legacy `ROOT.data`
    // carried — typed-map entries are strictly newer (a rc.9 write)
    // and take precedence per receiver key. Without this overlay the
    // receiver's status is invisible to the sender's watcher for any
    // distribution that was in flight across the upgrade — exactly
    // the #864 failure mode this whole change exists to close.
    if let Some((Value::Scalar(scalar), _)) = doc.get(ROOT, LEGACY_DATA_FIELD)? {
        if let ScalarValue::Bytes(bytes) = scalar.as_ref() {
            let mut legacy: DistributionDocument = serde_json::from_slice(bytes)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize legacy doc: {}", e))?;
            for (receiver_key, ns) in typed_node_statuses(doc)? {
                legacy.node_statuses.insert(receiver_key, ns);
            }
            return Ok(legacy);
        }
    }

    Err(anyhow::anyhow!(
        "distribution document has neither {METADATA_FIELD} nor {LEGACY_DATA_FIELD} field"
    ))
}

/// Iroh-based file distribution service
///
/// Distributes files/models using NetworkedIrohBlobStore with:
/// - Blob tokens stored in Automerge documents for discovery
/// - Direct P2P transfer via iroh-blobs protocol
/// - Status tracking via distribution documents
///
/// # Architecture
///
/// ```text
/// IrohFileDistribution
///     ├─ NetworkedIrohBlobStore (P2P blob transfer)
///     └─ AutomergeStore (distribution metadata sync)
///
/// Distribution Flow:
/// 1. Commander calls distribute(token, scope)
/// 2. Distribution document created in Automerge with blob token
/// 3. Document syncs to target nodes via CRDT sync
/// 4. Target nodes see distribution doc, fetch blob via iroh-blobs
/// 5. Target nodes update their status in distribution doc
/// ```
#[cfg(feature = "automerge-backend")]
type DistributionsMap = Arc<RwLock<HashMap<String, DistributionStatus>>>;
#[cfg(feature = "automerge-backend")]
type ProgressChannels = Arc<RwLock<HashMap<String, broadcast::Sender<DistributionStatus>>>>;

#[cfg(feature = "automerge-backend")]
pub struct IrohFileDistribution {
    /// Blob store for P2P file transfer
    blob_store: Arc<NetworkedIrohBlobStore>,
    /// Document store for distribution metadata
    document_store: Arc<AutomergeStore>,
    /// Active distributions (distribution_id -> status)
    distributions: DistributionsMap,
    /// Progress broadcast channels per distribution
    progress_channels: ProgressChannels,
    /// Handle to the background watcher that reacts to receiver-side
    /// `node_statuses` writes on the distribution document. Aborted on drop.
    watcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the receive-side watcher (set by
    /// [`Self::start_receive_watcher`]). Aborted on drop.
    receive_watcher_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[cfg(feature = "automerge-backend")]
impl IrohFileDistribution {
    /// Create a new Iroh file distribution service.
    ///
    /// Spawns a background task subscribed to `AutomergeStore`'s observer
    /// channel. The task reconciles per-node status writes (made by
    /// receivers as they fetch the blob — see issue #864 / peat-node#75)
    /// into the in-memory `distributions` map and publishes a fresh
    /// `DistributionStatus` to any progress subscribers.
    pub fn new(
        blob_store: Arc<NetworkedIrohBlobStore>,
        document_store: Arc<AutomergeStore>,
    ) -> Self {
        let distributions: DistributionsMap = Arc::new(RwLock::new(HashMap::new()));
        let progress_channels: ProgressChannels = Arc::new(RwLock::new(HashMap::new()));

        let watcher_handle = {
            let document_store = Arc::clone(&document_store);
            let distributions = Arc::clone(&distributions);
            let progress_channels = Arc::clone(&progress_channels);
            tokio::spawn(async move {
                watch_distribution_documents(document_store, distributions, progress_channels)
                    .await;
            })
        };

        Self {
            blob_store,
            document_store,
            distributions,
            progress_channels,
            watcher_handle: Some(watcher_handle),
            receive_watcher_handle: std::sync::Mutex::new(None),
        }
    }

    /// Start the receive-side watcher: a background task that polls
    /// synced distribution documents, fetches any blob whose
    /// `target_nodes` includes `own_short_id`, and hands the bytes to
    /// `sink`. Distributions this instance originated (present in the
    /// in-memory `distributions` map via `distribute()`) are skipped —
    /// a sender is not its own receiver.
    ///
    /// Idempotent per instance: a second call aborts the prior receive
    /// watcher and starts a fresh one. The task is aborted on drop.
    pub fn start_receive_watcher(
        &self,
        own_short_id: String,
        sink: Arc<dyn ReceiveSink>,
        poll_interval: Duration,
    ) {
        let document_store = Arc::clone(&self.document_store);
        let blob_store = Arc::clone(&self.blob_store);
        let originated = Arc::clone(&self.distributions);
        let handle = tokio::spawn(async move {
            watch_receive_documents(
                document_store,
                blob_store,
                sink,
                own_short_id,
                originated,
                poll_interval,
            )
            .await;
        });
        if let Some(prev) = self
            .receive_watcher_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(handle)
        {
            prev.abort();
        }
    }

    /// Get the blob store reference
    pub fn blob_store(&self) -> &Arc<NetworkedIrohBlobStore> {
        &self.blob_store
    }

    /// Get the document store reference
    pub fn document_store(&self) -> &Arc<AutomergeStore> {
        &self.document_store
    }

    /// Resolve target nodes from scope
    ///
    /// For now, returns known peers from the blob store.
    /// In the future, could query node capabilities from Automerge documents.
    async fn resolve_targets(&self, scope: &DistributionScope) -> Vec<String> {
        match scope {
            DistributionScope::AllNodes => {
                // Return all known peers
                self.blob_store
                    .known_peers()
                    .await
                    .iter()
                    .map(|p| p.fmt_short().to_string())
                    .collect()
            }
            DistributionScope::Nodes { node_ids } => {
                // Return specified nodes (if they're known peers)
                let known_peers: Vec<String> = self
                    .blob_store
                    .known_peers()
                    .await
                    .iter()
                    .map(|p| p.fmt_short().to_string())
                    .collect();

                node_ids
                    .iter()
                    .filter(|id| known_peers.contains(id))
                    .cloned()
                    .collect()
            }
            DistributionScope::Formation { formation_id } => {
                // TODO: Query formation membership from Automerge documents
                // For now, return all known peers (formation filtering not yet implemented)
                warn!(
                    formation_id = %formation_id,
                    "Formation-based distribution not yet implemented, distributing to all peers"
                );
                self.blob_store
                    .known_peers()
                    .await
                    .iter()
                    .map(|p| p.fmt_short().to_string())
                    .collect()
            }
            DistributionScope::Capable { .. } => {
                // TODO: Query node capabilities from Automerge documents
                // For now, return all known peers (capability filtering not yet implemented)
                warn!(
                    "Capability-based distribution not yet implemented, distributing to all peers"
                );
                self.blob_store
                    .known_peers()
                    .await
                    .iter()
                    .map(|p| p.fmt_short().to_string())
                    .collect()
            }
        }
    }

    /// Store the sender's immutable distribution metadata + initialize an
    /// empty `node_statuses` Automerge map. rc.9 schema: writes structured
    /// Automerge fields (`ROOT.metadata` byte-scalar + `ROOT.node_statuses`
    /// map) directly via `AutomergeStore::put`, bypassing the
    /// `Collection::upsert` wholesale-scalar `ROOT.data` field that the
    /// pre-rc.9 schema used (and that the receiver-local doc race in
    /// defenseunicorns/peat#864 traced back to).
    async fn store_distribution_document(
        &self,
        handle: &DistributionHandle,
        blob_token: &BlobToken,
        target_nodes: &[String],
    ) -> Result<()> {
        use automerge::transaction::Transactable;
        use automerge::{Automerge, ObjType, ReadDoc, ScalarValue, Value, ROOT};

        let key = distribution_doc_key(&handle.distribution_id);
        // Serialize the load-modify-write cycle on this doc key against
        // concurrent receiver writes on the same key. See the matching
        // lock in `write_receiver_node_status` for the rationale.
        let _guard = self.document_store.lock_doc(&key);

        let metadata = DistributionMetadata {
            distribution_id: handle.distribution_id.clone(),
            blob_hash: blob_token.hash.as_hex().to_string(),
            blob_size: blob_token.size_bytes,
            blob_metadata: blob_token.metadata.clone(),
            scope: handle.scope.clone(),
            priority: handle.priority,
            target_nodes: target_nodes.to_vec(),
            started_at: handle.started_at,
            status: "distributing".to_string(),
            cancelled_at: None,
        };
        let metadata_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| anyhow::anyhow!("Failed to serialize metadata: {}", e))?;

        let mut doc = self
            .document_store
            .get(&key)?
            .unwrap_or_else(Automerge::new);
        doc.transact::<_, _, automerge::AutomergeError>(|tx| {
            tx.put(
                ROOT,
                METADATA_FIELD,
                ScalarValue::Bytes(metadata_bytes.clone()),
            )?;
            // Initialize an empty node_statuses map if it doesn't exist
            // already. Don't overwrite an existing map — that would erase
            // any receiver writes that landed before the sender's
            // metadata write (rare but possible under aggressive sync).
            if !matches!(
                tx.get(ROOT, NODE_STATUSES_FIELD)?,
                Some((Value::Object(ObjType::Map), _))
            ) {
                tx.put_object(ROOT, NODE_STATUSES_FIELD, ObjType::Map)?;
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("Automerge transact failed: {:?}", e))?;
        self.document_store.put(&key, &doc)?;

        debug!(
            distribution_id = %handle.distribution_id,
            blob_hash = %blob_token.hash,
            target_count = target_nodes.len(),
            "Stored distribution document (rc.9 typed schema) in Automerge"
        );

        Ok(())
    }

    /// Broadcast progress update to subscribers
    async fn broadcast_progress(&self, distribution_id: &str, status: &DistributionStatus) {
        let channels = self.progress_channels.read().await;
        if let Some(sender) = channels.get(distribution_id) {
            // Ignore send errors (no subscribers)
            let _ = sender.send(status.clone());
        }
    }
}

#[cfg(feature = "automerge-backend")]
impl Drop for IrohFileDistribution {
    fn drop(&mut self) {
        if let Some(handle) = self.watcher_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self
            .receive_watcher_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
    }
}

/// Background task that reconciles receiver-written `node_statuses` from
/// the distribution document back into the sender's in-memory state and
/// publishes a fresh [`DistributionStatus`] to progress subscribers.
///
/// Subscribed to [`AutomergeStore::subscribe_to_observer_changes`], which
/// fires for both local writes and sync-applied remote writes — so this
/// task sees the receiver's `node_statuses` updates the moment they
/// CRDT-sync back to the sender.
///
/// Broadcasts only when the merge actually changes the in-memory state.
/// This filters out two noise sources:
///  - The sender's own `distribute()` initial doc write (`node_statuses`
///    is empty; merge is a no-op).
///  - The sender's own `cancel()` doc write (skipped explicitly because
///    `cancel()` already publishes a terminal frame on its own path).
///
/// Closes the broadcast channel after publishing a terminal frame so
/// subscribers see `RecvError::Closed` once the distribution is complete.
#[cfg(feature = "automerge-backend")]
async fn watch_distribution_documents(
    document_store: Arc<AutomergeStore>,
    distributions: DistributionsMap,
    progress_channels: ProgressChannels,
) {
    let mut rx = document_store.subscribe_to_observer_changes();
    let prefix = format!("{}:", IROH_DISTRIBUTION_COLLECTION);

    loop {
        let key = match rx.recv().await {
            Ok(k) => k,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    lagged = n,
                    "distribution watcher lagged on observer channel"
                );
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };

        let Some(doc_id) = key.strip_prefix(&prefix) else {
            continue;
        };

        // Only react to distributions this instance originated.
        if !distributions.read().await.contains_key(doc_id) {
            continue;
        }

        let doc = match read_distribution_document(document_store.as_ref(), doc_id) {
            Ok(Some(d)) => d,
            Ok(None) => continue,
            Err(e) => {
                warn!(error = %e, doc_id, "failed to read/decode distribution doc");
                continue;
            }
        };

        // The sender's own cancel() write flips status to "cancelled" and
        // publishes its terminal frame on a separate synchronous path —
        // skip so subscribers don't see a duplicate cancelled frame.
        if doc.status != "distributing" {
            continue;
        }

        // Merge node_statuses; track whether anything changed.
        let (snapshot, complete) = {
            let mut dists = distributions.write().await;
            let Some(status) = dists.get_mut(doc_id) else {
                continue;
            };
            let mut changed = false;
            for (node_id, ns) in &doc.node_statuses {
                let differs = match status.node_statuses.get(node_id) {
                    Some(existing) => {
                        existing.status != ns.status
                            || existing.progress_bytes != ns.progress_bytes
                            || existing.error != ns.error
                    }
                    None => true,
                };
                if differs {
                    status.node_statuses.insert(node_id.clone(), ns.clone());
                    changed = true;
                }
            }
            if !changed {
                continue;
            }
            status.recalculate_counts();
            (status.clone(), status.is_complete())
        };

        // Publish the merged snapshot.
        {
            let channels = progress_channels.read().await;
            if let Some(sender) = channels.get(doc_id) {
                let _ = sender.send(snapshot);
            }
        }

        // If the distribution is now complete, drop the sender so
        // subscribers observe RecvError::Closed after the terminal frame.
        if complete {
            progress_channels.write().await.remove(doc_id);
        }
    }
}

// ===========================================================================
// Receive-side distribution lifecycle (issue #68)
// ===========================================================================
//
// The receive side — observe synced distribution documents that target
// this node, fetch the referenced blob, write per-receiver
// `node_statuses` so the sender's `watch_distribution_documents` emits
// progress frames — is owned here in peat-protocol, not in consumers.
// Consumers supply only a [`ReceiveSink`]: where the fetched bytes land
// and whether a prior delivery already satisfies a distribution
// (restart idempotency). Everything orchestration-shaped (targeting,
// dedup, fetch, retry, status writes, the test fault seam) lives in
// this module so every consumer gets identical, tested behavior.

/// Where a received blob's bytes go, and whether a prior delivery
/// already satisfied a distribution. The receive watcher
/// ([`IrohFileDistribution::start_receive_watcher`]) owns observe /
/// target / dedup / fetch / status-write orchestration; a `ReceiveSink`
/// is the thin per-consumer tail: persist the bytes, and answer "do I
/// already have this?" durably enough to survive a process restart.
#[cfg(feature = "automerge-backend")]
#[async_trait::async_trait]
pub trait ReceiveSink: Send + Sync {
    /// Consulted before every fetch. Return `true` if this
    /// distribution's blob is already durably present (e.g. on the
    /// receiver's filesystem from a prior process) so the fetch and
    /// inbox write are skipped. The in-memory dedup set is cleared on
    /// restart; this is the durable source of truth. Return `false` on
    /// any ambiguity/error — re-delivering is safer than silently
    /// skipping a file that ought to land.
    async fn already_delivered(&self, doc: &DistributionDocument) -> bool;

    /// Persist the fetched blob (downloaded to `blob_path` on local
    /// disk) for `doc`. Implementations should write atomically
    /// (tmp + rename) so readers never observe a partial file. An
    /// `Err` return is retried on the next sweep (no terminal Failed
    /// status is written for transient delivery errors).
    async fn deliver(&self, doc: &DistributionDocument, blob_path: &std::path::Path) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Test fault/throttle seam (PRD §Testing Plan tests 24 & 29)
// ---------------------------------------------------------------------------
//
// Tests 24 (`cancel_in_flight_stops_transfer`) and 29
// (`subscribe_mixed_state_emits_snapshot_for_terminal_then_live_for_inflight`)
// both need to control a receiver's blob fetch deterministically: 24
// needs a measurable in-flight window to cancel into, 29 needs one
// distribution driven to FAILED while another stays IN_PROGRESS.
//
// This is a process-global, default-empty registry consulted once per
// distribution per sweep. **Not** a Cargo feature or `#[cfg(test)]`:
// integration tests are a separate crate (so `#[cfg(test)]` lib gates
// are inert for them), and a feature flag would exclude these PRD
// acceptance tests from the default `cargo test` CI run — the entire
// point of un-ignoring them is that CI exercises them. The cost when
// unpopulated (production) is one `RwLock` read returning `None` per
// distribution per sweep tick: negligible, and a complete behavioral
// no-op. Keyed by **blob_hash** (hex), not distribution_id, so a test
// can arm a directive *before* the distribution_id is minted —
// race-free against the receiver's first sweep. `#[doc(hidden)]`: the
// seam must be a non-`cfg(test)` `pub` symbol so the separate
// integration-test crate can reach it under the default `cargo test`,
// but it is NOT a supported library API.
#[cfg(feature = "automerge-backend")]
#[doc(hidden)]
#[derive(Clone, Debug)]
pub enum ReceiveTestDirective {
    /// Hold this distribution in-flight: after the `Transferring`
    /// write, skip the fetch *this sweep* and move on (do NOT block the
    /// sweep loop, do NOT mark handled) so the distribution stays
    /// IN_PROGRESS and is revisited next sweep. Each revisit re-reads
    /// the doc: once the sender cancels (status != "distributing") the
    /// receiver stops — it must not deliver a cancelled distribution
    /// (a correctness property, and the basis of PRD test 24's
    /// deterministic mid-flight cancel).
    ///
    /// Non-blocking by design: an inline `sleep` inside the sequential
    /// per-distribution sweep loop would starve every *other*
    /// distribution in the same sweep for the pause duration
    /// (order-dependent flake in PRD test 29's two-distribution bundle).
    HoldInFlight,
    /// Skip the fetch entirely and write a `Failed` node_status with
    /// this error string. Drives one distribution to FAILED
    /// deterministically for PRD test 29.
    FailFetch(String),
}

#[cfg(feature = "automerge-backend")]
static RECEIVE_TEST_HOOK: std::sync::OnceLock<
    std::sync::RwLock<HashMap<String, ReceiveTestDirective>>,
> = std::sync::OnceLock::new();

#[cfg(feature = "automerge-backend")]
fn receive_test_hook() -> &'static std::sync::RwLock<HashMap<String, ReceiveTestDirective>> {
    RECEIVE_TEST_HOOK.get_or_init(|| std::sync::RwLock::new(HashMap::new()))
}

/// Test-only: arm a receive-path directive for blobs whose hex
/// `blob_hash` equals `blob_hash`. Production never calls this; an
/// unarmed hash is a no-op. See [`ReceiveTestDirective`].
#[cfg(feature = "automerge-backend")]
#[doc(hidden)]
pub fn set_receive_test_directive(blob_hash: &str, directive: ReceiveTestDirective) {
    receive_test_hook()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(blob_hash.to_string(), directive);
}

/// Test-only: clear all armed receive-path directives.
#[cfg(feature = "automerge-backend")]
#[doc(hidden)]
pub fn clear_receive_test_directives() {
    receive_test_hook()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

#[cfg(feature = "automerge-backend")]
fn peek_receive_directive(blob_hash: &str) -> Option<ReceiveTestDirective> {
    let guard = receive_test_hook()
        .read()
        .unwrap_or_else(|e| e.into_inner());
    guard.get(blob_hash).cloned()
}

/// Receiver-side node-status writes the receive watcher emits via
/// [`write_receiver_node_status`].
///
/// `Transferring` once fetch begins; `Completed` once the sink's
/// delivery lands. `Failed` is normally NOT written — fetch/delivery
/// failures retry on the next sweep rather than being treated as
/// permanent — and is reachable only through the test fault seam
/// ([`ReceiveTestDirective::FailFetch`]). A production
/// retry-budget-exhaustion give-up would also use this arm.
#[cfg(feature = "automerge-backend")]
enum ReceiverStatusWrite {
    Transferring,
    Completed,
    /// error string carried into the written `NodeTransferStatus`.
    Failed(String),
}

/// Write a receiver's `NodeTransferStatus` into the distribution doc
/// via [`write_receiver_node_status`]. Each receiver writes only to its
/// own keyed entry in `node_statuses` (a typed Automerge Map), so
/// concurrent receivers don't collide and a receiver's sequential
/// writes (Transferring → Completed) are causally ordered against
/// themselves on the same key.
#[cfg(feature = "automerge-backend")]
fn write_receiver_status(
    document_store: &AutomergeStore,
    doc: &DistributionDocument,
    own_short_id: &str,
    state: ReceiverStatusWrite,
) -> Result<()> {
    let now = Utc::now();
    let ns = match state {
        ReceiverStatusWrite::Transferring => NodeTransferStatus {
            node_id: own_short_id.to_string(),
            status: TransferState::Transferring,
            progress_bytes: 0,
            total_bytes: doc.blob_size,
            started_at: Some(now),
            completed_at: None,
            error: None,
        },
        ReceiverStatusWrite::Completed => {
            // Preserve started_at if the sweep snapshot saw our own
            // Transferring write; otherwise stamp now so the doc has
            // some timing signal at all.
            let started_at = doc
                .node_statuses
                .get(own_short_id)
                .and_then(|s| s.started_at)
                .or(Some(now));
            NodeTransferStatus {
                node_id: own_short_id.to_string(),
                status: TransferState::Completed,
                progress_bytes: doc.blob_size,
                total_bytes: doc.blob_size,
                started_at,
                completed_at: Some(now),
                error: None,
            }
        }
        ReceiverStatusWrite::Failed(ref msg) => {
            let started_at = doc
                .node_statuses
                .get(own_short_id)
                .and_then(|s| s.started_at)
                .or(Some(now));
            NodeTransferStatus {
                node_id: own_short_id.to_string(),
                status: TransferState::Failed,
                progress_bytes: 0,
                total_bytes: doc.blob_size,
                started_at,
                completed_at: None,
                error: Some(msg.clone()),
            }
        }
    };

    write_receiver_node_status(document_store, &doc.distribution_id, own_short_id, &ns)?;

    debug!(
        distribution_id = %doc.distribution_id,
        node = %own_short_id,
        new_status = ?match state {
            ReceiverStatusWrite::Transferring => "Transferring",
            ReceiverStatusWrite::Completed => "Completed",
            ReceiverStatusWrite::Failed(_) => "Failed",
        },
        "wrote receiver node_status into distribution doc"
    );
    Ok(())
}

/// Number of receive sweeps a not-yet-delivered distribution is always
/// re-fetched before the Completed-holder gate engages (peat-mesh#137 /
/// #226). The grace lets a transient early-sweep fetch failure recover
/// (sender becomes reachable, or this node's own next attempt succeeds)
/// without stranding the distribution; after it, an unreachable sender no
/// longer drives per-sweep endpoint churn. ~3 sweeps at the 1s inbox poll
/// is a few seconds of grace — short relative to a real transfer, ample
/// for a handshake to settle.
#[cfg(feature = "automerge-backend")]
const RECEIVE_FETCH_GRACE_ATTEMPTS: u32 = 3;

/// Whether to *defer* (skip) re-fetching a distribution's blob this sweep.
///
/// Defer only once the grace window is exhausted AND no other node reports
/// the blob `Completed` (no reachable source to pull from). A completed
/// holder always re-enables the fetch — so a distribution is never
/// stranded once any peer holds it complete, and transient early failures
/// are retried through the grace window.
#[cfg(feature = "automerge-backend")]
fn should_defer_fetch(prior_attempts: u32, completed_holder_exists: bool) -> bool {
    prior_attempts >= RECEIVE_FETCH_GRACE_ATTEMPTS && !completed_holder_exists
}

/// One receive sweep: discover synced distribution documents, deliver
/// any that target this node and aren't already handled.
///
/// `originated` returns `true` for distributions this node *sent*
/// (so the receive path skips them — a sender is not its own
/// receiver). For [`IrohFileDistribution`] this is the in-memory
/// `distributions` map populated by `distribute()`.
#[cfg(feature = "automerge-backend")]
async fn receive_sweep_once(
    document_store: &Arc<AutomergeStore>,
    blob_store: &Arc<NetworkedIrohBlobStore>,
    sink: &Arc<dyn ReceiveSink>,
    own_short_id: &str,
    originated: &DistributionsMap,
    handled: &mut std::collections::HashSet<String>,
    attempt_counts: &mut std::collections::HashMap<String, u32>,
) -> Result<()> {
    // Key-only scan: no Automerge decode for IDs already in `handled`.
    // Docs are loaded individually for the unhandled subset only (peat#980).
    let all_ids = scan_distribution_document_ids(document_store.as_ref())?;
    let unhandled: Vec<String> = all_ids
        .into_iter()
        .filter(|id| !handled.contains(id))
        .collect();

    debug!(
        new_ids = unhandled.len(),
        already_handled = handled.len(),
        "receive sweep"
    );

    if unhandled.is_empty() {
        return Ok(());
    }

    // Pre-fetch originated IDs once per sweep — one lock acquisition
    // instead of one per document (peat#980).
    let originated_ids: std::collections::HashSet<String> =
        { originated.read().await.keys().cloned().collect() };

    for doc_id in unhandled {
        // Self-skip: distributions this node originated live in the
        // in-memory `distributions` map; a receiver never has an entry
        // there because that map is populated only by `distribute()`.
        if originated_ids.contains(&doc_id) {
            handled.insert(doc_id);
            continue;
        }

        let doc = match read_distribution_document(document_store.as_ref(), &doc_id) {
            Ok(Some(d)) => d,
            Ok(None) => {
                // Deleted between key scan and load — won't reappear.
                handled.insert(doc_id);
                continue;
            }
            Err(e) => {
                // Malformed doc (encoding bug, version skew, disk corruption).
                // Mark handled so the watcher doesn't re-abort on every sweep.
                debug!(
                    doc_id = %doc_id,
                    error = %e,
                    "skipping malformed distribution document during sweep"
                );
                handled.insert(doc_id);
                continue;
            }
        };

        debug!(
            distribution_id = %doc.distribution_id,
            blob_hash = %doc.blob_hash,
            target_nodes = ?doc.target_nodes,
            own = %own_short_id,
            "receive: seen distribution doc"
        );

        // Targeting check: my short endpoint id must be in the
        // sender's resolved target_nodes list.
        if !doc.target_nodes.contains(&own_short_id.to_string()) {
            debug!(distribution_id = %doc.distribution_id, "receive: not a target, skipping");
            handled.insert(doc_id);
            continue;
        }

        // Durable "already delivered" gate. Distinct from the in-memory
        // `handled` set: this survives process restart, so a
        // long-running receiver that restarts doesn't re-fetch and
        // re-deliver every historical distribution.
        if sink.already_delivered(&doc).await {
            debug!(
                distribution_id = %doc.distribution_id,
                "receive: sink reports already delivered, skipping fetch"
            );
            handled.insert(doc_id);
            continue;
        }

        // peat-mesh#137 follow-up: gate re-fetch on a Completed holder to
        // avoid endpoint churn against an unreachable sender — but only
        // AFTER a short grace window (#963 QA / peat-mesh#226). The first
        // RECEIVE_FETCH_GRACE_ATTEMPTS sweeps always fetch (direct from the
        // sender), so a *transient* early failure (iroh handshake still
        // settling, momentary route flap, blob-store warmup) recovers.
        // Without the grace, every receiver failing the first sweep before
        // any peer reaches Completed would gate itself and strand the
        // distribution until the next reconnect full-sync. Past the grace,
        // defer re-fetching until the mesh metadata (`node_statuses`) shows
        // another node holds the blob *complete* — a reachable source.
        // Blindly re-fetching every sweep against an unreachable sender
        // opens a connection attempt on the iroh endpoint shared with CRDT
        // sync, starving distribution-doc propagation (the 7n-dual-c2
        // failover stall). A Completed holder always re-enables the fetch.
        let completed_holder_exists = doc
            .node_statuses
            .iter()
            .any(|(id, s)| id.as_str() != own_short_id && s.status == TransferState::Completed);
        let prior_attempts = attempt_counts.get(&doc_id).copied().unwrap_or(0);
        if should_defer_fetch(prior_attempts, completed_holder_exists) {
            debug!(
                distribution_id = %doc.distribution_id,
                prior_attempts,
                "no completed holder after grace window; deferring re-fetch to avoid endpoint churn"
            );
            // NOT marked handled → revisited when a peer's Completed status
            // propagates via CRDT sync (or on the next tick).
            continue;
        }
        attempt_counts.insert(doc_id.clone(), prior_attempts + 1);

        // Write Transferring before fetching. The sender's progress
        // watcher re-reads the doc on each observer event and emits an
        // IN_PROGRESS frame to subscribers. Best-effort: a failure here
        // does not block the fetch — the worst case is the sender never
        // observes our in-flight state.
        if let Err(e) = write_receiver_status(
            document_store,
            &doc,
            own_short_id,
            ReceiverStatusWrite::Transferring,
        ) {
            warn!(
                distribution_id = %doc.distribution_id,
                error = %e,
                "failed to write Transferring node status; sender will see no in-progress frame"
            );
        }

        // Test fault/throttle seam (no-op in production). Consulted
        // after the Transferring write so the sender has already
        // observed IN_PROGRESS.
        match peek_receive_directive(&doc.blob_hash) {
            Some(ReceiveTestDirective::FailFetch(msg)) => {
                if let Err(e) = write_receiver_status(
                    document_store,
                    &doc,
                    own_short_id,
                    ReceiverStatusWrite::Failed(msg),
                ) {
                    warn!(
                        distribution_id = %doc.distribution_id,
                        error = %e,
                        "test seam: failed to write injected Failed node status"
                    );
                }
                handled.insert(doc_id);
                continue;
            }
            Some(ReceiveTestDirective::HoldInFlight) => {
                // Re-read (cheap): if the sender cancelled while we
                // were holding, stop — a receiver must not deliver a
                // cancelled distribution (basis of PRD test 24's
                // deterministic mid-flight cancel).
                match read_distribution_document(document_store.as_ref(), &doc.distribution_id) {
                    Ok(Some(fresh)) if fresh.status != "distributing" => {
                        debug!(
                            distribution_id = %doc.distribution_id,
                            status = %fresh.status,
                            "test seam: distribution no longer distributing; releasing hold"
                        );
                        handled.insert(doc_id);
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(
                            distribution_id = %doc.distribution_id,
                            error = %e,
                            "test seam: hold re-read failed; will retry next sweep"
                        );
                    }
                }
                // Skip fetch this sweep; NOT marked handled → revisited
                // next sweep, staying IN_PROGRESS. Non-blocking: other
                // distributions in this sweep proceed normally.
                continue;
            }
            None => {}
        }

        // Fetch the blob. `NetworkedIrohBlobStore::fetch_blob` iterates
        // known iroh peers internally. If the sender isn't yet
        // reachable (handshake still settling, transient network), the
        // call returns Err and we retry on the next sweep.
        let token = BlobToken {
            hash: BlobHash(doc.blob_hash.clone()),
            size_bytes: doc.blob_size,
            metadata: doc.blob_metadata.clone(),
        };
        let handle = match blob_store.fetch_blob(&token, |_| {}).await {
            Ok(h) => h,
            Err(e) => {
                debug!(
                    distribution_id = %doc.distribution_id,
                    error = %e,
                    "fetch_blob not yet succeeding; will retry next sweep"
                );
                continue;
            }
        };

        // Hand the bytes to the consumer's sink.
        match sink.deliver(&doc, &handle.path).await {
            Ok(()) => {
                info!(
                    distribution_id = %doc.distribution_id,
                    blob_hash = %doc.blob_hash,
                    size_bytes = doc.blob_size,
                    "attachment received and delivered to sink"
                );
                // Completed terminal status — the sender's watcher
                // observes this, emits one final frame with
                // completed=total_targets, and drops the broadcast
                // sender so subscribers see RecvError::Closed.
                if let Err(e) = write_receiver_status(
                    document_store,
                    &doc,
                    own_short_id,
                    ReceiverStatusWrite::Completed,
                ) {
                    warn!(
                        distribution_id = %doc.distribution_id,
                        error = %e,
                        "failed to write Completed node status; sender will see no terminal frame for this node"
                    );
                }
                handled.insert(doc_id);
            }
            Err(e) => {
                warn!(
                    distribution_id = %doc.distribution_id,
                    error = %e,
                    "sink delivery failed; will retry next sweep"
                );
                // No `handled.insert` — retry next sweep. No Failed
                // node-status either: retries are intentional and a
                // Failed flip would prematurely close the sender's
                // broadcast channel for this distribution.
            }
        }
    }
    Ok(())
}

/// Background task: poll synced distribution documents and deliver any
/// that target this node via `sink`. Mirrors the sender-side
/// [`watch_distribution_documents`] lifecycle (spawned + aborted on
/// drop) but for the receive path.
#[cfg(feature = "automerge-backend")]
async fn watch_receive_documents(
    document_store: Arc<AutomergeStore>,
    blob_store: Arc<NetworkedIrohBlobStore>,
    sink: Arc<dyn ReceiveSink>,
    own_short_id: String,
    originated: DistributionsMap,
    poll_interval: Duration,
) {
    info!(
        endpoint = %own_short_id,
        interval_secs = poll_interval.as_secs_f64(),
        "attachment receive watcher started"
    );
    let mut handled: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Per-distribution fetch-attempt counts (peat-mesh#137 / #226). After a
    // grace window, gates re-fetch on a Completed holder to avoid endpoint
    // churn — see `receive_sweep_once` / `should_defer_fetch`.
    let mut attempt_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut ticker = tokio::time::interval(poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Discard the immediate-first tick — the collection is empty at
    // startup; the first useful sweep is after at least one tick of
    // upstream sync.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if let Err(e) = receive_sweep_once(
            &document_store,
            &blob_store,
            &sink,
            &own_short_id,
            &originated,
            &mut handled,
            &mut attempt_counts,
        )
        .await
        {
            warn!(error = %e, "receive sweep failed; will retry next tick");
        }
    }
}

#[cfg(feature = "automerge-backend")]
#[async_trait::async_trait]
impl FileDistribution for IrohFileDistribution {
    async fn distribute(
        &self,
        blob_token: &BlobToken,
        scope: DistributionScope,
        priority: TransferPriority,
    ) -> Result<DistributionHandle> {
        info!(
            blob_hash = %blob_token.hash,
            blob_size = blob_token.size_bytes,
            scope = ?scope,
            priority = ?priority,
            "Starting file distribution"
        );

        // Create distribution handle
        let handle = DistributionHandle::new(blob_token.hash.clone(), scope.clone(), priority);

        // Resolve target nodes
        let target_nodes = self.resolve_targets(&scope).await;

        if target_nodes.is_empty() {
            warn!("No target nodes found for distribution scope");
        }

        // Create initial status
        let status =
            DistributionStatus::new(handle.clone(), target_nodes.clone(), blob_token.size_bytes);

        // Store distribution document (syncs to peers via Automerge)
        self.store_distribution_document(&handle, blob_token, &target_nodes)
            .await?;

        // Store status locally
        {
            let mut distributions = self.distributions.write().await;
            distributions.insert(handle.distribution_id.clone(), status.clone());
        }

        // Create progress channel
        {
            let (tx, _rx) = broadcast::channel(16);
            let mut channels = self.progress_channels.write().await;
            channels.insert(handle.distribution_id.clone(), tx);
        }

        info!(
            distribution_id = %handle.distribution_id,
            target_count = target_nodes.len(),
            "Distribution initiated - document synced to peers"
        );

        // Note: Actual blob transfer happens when target nodes:
        // 1. Receive the distribution document via Automerge sync
        // 2. See they are a target node
        // 3. Fetch the blob via NetworkedIrohBlobStore::fetch_blob()
        // 4. Update their status (not yet implemented - would require observer pattern)

        Ok(handle)
    }

    async fn status(&self, handle: &DistributionHandle) -> Result<DistributionStatus> {
        let distributions = self.distributions.read().await;
        distributions
            .get(&handle.distribution_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Distribution not found: {}", handle.distribution_id))
    }

    async fn cancel(&self, handle: &DistributionHandle) -> Result<()> {
        info!(
            distribution_id = %handle.distribution_id,
            "Cancelling distribution"
        );

        // Update status to cancelled and capture a terminal snapshot for subscribers.
        let cancelled_status = {
            let mut distributions = self.distributions.write().await;
            distributions
                .get_mut(&handle.distribution_id)
                .map(|status| {
                    for node_status in status.node_statuses.values_mut() {
                        if node_status.status != TransferState::Completed {
                            node_status.status = TransferState::Failed;
                            node_status.error = Some("Distribution cancelled".to_string());
                        }
                    }
                    status.recalculate_counts();
                    status.clone()
                })
        };

        // Publish the terminal frame and close the broadcast so subscribers see
        // a final status followed by RecvError::Closed.
        if let Some(status) = cancelled_status {
            self.broadcast_progress(&handle.distribution_id, &status)
                .await;
            let mut channels = self.progress_channels.write().await;
            channels.remove(&handle.distribution_id);
        }

        // Read-modify-write only on the `ROOT.metadata` byte-scalar (the
        // sender-owned half of the document). The `ROOT.node_statuses`
        // Automerge map is left strictly alone — under the rc.9 schema
        // the receivers own that map, and trampling their entries on
        // cancel would re-introduce the wholesale-overwrite failure mode
        // the typed schema exists to prevent. Receivers learn that the
        // distribution is cancelled via `status: "cancelled"` in the
        // metadata; their inbox watchers stop fetching on a non-
        // "distributing" status.
        use automerge::transaction::Transactable;
        use automerge::{ObjType, ReadDoc, ScalarValue, Value, ROOT};

        let key = distribution_doc_key(&handle.distribution_id);
        // Serialize cancel's read-modify-write against concurrent
        // receiver writes on the same doc; without this lock a
        // cancel's metadata flip could overwrite a receiver's
        // in-flight `node_statuses` write or vice versa.
        let _guard = self.document_store.lock_doc(&key);
        if let Some(mut doc) = self.document_store.get(&key)? {
            // Legacy `node_statuses` seeding accumulator. Populated only
            // when this is the first cancel after a rc.7/rc.8 → rc.9
            // upgrade (the `_` arm of the match below); applied inside
            // the same `doc.transact` so the metadata flip + legacy
            // node_statuses migration land in a single Automerge change.
            // Pre-serialize the legacy entries into `(receiver_key, bytes)`
            // pairs so the `doc.transact` closure can't fail on serde —
            // its error type is `automerge::AutomergeError` which has no
            // serde-error variant.
            let mut legacy_node_statuses_to_seed: Option<Vec<(String, Vec<u8>)>> = None;
            let new_metadata_bytes = match doc.get(ROOT, METADATA_FIELD)? {
                // rc.9 path
                Some((Value::Scalar(scalar), _)) => {
                    let bytes = match scalar.as_ref() {
                        ScalarValue::Bytes(b) => b.clone(),
                        other => {
                            return Err(anyhow::anyhow!(
                                "metadata field has unexpected scalar type {:?}",
                                other
                            ));
                        }
                    };
                    let mut metadata: DistributionMetadata = serde_json::from_slice(&bytes)
                        .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {}", e))?;
                    metadata.status = "cancelled".to_string();
                    metadata.cancelled_at = Some(Utc::now());
                    serde_json::to_vec(&metadata)
                        .map_err(|e| anyhow::anyhow!("Failed to serialize cancel update: {}", e))?
                }
                // Legacy rc.7/rc.8 doc with `ROOT.data`. Read the full
                // legacy structure, flip status to "cancelled", and
                // serialize the metadata half back as the rc.9
                // `ROOT.metadata` field. The legacy doc's `node_statuses`
                // entries are seeded into a fresh `ROOT.node_statuses`
                // typed Map below so receiver progress recorded under
                // the pre-rc.9 schema is preserved across the migration
                // — without this seeding, the first cancel after a
                // rc.7/rc.8 → rc.9 upgrade would silently drop every
                // receiver's status entry.
                _ => {
                    let legacy = distribution_document_from_automerge(&doc)?;
                    let migrated = DistributionMetadata {
                        distribution_id: legacy.distribution_id,
                        blob_hash: legacy.blob_hash,
                        blob_size: legacy.blob_size,
                        blob_metadata: legacy.blob_metadata,
                        scope: legacy.scope,
                        priority: legacy.priority,
                        target_nodes: legacy.target_nodes,
                        started_at: legacy.started_at,
                        status: "cancelled".to_string(),
                        cancelled_at: Some(Utc::now()),
                    };
                    // Pre-serialize each receiver's NodeTransferStatus
                    // here so the closure below only does infallible
                    // Automerge ops.
                    let mut pairs: Vec<(String, Vec<u8>)> =
                        Vec::with_capacity(legacy.node_statuses.len());
                    for (k, v) in &legacy.node_statuses {
                        let bytes = serde_json::to_vec(v).map_err(|e| {
                            anyhow::anyhow!(
                                "Failed to serialize legacy NodeTransferStatus during migration: {}",
                                e
                            )
                        })?;
                        pairs.push((k.clone(), bytes));
                    }
                    legacy_node_statuses_to_seed = Some(pairs);
                    serde_json::to_vec(&migrated).map_err(|e| {
                        anyhow::anyhow!("Failed to serialize migrated metadata: {}", e)
                    })?
                }
            };
            doc.transact::<_, _, automerge::AutomergeError>(|tx| {
                tx.put(
                    ROOT,
                    METADATA_FIELD,
                    ScalarValue::Bytes(new_metadata_bytes.clone()),
                )?;
                // Migration path: seed `ROOT.node_statuses` from the
                // legacy doc's embedded entries. Only runs once per
                // legacy doc (subsequent reads take the rc.9 path
                // because METADATA_FIELD is now present).
                if let Some(ref pairs) = legacy_node_statuses_to_seed {
                    let map_id = match tx.get(ROOT, NODE_STATUSES_FIELD)? {
                        Some((Value::Object(ObjType::Map), id)) => id,
                        _ => tx.put_object(ROOT, NODE_STATUSES_FIELD, ObjType::Map)?,
                    };
                    for (receiver_short_id, bytes) in pairs {
                        tx.put(
                            &map_id,
                            receiver_short_id.as_str(),
                            ScalarValue::Bytes(bytes.clone()),
                        )?;
                    }
                }
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("Automerge transact failed on cancel: {:?}", e))?;
            self.document_store.put(&key, &doc)?;
        }

        Ok(())
    }

    async fn wait_for_completion(
        &self,
        handle: &DistributionHandle,
        timeout: Duration,
    ) -> Result<DistributionStatus> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(500);

        loop {
            let status = self.status(handle).await?;

            if status.is_complete() {
                return Ok(status);
            }

            if start.elapsed() >= timeout {
                return Err(anyhow::anyhow!("Distribution timeout after {:?}", timeout));
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn subscribe_progress(
        &self,
        handle: &DistributionHandle,
    ) -> Result<broadcast::Receiver<DistributionStatus>> {
        let channels = self.progress_channels.read().await;
        channels
            .get(&handle.distribution_id)
            .map(|sender| sender.subscribe())
            .ok_or_else(|| anyhow::anyhow!("Distribution not found: {}", handle.distribution_id))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_priority_ordering() {
        assert!(TransferPriority::Critical.as_numeric() > TransferPriority::High.as_numeric());
        assert!(TransferPriority::High.as_numeric() > TransferPriority::Normal.as_numeric());
        assert!(TransferPriority::Normal.as_numeric() > TransferPriority::Low.as_numeric());
    }

    #[test]
    fn test_distribution_handle_creation() {
        let hash = BlobHash::from_hex("abc123");
        let scope = DistributionScope::AllNodes;
        let priority = TransferPriority::High;

        let handle = DistributionHandle::new(hash.clone(), scope, priority);

        assert!(!handle.distribution_id.is_empty());
        assert_eq!(handle.blob_hash, hash);
        assert_eq!(handle.priority, TransferPriority::High);
    }

    #[test]
    fn test_node_transfer_status() {
        let mut status = NodeTransferStatus::new("node-1".to_string(), 1000);

        assert_eq!(status.status, TransferState::Pending);
        assert_eq!(status.progress_fraction(), 0.0);

        status.progress_bytes = 500;
        status.status = TransferState::Transferring;
        assert_eq!(status.progress_fraction(), 0.5);

        status.progress_bytes = 1000;
        status.status = TransferState::Completed;
        assert_eq!(status.progress_fraction(), 1.0);
    }

    #[test]
    fn test_distribution_status() {
        let hash = BlobHash::from_hex("abc123");
        let handle =
            DistributionHandle::new(hash, DistributionScope::AllNodes, TransferPriority::Normal);
        let targets = vec![
            "node-1".to_string(),
            "node-2".to_string(),
            "node-3".to_string(),
        ];

        let mut status = DistributionStatus::new(handle, targets, 1000);

        assert_eq!(status.total_targets, 3);
        assert_eq!(status.completed, 0);
        assert!(!status.is_complete());

        // Simulate completion
        if let Some(node_status) = status.node_statuses.get_mut("node-1") {
            node_status.status = TransferState::Completed;
            node_status.progress_bytes = 1000;
        }
        if let Some(node_status) = status.node_statuses.get_mut("node-2") {
            node_status.status = TransferState::Completed;
            node_status.progress_bytes = 1000;
        }
        if let Some(node_status) = status.node_statuses.get_mut("node-3") {
            node_status.status = TransferState::Failed;
            node_status.error = Some("Connection lost".to_string());
        }

        status.recalculate_counts();

        assert_eq!(status.completed, 2);
        assert_eq!(status.failed, 1);
        assert!(status.is_complete());
        assert!(!status.is_success());
    }

    #[cfg(feature = "automerge-backend")]
    #[test]
    fn test_distribution_document_round_trip() {
        let mut node_statuses = HashMap::new();
        node_statuses.insert(
            "node-a".to_string(),
            NodeTransferStatus {
                node_id: "node-a".to_string(),
                status: TransferState::Completed,
                progress_bytes: 1024,
                total_bytes: 1024,
                started_at: None,
                completed_at: None,
                error: None,
            },
        );

        let doc = DistributionDocument {
            distribution_id: "dist-1".to_string(),
            blob_hash: "deadbeef".to_string(),
            blob_size: 1024,
            blob_metadata: BlobMetadata::default(),
            scope: DistributionScope::AllNodes,
            priority: TransferPriority::Normal,
            target_nodes: vec!["node-a".to_string()],
            started_at: Utc::now(),
            status: "distributing".to_string(),
            cancelled_at: None,
            node_statuses,
        };

        let bytes = serde_json::to_vec(&doc).expect("serialize");
        let restored: DistributionDocument = serde_json::from_slice(&bytes).expect("deserialize");

        assert_eq!(restored.distribution_id, "dist-1");
        assert_eq!(restored.target_nodes, vec!["node-a".to_string()]);
        assert_eq!(restored.node_statuses.len(), 1);
        assert_eq!(
            restored.node_statuses["node-a"].status,
            TransferState::Completed
        );
    }

    /// Documents written before #864 lacked `node_statuses` entirely. They
    /// must still deserialize so an in-flight distribution survives a
    /// peat-protocol upgrade.
    #[cfg(feature = "automerge-backend")]
    #[test]
    fn test_distribution_document_legacy_compat() {
        // Build a current doc, serialize it, then strip node_statuses to
        // mimic the pre-#864 wire format. The rest of the schema is
        // identical to what distribute() wrote before this change.
        let current = DistributionDocument {
            distribution_id: "dist-legacy".to_string(),
            blob_hash: "abc123".to_string(),
            blob_size: 42,
            blob_metadata: BlobMetadata::default(),
            scope: DistributionScope::AllNodes,
            priority: TransferPriority::Normal,
            target_nodes: vec!["node-x".to_string()],
            started_at: Utc::now(),
            status: "distributing".to_string(),
            cancelled_at: None,
            node_statuses: HashMap::new(),
        };
        let mut value = serde_json::to_value(&current).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("node_statuses")
            .expect("node_statuses present in current schema");

        let bytes = serde_json::to_vec(&value).unwrap();
        let restored: DistributionDocument = serde_json::from_slice(&bytes).expect("deserialize");

        assert_eq!(restored.distribution_id, "dist-legacy");
        assert!(restored.node_statuses.is_empty());
        assert!(restored.cancelled_at.is_none());
    }

    #[test]
    fn test_distribution_scope_serialization() {
        let scope = DistributionScope::Capable {
            min_gpu_gb: Some(4.0),
            cpu_arch: Some("x86_64".to_string()),
            min_storage_mb: Some(1024),
        };

        let json = serde_json::to_string(&scope).unwrap();
        let restored: DistributionScope = serde_json::from_str(&json).unwrap();

        match restored {
            DistributionScope::Capable {
                min_gpu_gb,
                cpu_arch,
                min_storage_mb,
            } => {
                assert_eq!(min_gpu_gb, Some(4.0));
                assert_eq!(cpu_arch, Some("x86_64".to_string()));
                assert_eq!(min_storage_mb, Some(1024));
            }
            _ => panic!("Wrong variant"),
        }
    }

    // peat-mesh#137 / #226: the inbox re-fetch gate. Locks in the gate
    // semantics (and the stranding-avoidance grace window) without needing
    // a live `NetworkedIrohBlobStore` (it's a concrete, un-mockable type),
    // by testing the policy predicate `should_defer_fetch` directly.
    #[cfg(feature = "automerge-backend")]
    #[test]
    fn fetch_gate_grace_window_then_completed_holder_gating() {
        // Within the grace window: ALWAYS fetch, holder or not. This is the
        // stranding-avoidance property — a transient early-sweep failure on
        // every receiver still gets retried, so the distribution can never
        // wedge before any peer reaches Completed.
        for n in 0..RECEIVE_FETCH_GRACE_ATTEMPTS {
            assert!(
                !should_defer_fetch(n, false),
                "attempt {n} (< grace) must fetch even with no completed holder"
            );
            assert!(
                !should_defer_fetch(n, true),
                "attempt {n} (< grace) must fetch"
            );
        }

        // Past the grace window with NO completed holder: defer (the
        // dual-C2 churn-avoidance property — stop hammering an unreachable
        // sender on the shared iroh endpoint).
        assert!(should_defer_fetch(RECEIVE_FETCH_GRACE_ATTEMPTS, false));
        assert!(should_defer_fetch(RECEIVE_FETCH_GRACE_ATTEMPTS + 10, false));

        // A Completed holder ALWAYS re-enables the fetch, even past grace —
        // so once a reachable peer holds the blob complete, the receiver
        // pulls from it and is never stranded.
        assert!(!should_defer_fetch(RECEIVE_FETCH_GRACE_ATTEMPTS, true));
        assert!(!should_defer_fetch(RECEIVE_FETCH_GRACE_ATTEMPTS + 10, true));
    }
}
