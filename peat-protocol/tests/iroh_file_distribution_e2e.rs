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
    read_distribution_document, write_receiver_node_status, AutomergeStore, BlobMetadata,
    BlobStore, DistributionScope, FileDistribution, IrohFileDistribution, NetworkedIrohBlobStore,
    NodeTransferStatus, TransferPriority, TransferState,
};
use std::collections::HashMap;
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
    let before = peat_protocol::storage::read_distribution_document(
        doc_store.as_ref(),
        &handle.distribution_id,
    )
    .expect("read distribution doc")
    .expect("doc present");
    assert_eq!(before.distribution_id, handle.distribution_id);
    assert_eq!(before.blob_hash, token.hash.as_hex());
    assert_eq!(before.status, "distributing");
    assert!(before.cancelled_at.is_none());

    distribution.cancel(&handle).await.expect("cancel");

    let after = peat_protocol::storage::read_distribution_document(
        doc_store.as_ref(),
        &handle.distribution_id,
    )
    .expect("read distribution doc")
    .expect("doc present after cancel");

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
    let ns = NodeTransferStatus {
        node_id: "receiver-1".to_string(),
        status: TransferState::Completed,
        progress_bytes: 7,
        total_bytes: 7,
        started_at: None,
        completed_at: Some(Utc::now()),
        error: None,
    };
    peat_protocol::storage::write_receiver_node_status(
        doc_store.as_ref(),
        &handle.distribution_id,
        "receiver-1",
        &ns,
    )
    .expect("write receiver status");

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

    // Distribute to alpha-cell formation
    println!("  1. Distributing to formation 'alpha-cell'...");
    let scope = DistributionScope::Formation {
        formation_id: "alpha-cell".to_string(),
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
            assert_eq!(formation_id, "alpha-cell");
        }
        _ => panic!("Expected Formation scope"),
    }

    println!("  ✓ Formation scope distribution test passed");
}

/// Substrate regression for defenseunicorns/peat#864 round 2: concurrent
/// receivers writing their own `NodeTransferStatus` into the distribution
/// document must NOT overwrite each other.
///
/// The pre-rc.9 schema stored `node_statuses` inside the wholesale-scalar
/// `ROOT.data` field, so two receivers calling `Collection::upsert` (or
/// the legacy read-modify-write helpers) would both write the SAME field,
/// and Automerge's actor-id tiebreak would pick one of the two — losing
/// the other receiver's status entirely. On CI hardware this manifested
/// as the receiver-local doc reverting to a stale state.
///
/// The rc.9 schema stores `node_statuses` as a typed `ObjType::Map` at
/// `ROOT.node_statuses`. Each receiver writes only to its own keyed
/// entry (`peer.fmt_short()`), so concurrent writes from different
/// receivers target *different* Automerge fields and never compete.
///
/// This test exercises that property directly: two synthetic receivers
/// write their own `Completed` entries against the same distribution
/// doc, and both entries must be present after the writes settle. A
/// regression to the wholesale-scalar pattern fails the second
/// assertion deterministically.
///
/// (Note: this is the *structural* property — different keys don't
/// collide on the merge tiebreak. `test_concurrent_receiver_writes_via_spawn`
/// below stress-tests the same property with actually-concurrent
/// `tokio::spawn` writers + `join`, pinning against a future
/// regression where a doc-level lock serializes everything through a
/// single actor-id.)
#[tokio::test]
async fn test_per_receiver_keys_dont_collide() {
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

    let receiver_a = NodeTransferStatus {
        node_id: "recv-a-short".to_string(),
        status: TransferState::Completed,
        progress_bytes: 7,
        total_bytes: 7,
        started_at: None,
        completed_at: Some(Utc::now()),
        error: None,
    };
    let receiver_b = NodeTransferStatus {
        node_id: "recv-b-short".to_string(),
        status: TransferState::Completed,
        progress_bytes: 7,
        total_bytes: 7,
        started_at: None,
        completed_at: Some(Utc::now()),
        error: None,
    };

    // Two receivers write to their own keys. Order is interleaved so a
    // wholesale-replacement regression would have either A or B win
    // depending on the actor-id tiebreak — never both.
    write_receiver_node_status(
        doc_store.as_ref(),
        &handle.distribution_id,
        "recv-a-short",
        &receiver_a,
    )
    .expect("write receiver A");
    write_receiver_node_status(
        doc_store.as_ref(),
        &handle.distribution_id,
        "recv-b-short",
        &receiver_b,
    )
    .expect("write receiver B");

    let doc = read_distribution_document(doc_store.as_ref(), &handle.distribution_id)
        .expect("read distribution doc")
        .expect("doc present");

    // BOTH receiver entries must be present. Pre-rc.9 wholesale-scalar
    // schema would have lost one of them at the Automerge merge layer.
    assert_eq!(
        doc.node_statuses.len(),
        2,
        "expected both receiver entries present, got {} entries: {:?}",
        doc.node_statuses.len(),
        doc.node_statuses.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        doc.node_statuses
            .get("recv-a-short")
            .map(|s| s.status.clone()),
        Some(TransferState::Completed),
        "receiver A must be Completed",
    );
    assert_eq!(
        doc.node_statuses
            .get("recv-b-short")
            .map(|s| s.status.clone()),
        Some(TransferState::Completed),
        "receiver B must be Completed",
    );

    // Sender's immutable metadata must survive the per-receiver writes
    // unchanged — the typed-schema split guarantees ROOT.metadata and
    // ROOT.node_statuses don't compete.
    assert_eq!(doc.status, "distributing");
    assert_eq!(doc.blob_hash, token.hash.as_hex());
    assert!(doc.cancelled_at.is_none());
}

