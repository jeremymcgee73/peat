//! ADR-059 Slice 1.b.2.2: end-to-end test that origin labels supplied to
//! [`peat_protocol::sync::DocumentStore::upsert_with_origin`] propagate
//! through the [`IrohDocumentStore`] observer pipeline and arrive on the
//! emitted [`peat_mesh::sync::types::ChangeEvent::Updated`].
//!
//! The earlier integration suites (`automerge_iroh_sync_e2e`,
//! `multi_node_mesh_e2e`) cover doc-content propagation but pre-date origin
//! threading — they all pass `origin: None` and the observer emits
//! `origin: None`, so they don't catch a regression where origin is dropped
//! mid-pipeline.
//!
//! This file's tests are the single-node companion: stash an origin via
//! `upsert_with_origin`, observe the same backend's stream, and assert the
//! origin arrives intact on the event. They are explicitly the
//! "self-loop" shape — origin propagation across multiple iroh peers is the
//! larger Slice 2 cross-node test surface.

#![cfg(feature = "automerge-backend")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use peat_mesh::sync::types::ChangeEvent;
use peat_protocol::network::IrohTransport;
use peat_protocol::security::FormationKey;
use peat_protocol::storage::AutomergeStore;
use peat_protocol::sync::automerge::AutomergeIrohBackend;
use peat_protocol::sync::traits::DataSyncBackend;
use peat_protocol::sync::types::{BackendConfig, Document, Query, TransportConfig};
use serial_test::serial;
use tokio::time::timeout;

async fn make_backend() -> (Arc<AutomergeIrohBackend>, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(AutomergeStore::open(temp.path()).expect("store"));
    let transport = Arc::new(IrohTransport::new().await.expect("transport"));
    let backend = Arc::new(AutomergeIrohBackend::from_parts(store, transport));

    let cfg = BackendConfig {
        app_id: "origin-threading-test".to_string(),
        persistence_dir: temp.path().to_path_buf(),
        // FormationKey required by AutomergeIrohBackend even though this
        // test never connects to a peer — single-node stash/observe path.
        shared_key: Some(FormationKey::generate_secret()),
        transport: TransportConfig::default(),
        extra: HashMap::new(),
    };
    backend.initialize(cfg).await.expect("initialize");
    (backend, temp)
}

fn track_doc(id: &str, lat: f64, lon: f64) -> Document {
    let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
    fields.insert("lat".to_string(), serde_json::json!(lat));
    fields.insert("lon".to_string(), serde_json::json!(lon));
    Document::with_id(id.to_string(), fields)
}

/// Recv on the observer stream until we get the first `Updated` event for
/// `expected_doc_id`, ignoring the leading `Initial` snapshot. Times out at
/// 5s to fail loud rather than hang.
async fn next_updated_for(
    stream: &mut peat_mesh::sync::types::ChangeStream,
    expected_doc_id: &str,
) -> ChangeEvent {
    let deadline = Duration::from_secs(5);
    timeout(deadline, async {
        loop {
            let ev = stream.receiver.recv().await.expect("stream closed");
            if let ChangeEvent::Updated { ref document, .. } = ev {
                if document.id.as_deref() == Some(expected_doc_id) {
                    return ev;
                }
            }
            // Skip Initial snapshot and unrelated docs.
        }
    })
    .await
    .expect("timed out waiting for matching Updated event")
}

/// Baseline: `upsert_with_origin(.., Some("ble"))` propagates `Some("ble")`
/// to the observer's emitted `ChangeEvent::Updated`.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn upsert_with_origin_propagates_to_observer() {
    let (backend, _tmp) = make_backend().await;
    let store = backend.document_store();
    let mut stream = store.observe("tracks", &Query::All).expect("observe");

    let doc = track_doc("track-001", 40.0, -74.0);
    store
        .upsert_with_origin("tracks", doc, Some("ble".into()))
        .await
        .expect("upsert");

    let ev = next_updated_for(&mut stream, "track-001").await;
    match ev {
        ChangeEvent::Updated { origin, .. } => {
            assert_eq!(
                origin,
                Some("ble".to_string()),
                "origin must thread through observer"
            );
        }
        _ => unreachable!("filtered by next_updated_for"),
    }

    let _ = backend.shutdown().await;
}

