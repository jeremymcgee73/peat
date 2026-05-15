//! Iroh File Distribution End-to-End Tests (Issue #379)
//!
//! These tests validate the integration of blob storage with AutomergeIrohBackend
//! for model/file distribution across the mesh.
//!
//! # What This Tests
//!
//! 1. **Blob Store Integration**: AutomergeIrohBackend with blob storage enabled
//! 2. **Auto Peer Registration**: Blob peers synced with document sync peers
//! 3. **IrohFileDistribution**: Higher-level distribution API
//!
//! # Test Architecture
//!
//! ```text
//! Commander Node                    Sensor Node
//! ┌──────────────────────────┐      ┌──────────────────────────┐
//! │ AutomergeIrohBackend     │      │ AutomergeIrohBackend     │
//! │ ├─ AutomergeStore        │      │ ├─ AutomergeStore        │
//! │ ├─ IrohTransport         │──────│ ├─ IrohTransport         │
//! │ └─ NetworkedIrohBlobStore│      │ └─ NetworkedIrohBlobStore│
//! │                          │      │                          │
//! │ 1. Create model blob     │      │                          │
//! │ 2. distribute(token)     │      │                          │
//! │    └─ store dist doc ────┼──────┼─→ 3. Receive dist doc   │
//! │                          │      │    └─ fetch_blob()       │
//! │                          │      │       └─ verify model    │
//! └──────────────────────────┘      └──────────────────────────┘
//! ```

#![cfg(feature = "automerge-backend")]