/// Same regression as above but for the sequential-write case from a
/// single receiver: writing `Transferring` then `Completed` to the same
/// key must converge on `Completed`, with no merge-tiebreak race that
/// reverts to `Transferring`.
///
/// Pre-rc.9 the load-modify-write cycle on the wholesale-scalar `data`
/// field could lose to a concurrent inbound sync carrying older state.
/// rc.9 per-key map writes are still load-modify-write at the Automerge
/// level, but the *target field* is `node_statuses[receiver_short_id]`
/// rather than the wholesale `data` blob — so the receiver's two
/// sequential writes are causally ordered (each load includes the
/// prior put in its history) and the second `put` correctly replaces
/// the first.
#[tokio::test]
async fn test_sequential_receiver_writes_converge_on_latest() {
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

    let transferring = NodeTransferStatus {
        node_id: "recv-1".to_string(),
        status: TransferState::Transferring,
        progress_bytes: 0,
        total_bytes: 100,
        started_at: Some(Utc::now()),
        completed_at: None,
        error: None,
    };
    let completed = NodeTransferStatus {
        node_id: "recv-1".to_string(),
        status: TransferState::Completed,
        progress_bytes: 100,
        total_bytes: 100,
        started_at: transferring.started_at,
        completed_at: Some(Utc::now()),
        error: None,
    };

    write_receiver_node_status(
        doc_store.as_ref(),
        &handle.distribution_id,
        "recv-1",
        &transferring,
    )
    .expect("write Transferring");
    write_receiver_node_status(
        doc_store.as_ref(),
        &handle.distribution_id,
        "recv-1",
        &completed,
    )
    .expect("write Completed");

    let doc = read_distribution_document(doc_store.as_ref(), &handle.distribution_id)
        .expect("read distribution doc")
        .expect("doc present");

    let entry = doc
        .node_statuses
        .get("recv-1")
        .expect("recv-1 entry must be present");
    assert_eq!(
        entry.status,
        TransferState::Completed,
        "second sequential write must win — pre-rc.9 wholesale-scalar \
         schema could have left this at Transferring under merge-tiebreak \
         race against concurrent sync"
    );
    assert_eq!(entry.progress_bytes, 100);
    assert!(entry.completed_at.is_some());
}