/// `upsert_with_origin(.., None)` keeps origin empty on the observer event —
/// distinguishes "stashed None" from "stashed Some" round-trips.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn upsert_with_none_origin_emits_none() {
    let (backend, _tmp) = make_backend().await;
    let store = backend.document_store();
    let mut stream = store.observe("tracks", &Query::All).expect("observe");

    let doc = track_doc("track-002", 41.0, -75.0);
    store
        .upsert_with_origin("tracks", doc, None)
        .await
        .expect("upsert");

    let ev = next_updated_for(&mut stream, "track-002").await;
    match ev {
        ChangeEvent::Updated { origin, .. } => {
            assert_eq!(origin, None, "None origin must round-trip");
        }
        _ => unreachable!(),
    }

    let _ = backend.shutdown().await;
}

/// Rapid same-key burst — every observer event still carries an origin
/// (latest-wins on collision, but for the rapid-update steady state every
/// upsert ends up paired with its own broadcast notification before the
/// next upsert overwrites). The strong invariant is "no `None` mixed in
/// across the burst when all callers passed `Some(...)`".
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn rapid_same_key_burst_keeps_origin_present() {
    let (backend, _tmp) = make_backend().await;
    let store = backend.document_store();
    let mut stream = store.observe("tracks", &Query::All).expect("observe");

    // Burst of 5 updates to the same doc id.
    for i in 0..5 {
        let doc = track_doc("track-rapid", 40.0 + i as f64 * 0.001, -74.0);
        store
            .upsert_with_origin("tracks", doc, Some(format!("ble-{}", i)))
            .await
            .expect("upsert");
    }

    // Drain Updated events for `track-rapid` for up to 2s; assert every one
    // we see has Some(origin) starting with "ble-". Tolerate fewer than 5
    // events delivered — the broadcast may coalesce — but require that
    // every delivered event carries an origin.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut delivered_with_origin = 0;
    while tokio::time::Instant::now() < deadline {
        let recv_result =
            tokio::time::timeout(Duration::from_millis(100), stream.receiver.recv()).await;
        match recv_result {
            Ok(Some(ChangeEvent::Updated {
                document, origin, ..
            })) if document.id.as_deref() == Some("track-rapid") => {
                assert!(
                    origin.is_some(),
                    "rapid burst must not surface None origin when every upsert passed Some(...)"
                );
                let label = origin.unwrap();
                assert!(
                    label.starts_with("ble-"),
                    "origin {:?} must come from this test's burst",
                    label
                );
                delivered_with_origin += 1;
            }
            Ok(Some(_)) => {
                // Initial snapshot or unrelated doc — skip.
            }
            Ok(None) => break, // stream closed
            Err(_) => break,   // recv timeout — burst delivery done
        }
    }

    assert!(
        delivered_with_origin >= 1,
        "at least one Updated event must reach the observer for the burst"
    );

    let _ = backend.shutdown().await;
}

/// Regression for the cross-instance `pending_origins` bug — the production
/// BLE echo-storm root cause. `AutomergeIrohBackend::document_store()` used to
/// mint a fresh `PendingOrigins` map on every call, so an origin stashed via
/// one `IrohDocumentStore` instance was invisible to the observer task spawned
/// by a *different* instance (the observer always popped `None`). The
/// real call shape hits exactly this: `Node::publish_with_origin` and
/// `Node::observe` each call `backend.document_store()` independently. The
/// baseline `upsert_with_origin_propagates_to_observer` uses a SINGLE store, so
/// it passes even with the bug; this test uses TWO and fails unless the map is
/// owned on the backend and shared into each instance.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn origin_threads_across_separate_document_store_instances() {
    let (backend, _tmp) = make_backend().await;

    // Observer owns one IrohDocumentStore instance...
    let observe_store = backend.document_store();
    let mut stream = observe_store
        .observe("tracks", &Query::All)
        .expect("observe");

    // ...the writer uses a DISTINCT instance (separate document_store() call).
    let write_store = backend.document_store();
    let doc = track_doc("track-xinst", 41.0, -75.0);
    write_store
        .upsert_with_origin("tracks", doc, Some("ble".into()))
        .await
        .expect("upsert");

    let ev = next_updated_for(&mut stream, "track-xinst").await;
    match ev {
        ChangeEvent::Updated { origin, .. } => {
            assert_eq!(
                origin,
                Some("ble".to_string()),
                "origin must thread across separate document_store() instances \
                 (shared pending_origins on the backend); a fresh per-call map \
                 regresses this to None and re-opens the BLE echo storm"
            );
        }
        _ => unreachable!("filtered by next_updated_for"),
    }

    let _ = backend.shutdown().await;
}