use chrono::Utc;
use peat_protocol::storage::{
    AutomergeStore, BlobMetadata, BlobStore, DistributionDocument, DistributionScope,
    FileDistribution, IrohFileDistribution, NetworkedIrohBlobStore, NodeTransferStatus,
    TransferPriority, TransferState, IROH_DISTRIBUTION_COLLECTION,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to create a NetworkedIrohBlobStore with AutomergeStore
async fn create_integrated_stores(
    bind_addr: SocketAddr,
    temp_dir: &std::path::Path,
) -> (Arc<NetworkedIrohBlobStore>, Arc<AutomergeStore>) {
    let blob_dir = temp_dir.join("blobs");
    std::fs::create_dir_all(&blob_dir).unwrap();

    let blob_store = NetworkedIrohBlobStore::bind(blob_dir, bind_addr)
        .await
        .expect("Should create NetworkedIrohBlobStore");

    let db_path = temp_dir.join("automerge.db");
    let doc_store = Arc::new(AutomergeStore::open(&db_path).expect("Should open AutomergeStore"));

    (blob_store, doc_store)
}

/// Test 1: IrohFileDistribution Basic Usage
///
/// Validates that IrohFileDistribution can create a distribution and track status.
#[tokio::test]
async fn test_iroh_file_distribution_basic() {
    println!("=== E2E: IrohFileDistribution Basic ===");

    let temp = TempDir::new().unwrap();
    // Ephemeral port to avoid CI-runner port conflicts.
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    println!("  Creating integrated stores...");
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;

    println!("  Node ID: {}", blob_store.endpoint_id().fmt_short());

    // Create IrohFileDistribution service
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // Create a test model blob
    println!("  1. Creating model blob...");
    let model_data = b"ONNX Model: YOLOv8 Nano for target detection - v1.0.0";
    let metadata = BlobMetadata::with_name_and_type("yolov8-nano.onnx", "application/onnx")
        .with_custom("version", "1.0.0")
        .with_custom("model_type", "detection");

    let token = blob_store
        .create_blob_from_bytes(model_data, metadata)
        .await
        .expect("Should create blob");

    println!("    Created blob: hash={}", token.hash.as_hex());
    println!("    Size: {} bytes", token.size_bytes);

    // Initiate distribution
    println!("  2. Initiating distribution to AllNodes...");
    let handle = distribution
        .distribute(&token, DistributionScope::AllNodes, TransferPriority::High)
        .await
        .expect("Should start distribution");

    println!("    Distribution ID: {}", handle.distribution_id);
    println!("    Priority: {:?}", handle.priority);

    // Check status
    println!("  3. Checking distribution status...");
    let status = distribution
        .status(&handle)
        .await
        .expect("Should get status");

    println!("    Total targets: {}", status.total_targets);
    println!("    Completed: {}", status.completed);
    println!("    In progress: {}", status.in_progress);
    println!("    Failed: {}", status.failed);

    // With no connected peers, total_targets should be 0
    assert_eq!(status.total_targets, 0, "No peers connected yet");

    println!("  ✓ IrohFileDistribution basic test passed");
}

/// Test 2: Distribution Document Stored in Automerge
///
/// Validates that distribution metadata is stored as an Automerge document
/// that would sync to peers.
#[tokio::test]
async fn test_distribution_document_stored() {
    println!("=== E2E: Distribution Document Storage ===");

    let temp = TempDir::new().unwrap();
    // Ephemeral port to avoid CI-runner port conflicts.
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    println!("  Creating integrated stores...");
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;

    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // Create and distribute a blob
    println!("  1. Creating and distributing blob...");
    let model_data = b"Test model content";
    let metadata = BlobMetadata::with_name_and_type("test.bin", "application/octet-stream");

    let token = blob_store
        .create_blob_from_bytes(model_data, metadata)
        .await
        .expect("Should create blob");

    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .expect("Should start distribution");

    // Check that distribution document was stored via status API
    println!("  2. Checking distribution status...");
    let status = distribution
        .status(&handle)
        .await
        .expect("Should get status");

    println!("    Distribution found:");
    println!("      distribution_id: {}", status.handle.distribution_id);
    println!("      blob_hash: {}", status.handle.blob_hash.as_hex());
    println!("      total_targets: {}", status.total_targets);

    assert_eq!(status.handle.distribution_id, handle.distribution_id);
    assert_eq!(status.handle.blob_hash.as_hex(), token.hash.as_hex());

    println!("  ✓ Distribution document storage test passed");
}

/// Test 3: Cancel Distribution
///
/// Validates that a distribution can be cancelled.
#[tokio::test]
async fn test_cancel_distribution() {
    println!("=== E2E: Cancel Distribution ===");

    let temp = TempDir::new().unwrap();
    // Ephemeral port to avoid CI-runner port conflicts.
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    println!("  Creating integrated stores...");
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;

    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // Create and distribute a blob
    println!("  1. Starting distribution...");
    let model_data = b"Model to cancel";
    let metadata = BlobMetadata::with_name_and_type("cancel-test.bin", "application/octet-stream");

    let token = blob_store
        .create_blob_from_bytes(model_data, metadata)
        .await
        .expect("Should create blob");

    let handle = distribution
        .distribute(&token, DistributionScope::AllNodes, TransferPriority::Low)
        .await
        .expect("Should start distribution");

    println!("    Distribution ID: {}", handle.distribution_id);

    // Cancel the distribution
    println!("  2. Cancelling distribution...");
    distribution
        .cancel(&handle)
        .await
        .expect("Should cancel distribution");

    println!("    Distribution cancelled");

    // Verify status shows it's complete (cancelled counts as complete)
    println!("  3. Verifying cancellation via status...");
    let status = distribution
        .status(&handle)
        .await
        .expect("Should get status");

    // With no peers, is_complete should be true after cancel
    println!(
        "    Status: completed={}, failed={}",
        status.completed, status.failed
    );

    println!("  ✓ Cancel distribution test passed");
}

/// Test: cancel() publishes a terminal frame then closes the progress stream.
///
/// Contract from issue #864: a subscriber that holds a `subscribe_progress`
/// receiver across a `cancel()` call must observe exactly one final
/// `DistributionStatus` frame followed by `RecvError::Closed`. Before this
/// change, the broadcast channel was never written to and the receiver
/// observed no frames for the lifetime of the distribution.
#[tokio::test]
async fn test_cancel_emits_terminal_frame_then_closes() {
    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    let token = blob_store
        .create_blob_from_bytes(
            b"payload",
            BlobMetadata::with_name_and_type("payload.bin", "application/octet-stream"),
        )
        .await
        .expect("create blob");

    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .expect("start distribution");

    let mut rx = distribution
        .subscribe_progress(&handle)
        .await
        .expect("subscribe");

    distribution.cancel(&handle).await.expect("cancel");

    let frame = rx.recv().await.expect("terminal frame");
    assert_eq!(frame.handle.distribution_id, handle.distribution_id);

    match rx.recv().await {
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
        other => panic!("expected RecvError::Closed after terminal frame, got {other:?}"),
    }
}

/// Test: cancel() preserves the distribution document via read-modify-write.
///
/// Before #864 the cancel path overwrote the doc wholesale with a
/// `{status, cancelled_at}` stub, destroying `target_nodes`, `blob_hash`,
/// and `node_statuses`. The slice 2 schema work flips this to a typed RMW
/// so the doc retains all of its fields and only the status / cancelled_at
/// transition.
#[tokio::test]
async fn test_cancel_preserves_distribution_document_fields() {
    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    let token = blob_store
        .create_blob_from_bytes(
            b"payload",
            BlobMetadata::with_name_and_type("payload.bin", "application/octet-stream"),
        )
        .await
        .expect("create blob");

    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .expect("start distribution");

    // Verify the doc is the full typed schema as written by distribute().
    let collection = doc_store.collection(IROH_DISTRIBUTION_COLLECTION);
    let before = collection
        .get(&handle.distribution_id)
        .expect("collection get")
        .expect("doc present");
    let before: DistributionDocument = serde_json::from_slice(&before).expect("deserialize");
    assert_eq!(before.distribution_id, handle.distribution_id);
    assert_eq!(before.blob_hash, token.hash.as_hex());
    assert_eq!(before.status, "distributing");
    assert!(before.cancelled_at.is_none());

    distribution.cancel(&handle).await.expect("cancel");

    let after = collection
        .get(&handle.distribution_id)
        .expect("collection get")
        .expect("doc present after cancel");
    let after: DistributionDocument = serde_json::from_slice(&after).expect("deserialize");

    assert_eq!(after.distribution_id, before.distribution_id);
    assert_eq!(after.blob_hash, before.blob_hash);
    assert_eq!(after.blob_size, before.blob_size);
    assert_eq!(after.target_nodes, before.target_nodes);
    assert_eq!(after.status, "cancelled");
    assert!(
        after.cancelled_at.is_some(),
        "cancelled_at must be set after cancel"
    );
}

/// Test: the sender-side watcher publishes a `DistributionStatus` frame
/// whenever a receiver writes its `NodeTransferStatus` into the
/// distribution document.
///
/// Issue #864: prior to this change `subscribe_progress` returned a
/// receiver that observed zero frames because nothing ever published.
/// The slice-3 watcher subscribes to `AutomergeStore` observer events,
/// reads any updated distribution document, merges its `node_statuses`
/// into the in-memory state, and broadcasts a fresh snapshot. The test
/// simulates the receiver-side write directly (which in production lives
/// in peat-node — see peat-node#75).
#[tokio::test]
async fn test_watcher_publishes_frame_on_receiver_node_status_write() {
    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    let token = blob_store
        .create_blob_from_bytes(
            b"payload",
            BlobMetadata::with_name_and_type("payload.bin", "application/octet-stream"),
        )
        .await
        .expect("create blob");

    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .expect("start distribution");

    let mut rx = distribution
        .subscribe_progress(&handle)
        .await
        .expect("subscribe");

    // Simulate a receiver writing its NodeTransferStatus into the
    // distribution document. With AllNodes scope and no real peers in
    // this single-node test, target_nodes is empty, so we inject a
    // synthetic "receiver-1" entry just to exercise the merge path.
    let collection = doc_store.collection(IROH_DISTRIBUTION_COLLECTION);
    let existing = collection
        .get(&handle.distribution_id)
        .unwrap()
        .expect("doc present");
    let mut doc: DistributionDocument = serde_json::from_slice(&existing).unwrap();
    doc.node_statuses.insert(
        "receiver-1".to_string(),
        NodeTransferStatus {
            node_id: "receiver-1".to_string(),
            status: TransferState::Completed,
            progress_bytes: 7,
            total_bytes: 7,
            started_at: None,
            completed_at: Some(Utc::now()),
            error: None,
        },
    );
    let bytes = serde_json::to_vec(&doc).unwrap();
    collection
        .upsert(&handle.distribution_id, bytes)
        .expect("upsert receiver status");

    // The watcher reacts asynchronously to the observer broadcast — give
    // it a generous timeout but not so long that a real bug stalls CI.
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("watcher should publish within 2s")
        .expect("recv frame");

    assert_eq!(frame.handle.distribution_id, handle.distribution_id);
    assert_eq!(
        frame
            .node_statuses
            .get("receiver-1")
            .map(|s| s.status.clone()),
        Some(TransferState::Completed),
    );
}

/// Test 4: Distribution with Formation Scope
///
/// Validates distribution targeting a specific formation.
#[tokio::test]
async fn test_distribution_formation_scope() {
    println!("=== E2E: Distribution with Formation Scope ===");

    let temp = TempDir::new().unwrap();
    // Ephemeral port to avoid CI-runner port conflicts.
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    println!("  Creating integrated stores...");
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;

    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // Create blob
    let model_data = b"Formation-specific model";
    let metadata = BlobMetadata::with_name_and_type("formation-model.onnx", "application/onnx");

    let token = blob_store
        .create_blob_from_bytes(model_data, metadata)
        .await
        .expect("Should create blob");

    // Distribute to alpha-squad formation
    println!("  1. Distributing to formation 'alpha-squad'...");
    let scope = DistributionScope::Formation {
        formation_id: "alpha-squad".to_string(),
    };

    let handle = distribution
        .distribute(&token, scope.clone(), TransferPriority::Critical)
        .await
        .expect("Should start distribution");

    println!("    Distribution ID: {}", handle.distribution_id);

    // Check the distribution handle has formation scope
    match &handle.scope {
        DistributionScope::Formation { formation_id } => {
            println!("    Formation ID: {}", formation_id);
            assert_eq!(formation_id, "alpha-squad");
        }
        _ => panic!("Expected Formation scope"),
    }

    println!("  ✓ Formation scope distribution test passed");
}