/// Cross-version (legacy rc.7/rc.8) read compat: a rc.9 reader must
/// decode a doc written under the pre-rc.9 wholesale-scalar
/// `ROOT.data` schema (the case where a rc.7/rc.8 peer wrote the doc
/// before everyone upgraded). The CHANGELOG promises this; without a
/// test a future refactor could silently break the legacy fallback
/// while every typed-schema test stays green.
#[tokio::test]
async fn test_legacy_data_field_read_compat() {
    use automerge::transaction::Transactable;
    use automerge::{Automerge, ScalarValue, ROOT};
    use peat_protocol::storage::{
        DistributionDocument, DistributionScope as Scope, TransferPriority as Prio,
    };

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("automerge.db");
    let doc_store = Arc::new(AutomergeStore::open(&db_path).expect("Should open AutomergeStore"));

    let mut legacy_node_statuses = HashMap::new();
    legacy_node_statuses.insert(
        "legacy-recv".to_string(),
        NodeTransferStatus {
            node_id: "legacy-recv".to_string(),
            status: TransferState::Completed,
            progress_bytes: 42,
            total_bytes: 42,
            started_at: None,
            completed_at: Some(Utc::now()),
            error: None,
        },
    );

    let legacy_doc = DistributionDocument {
        distribution_id: "legacy-dist-1".to_string(),
        blob_hash: "deadbeef".to_string(),
        blob_size: 42,
        blob_metadata: BlobMetadata::with_name_and_type("legacy.bin", "application/octet-stream"),
        scope: Scope::AllNodes,
        priority: Prio::Normal,
        target_nodes: vec!["legacy-recv".to_string()],
        started_at: Utc::now(),
        status: "distributing".to_string(),
        cancelled_at: None,
        collection: None,
        node_statuses: legacy_node_statuses,
    };
    let legacy_bytes = serde_json::to_vec(&legacy_doc).expect("serialize legacy");

    // Construct an Automerge doc with the pre-rc.9 wholesale-scalar shape.
    let mut am_doc = Automerge::new();
    am_doc
        .transact::<_, _, automerge::AutomergeError>(|tx| {
            tx.put(ROOT, "data", ScalarValue::Bytes(legacy_bytes))?;
            Ok(())
        })
        .expect("transact legacy data");
    let key = format!("file_distributions:{}", legacy_doc.distribution_id);
    doc_store.put(&key, &am_doc).expect("put legacy doc");

    // rc.9 reader must decode it.
    let read = read_distribution_document(doc_store.as_ref(), &legacy_doc.distribution_id)
        .expect("read legacy doc")
        .expect("doc present");
    assert_eq!(read.distribution_id, legacy_doc.distribution_id);
    assert_eq!(read.blob_hash, legacy_doc.blob_hash);
    assert_eq!(read.target_nodes, legacy_doc.target_nodes);
    assert_eq!(read.status, "distributing");
    assert_eq!(
        read.node_statuses.len(),
        1,
        "legacy node_statuses entry must round-trip"
    );
    assert_eq!(
        read.node_statuses
            .get("legacy-recv")
            .map(|s| s.status.clone()),
        Some(TransferState::Completed),
        "legacy receiver entry must read as Completed"
    );
}

/// Cancel-path migration of a legacy rc.7/rc.8 doc must preserve the
/// `node_statuses` entries the legacy doc carried — pre-fix the
/// migration silently dropped them because the new `ROOT.metadata`
/// field carries only the sender-immutable half. With the seeding
/// fix, the migration writes `ROOT.node_statuses` as a typed Map
/// populated from the legacy embedded entries in the same
/// transaction.
#[tokio::test]
async fn test_cancel_legacy_doc_preserves_node_statuses() {
    use automerge::transaction::Transactable;
    use automerge::{Automerge, ScalarValue, ROOT};
    use peat_protocol::storage::{
        DistributionDocument, DistributionScope as Scope, TransferPriority as Prio,
    };

    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // Seed a legacy doc directly with `ROOT.data` and an embedded
    // node_statuses entry the migration must preserve.
    let dist_id = "legacy-cancel-1".to_string();
    let mut ns = HashMap::new();
    ns.insert(
        "recv-X".to_string(),
        NodeTransferStatus {
            node_id: "recv-X".to_string(),
            status: TransferState::Transferring,
            progress_bytes: 25,
            total_bytes: 100,
            started_at: Some(Utc::now()),
            completed_at: None,
            error: None,
        },
    );
    let legacy = DistributionDocument {
        distribution_id: dist_id.clone(),
        blob_hash: "facade".to_string(),
        blob_size: 100,
        blob_metadata: BlobMetadata::with_name_and_type(
            "legacy-cancel.bin",
            "application/octet-stream",
        ),
        scope: Scope::AllNodes,
        priority: Prio::Normal,
        target_nodes: vec!["recv-X".to_string()],
        started_at: Utc::now(),
        status: "distributing".to_string(),
        cancelled_at: None,
        collection: None,
        node_statuses: ns,
    };
    let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
    let mut am = Automerge::new();
    am.transact::<_, _, automerge::AutomergeError>(|tx| {
        tx.put(ROOT, "data", ScalarValue::Bytes(legacy_bytes))?;
        Ok(())
    })
    .unwrap();
    doc_store
        .put(&format!("file_distributions:{}", dist_id), &am)
        .unwrap();

    // The cancel path expects an `IrohFileDistribution` with the
    // distribution registered in-memory (the watcher's `distributions`
    // map). Replicate what `distribute()` does for that record: insert
    // a status entry so cancel doesn't no-op on the in-memory miss.
    use peat_protocol::storage::DistributionHandle;
    let handle = DistributionHandle {
        distribution_id: dist_id.clone(),
        blob_hash: peat_protocol::storage::BlobHash("facade".to_string()),
        scope: Scope::AllNodes,
        priority: Prio::Normal,
        started_at: Utc::now(),
    };
    // The in-memory `distributions` map is private to
    // `IrohFileDistribution`; cancel() is robust to the entry not
    // existing locally — it falls through to the doc-store branch we
    // care about. So we just call `cancel(&handle)` directly. The
    // branch that writes `ROOT.metadata` runs regardless.
    distribution
        .cancel(&handle)
        .await
        .expect("cancel migration");

    // Read back: status must be cancelled, and `recv-X`'s entry must
    // be present in `node_statuses` with the same fields the legacy
    // doc carried.
    let post = read_distribution_document(doc_store.as_ref(), &dist_id)
        .expect("read after cancel")
        .expect("doc present");
    assert_eq!(post.status, "cancelled");
    assert!(post.cancelled_at.is_some());
    let entry = post
        .node_statuses
        .get("recv-X")
        .expect("legacy receiver entry must be preserved across cancel migration");
    assert_eq!(entry.status, TransferState::Transferring);
    assert_eq!(entry.progress_bytes, 25);
    assert_eq!(entry.total_bytes, 100);
}

/// `scan_distribution_documents` returns every well-formed entry and
/// skips deliberately-malformed entries at `debug!` rather than
/// failing the whole scan. peat-node's inbox watcher depends on this:
/// a single corrupted doc must not halt iteration over the rest.
#[tokio::test]
async fn test_scan_distribution_documents_skips_malformed() {
    use automerge::transaction::Transactable;
    use automerge::{Automerge, ScalarValue, ROOT};

    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // Two well-formed rc.9 docs.
    let token_a = blob_store
        .create_blob_from_bytes(b"a", BlobMetadata::with_name_and_type("a.bin", "x"))
        .await
        .unwrap();
    let token_b = blob_store
        .create_blob_from_bytes(b"b", BlobMetadata::with_name_and_type("b.bin", "x"))
        .await
        .unwrap();
    let handle_a = distribution
        .distribute(
            &token_a,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .unwrap();
    let handle_b = distribution
        .distribute(
            &token_b,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .unwrap();

    // One malformed entry under the prefix — metadata field present but
    // not deserializable as DistributionMetadata.
    let mut bad = Automerge::new();
    bad.transact::<_, _, automerge::AutomergeError>(|tx| {
        tx.put(
            ROOT,
            "metadata",
            ScalarValue::Bytes(b"not-valid-json".to_vec()),
        )?;
        Ok(())
    })
    .unwrap();
    doc_store
        .put("file_distributions:malformed-1", &bad)
        .unwrap();

    let docs = peat_protocol::storage::scan_distribution_documents(doc_store.as_ref())
        .expect("scan must succeed even with one bad entry");
    let ids: Vec<&str> = docs.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        ids.contains(&handle_a.distribution_id.as_str()),
        "doc A must be present, got {:?}",
        ids
    );
    assert!(
        ids.contains(&handle_b.distribution_id.as_str()),
        "doc B must be present, got {:?}",
        ids
    );
    assert!(
        !ids.contains(&"malformed-1"),
        "malformed entry must be skipped, got {:?}",
        ids
    );
    assert_eq!(docs.len(), 2, "expected exactly 2 well-formed docs");
}

/// Stress version of `test_per_receiver_keys_dont_collide` that actually
/// drives the writes from concurrently-spawned tasks rather than
/// sequentially. Pins against a future regression where a doc-level
/// lock serializes everything through a single actor-id and silently
/// converts the per-key map back into a single-writer queue (which
/// would still pass the sequential structural test).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_receiver_writes_via_spawn() {
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
        .unwrap();
    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .unwrap();

    // Spawn N concurrent writers, each writing to its own
    // `recv-<i>` key. The key-isolation property must hold under
    // parallel pressure, not just sequential calls.
    const N: usize = 8;
    let mut tasks = Vec::with_capacity(N);
    for i in 0..N {
        let dist_id = handle.distribution_id.clone();
        let store = Arc::clone(&doc_store);
        tasks.push(tokio::spawn(async move {
            let key = format!("recv-{i}");
            let ns = NodeTransferStatus {
                node_id: key.clone(),
                status: TransferState::Completed,
                progress_bytes: i as u64,
                total_bytes: N as u64,
                started_at: None,
                completed_at: Some(Utc::now()),
                error: None,
            };
            write_receiver_node_status(store.as_ref(), &dist_id, &key, &ns)
                .expect("write receiver under concurrent load");
        }));
    }
    for t in tasks {
        t.await.expect("writer task panicked");
    }

    let doc = read_distribution_document(doc_store.as_ref(), &handle.distribution_id)
        .expect("read")
        .expect("doc present");
    assert_eq!(
        doc.node_statuses.len(),
        N,
        "all {N} concurrent receiver entries must survive; got {} entries: {:?}",
        doc.node_statuses.len(),
        doc.node_statuses.keys().collect::<Vec<_>>()
    );
    for i in 0..N {
        let key = format!("recv-{i}");
        let entry = doc
            .node_statuses
            .get(&key)
            .unwrap_or_else(|| panic!("missing concurrent-writer key {key}"));
        assert_eq!(entry.status, TransferState::Completed);
        assert_eq!(entry.progress_bytes, i as u64);
    }
}

/// [BLOCKER] regression (peat#868 QA round 3): a rc.9 receiver writing
/// against a NOT-yet-migrated legacy (`ROOT.data`) doc must still be
/// visible through `read_distribution_document`.
///
/// The cross-version window: a rc.7/rc.8 sender published a
/// distribution (stored as `ROOT.data` wholesale-scalar). Before the
/// doc is migrated forward (migration only happens on the sender's
/// `cancel` path), a rc.9 receiver calls `write_receiver_node_status`,
/// which lands the entry in the typed `ROOT.node_statuses` Map next
/// to — not inside — the legacy `ROOT.data` blob.
///
/// Pre-fix, `read_distribution_document` returned early from the
/// legacy branch and never consulted `ROOT.node_statuses`, so the
/// receiver's status was invisible to the sender's watcher — the
/// exact #864 failure mode, re-introduced for any distribution in
/// flight across the upgrade. Post-fix the legacy read overlays the
/// typed map on top of the legacy embedded node_statuses.
#[tokio::test]
async fn test_legacy_doc_with_rc9_receiver_write_is_visible() {
    use automerge::transaction::Transactable;
    use automerge::{Automerge, ScalarValue, ROOT};
    use peat_protocol::storage::{
        DistributionDocument, DistributionScope as Scope, TransferPriority as Prio,
    };

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("automerge.db");
    let doc_store = Arc::new(AutomergeStore::open(&db_path).expect("Should open AutomergeStore"));

    // 1. rc.7/rc.8 sender publishes — legacy `ROOT.data` shape, empty
    //    node_statuses (sender publishes before any receiver acts).
    let dist_id = "xver-1".to_string();
    let legacy = DistributionDocument {
        distribution_id: dist_id.clone(),
        blob_hash: "c0ffee".to_string(),
        blob_size: 64,
        blob_metadata: BlobMetadata::with_name_and_type("xver.bin", "application/octet-stream"),
        scope: Scope::AllNodes,
        priority: Prio::Normal,
        target_nodes: vec!["recv-xver".to_string()],
        started_at: Utc::now(),
        status: "distributing".to_string(),
        cancelled_at: None,
        collection: None,
        node_statuses: HashMap::new(),
    };
    let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
    let mut am = Automerge::new();
    am.transact::<_, _, automerge::AutomergeError>(|tx| {
        tx.put(ROOT, "data", ScalarValue::Bytes(legacy_bytes))?;
        Ok(())
    })
    .unwrap();
    let key = format!("file_distributions:{dist_id}");
    doc_store.put(&key, &am).unwrap();

    // 2. rc.9 receiver writes its status (no migration has happened —
    //    the doc still has `ROOT.data`, no `ROOT.metadata`).
    let recv_status = NodeTransferStatus {
        node_id: "recv-xver".to_string(),
        status: TransferState::Completed,
        progress_bytes: 64,
        total_bytes: 64,
        started_at: Some(Utc::now()),
        completed_at: Some(Utc::now()),
        error: None,
    };
    write_receiver_node_status(doc_store.as_ref(), &dist_id, "recv-xver", &recv_status)
        .expect("rc.9 receiver write against legacy doc");

    // 3. rc.9 sender/watcher reads the doc. The receiver's entry MUST
    //    be visible even though metadata is still legacy `ROOT.data`.
    let read = read_distribution_document(doc_store.as_ref(), &dist_id)
        .expect("read")
        .expect("doc present");
    assert_eq!(read.distribution_id, dist_id);
    assert_eq!(read.status, "distributing", "legacy metadata preserved");
    let entry = read.node_statuses.get("recv-xver").expect(
        "rc.9 receiver write against a not-yet-migrated legacy doc MUST be \
         visible through read_distribution_document — pre-fix this was \
         dropped (the #864 failure mode re-introduced cross-version)",
    );
    assert_eq!(entry.status, TransferState::Completed);
    assert_eq!(entry.progress_bytes, 64);
}

/// rc.9-native cancel must NOT trample receiver-written
/// `ROOT.node_statuses` entries. The CHANGELOG promises this as the
/// schema split's whole point on the cancel side; the legacy-migration
/// variant is covered by `test_cancel_legacy_doc_preserves_node_statuses`
/// but the rc.9-native happy path had no direct pin.
///
/// A future refactor consolidating the cancel transact into a single
/// put across `ROOT.metadata` + `ROOT.node_statuses` would silently
/// regress #864 on the cancel path while every other test stayed
/// green — this test fails loudly on that.
#[tokio::test]
async fn test_rc9_cancel_preserves_receiver_node_statuses() {
    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    let token = blob_store
        .create_blob_from_bytes(
            b"cancel-preserve",
            BlobMetadata::with_name_and_type("cp.bin", "application/octet-stream"),
        )
        .await
        .expect("create blob");

    // 1. sender distribute() — rc.9 typed schema: ROOT.metadata +
    //    empty ROOT.node_statuses Map.
    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .expect("distribute");

    // 2. receiver writes its status into the typed node_statuses map.
    let recv = NodeTransferStatus {
        node_id: "recv-cp".to_string(),
        status: TransferState::Completed,
        progress_bytes: 15,
        total_bytes: 15,
        started_at: Some(Utc::now()),
        completed_at: Some(Utc::now()),
        error: None,
    };
    write_receiver_node_status(
        doc_store.as_ref(),
        &handle.distribution_id,
        "recv-cp",
        &recv,
    )
    .expect("receiver write");

    // 3. sender cancel() — rc.9 path: RMW on ROOT.metadata only.
    distribution.cancel(&handle).await.expect("cancel");

    // 4. receiver entry must survive the cancel untouched.
    let post = read_distribution_document(doc_store.as_ref(), &handle.distribution_id)
        .expect("read")
        .expect("doc present");
    assert_eq!(post.status, "cancelled", "metadata flip must apply");
    assert!(post.cancelled_at.is_some());
    let entry = post.node_statuses.get("recv-cp").expect(
        "rc.9 cancel must NOT trample receiver-written node_statuses — \
         this is the schema split's load-bearing cancel-side contract",
    );
    assert_eq!(entry.status, TransferState::Completed);
    assert_eq!(entry.progress_bytes, 15);
    assert_eq!(entry.total_bytes, 15);
}

/// `receive_sweep_once` must not abort when the store contains a malformed
/// distribution document — a single corrupted doc must not permanently stall
/// the inbox watcher for all remaining distributions.
///
/// This test validates the two preconditions that make the resilience fix
/// in `receive_sweep_once` (peat#980) effective:
///
/// 1. `scan_distribution_document_ids` returns ALL keys — including the
///    malformed one — because it never loads the Automerge payload.
/// 2. `read_distribution_document` returns `Err` for the malformed entry,
///    giving `receive_sweep_once` the opportunity to log-and-skip rather
///    than propagate.
///
/// The well-formed doc alongside it must remain readable, confirming
/// the store is not corrupted by the malformed neighbour.
#[tokio::test]
async fn test_receive_sweep_resilience_to_malformed_distribution_doc() {
    use automerge::transaction::Transactable;
    use automerge::{Automerge, ScalarValue, ROOT};
    use peat_protocol::storage::scan_distribution_document_ids;

    let temp = TempDir::new().unwrap();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (blob_store, doc_store) = create_integrated_stores(addr, temp.path()).await;
    let distribution = IrohFileDistribution::new(Arc::clone(&blob_store), Arc::clone(&doc_store));

    // One well-formed distribution doc.
    let token = blob_store
        .create_blob_from_bytes(b"payload", BlobMetadata::with_name_and_type("f.bin", "x"))
        .await
        .unwrap();
    let handle = distribution
        .distribute(
            &token,
            DistributionScope::AllNodes,
            TransferPriority::Normal,
        )
        .await
        .unwrap();

    // One malformed entry: metadata field present but not deserializable as
    // DistributionMetadata (mirrors the pattern in
    // `test_scan_distribution_documents_skips_malformed`).
    let mut bad = Automerge::new();
    bad.transact::<_, _, automerge::AutomergeError>(|tx| {
        tx.put(
            ROOT,
            "metadata",
            ScalarValue::Bytes(b"not-valid-json".to_vec()),
        )?;
        Ok(())
    })
    .unwrap();
    doc_store
        .put("file_distributions:malformed-sweep", &bad)
        .unwrap();

    // Precondition 1: scan_distribution_document_ids returns BOTH keys.
    // The key-only scan never loads Automerge payloads, so the malformed
    // entry is not filtered — receive_sweep_once will encounter it.
    let ids = scan_distribution_document_ids(doc_store.as_ref())
        .expect("key scan must succeed even with a malformed entry");
    assert!(
        ids.contains(&handle.distribution_id),
        "well-formed distribution must appear in key scan, got {:?}",
        ids
    );
    assert!(
        ids.contains(&"malformed-sweep".to_string()),
        "malformed key must appear in key scan — receive_sweep_once sees it, got {:?}",
        ids
    );

    // Precondition 2: read_distribution_document returns Err for the malformed
    // entry, not Ok(None).  receive_sweep_once handles Err by logging and
    // inserting into `handled`; if this returned Ok(None) instead the watcher
    // would still be safe, but Err is the actual signal for bad payloads.
    let result = read_distribution_document(doc_store.as_ref(), "malformed-sweep");
    assert!(
        result.is_err(),
        "read_distribution_document must return Err for a malformed entry, got Ok({:?})",
        result.unwrap()
    );

    // Well-formed doc is still readable — the store is not corrupted by its
    // malformed neighbour, and receive_sweep_once would load it successfully.
    let well_formed = read_distribution_document(doc_store.as_ref(), &handle.distribution_id)
        .expect("read must succeed for well-formed doc")
        .expect("well-formed doc must be present");
    assert_eq!(well_formed.distribution_id, handle.distribution_id);
}
