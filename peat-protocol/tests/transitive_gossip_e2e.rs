//! Transitive-gossip hub-and-spoke convergence test (peat#891 / peat#907).
//!
//! Regression protection for the QUICKSTART Scenario 2 / Scenario 4 promise
//! that a 3-node mesh wired as hub-and-spoke (alpha as hub, bravo and charlie
//! as spokes pointed only at alpha) converges — bravo and charlie see each
//! other's state via alpha without a direct B↔C edge.
//!
//! Pre-fix (rc.15 and earlier): the receive path in
//! `AutomergeSyncCoordinator::put_received` called `put_without_notify`, so
//! the change broadcast that the outbound pusher subscribes to never fired
//! on remote applies. A doc bravo wrote and synced to alpha never propagated
//! onward to charlie. Hub-and-spoke deadlocked at "each spoke sees alpha but
//! never the other spoke."
//!
//! Post-fix (rc.16): receive paths route through
//! `AutomergeStore::put_with_origin` so the new origin-tagged broadcast
//! (`subscribe_to_changes_with_origin`) fires with the source peer
//! attribution. The propagation task in
//! `peat-protocol/src/storage/automerge_backend.rs` subscribes there, pushes
//! to every connected peer except the source, and gossip converges.
//!
//! This test asserts both directions:
//! - bravo writes → alpha receives via direct sync → alpha gossips to charlie
//!   → charlie observes bravo's doc.
//! - charlie writes → alpha receives via direct sync → alpha gossips to bravo
//!   → bravo observes charlie's doc.

#![cfg(feature = "automerge-backend")]

use peat_protocol::sync::{ChangeEvent, DataSyncBackend, Document, Query, Value};
use peat_protocol::testing::E2EHarness;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Poll interval used as a fallback if observe() events don't arrive.
const SYNC_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Generous deadline for transitive convergence. Two-hop sync (write →
/// direct sync to hub → gossip to other spoke) is naturally slower than
/// one-hop direct sync, AND `cargo nextest` runs this test in parallel
/// with dozens of others on a 2-vCPU GitHub Actions runner. Empirical
/// progression:
/// - 45s: not enough; first CI run timed out on the second leg.
/// - 120s: not enough either; CI run 26266389895 spent 123.92s and
///   timed out just past the budget (second leg consistently slower
///   than first — same asymmetry the `#[ignore]`-gated
///   `test_automerge_three_node_mesh` documents per peat#829).
/// - 300s: chosen here. A real gossip failure hangs forever, not just
///   minutes, so this still cleanly separates "slow CI" from "broken
///   gossip" without inviting flakes.
const TRANSITIVE_SYNC_TIMEOUT: Duration = Duration::from_secs(300);

/// 3-node hub-and-spoke: alpha↔bravo + alpha↔charlie ONLY. No bravo↔charlie
/// direct edge. Asserts that documents written on one spoke converge on the
/// other spoke via gossip through the hub.
#[tokio::test]
async fn transitive_gossip_hub_and_spoke_converges_peat891() {
    println!("=== Transitive Gossip: hub-and-spoke convergence (peat#891) ===");

    let mut harness = E2EHarness::new("transitive_gossip_hub_and_spoke");

    let port_alpha = E2EHarness::allocate_tcp_port().expect("alpha port");
    let port_bravo = E2EHarness::allocate_tcp_port().expect("bravo port");
    let port_charlie = E2EHarness::allocate_tcp_port().expect("charlie port");
    println!(
        "  Ports: alpha={} bravo={} charlie={}",
        port_alpha, port_bravo, port_charlie
    );

    let addr_alpha: std::net::SocketAddr = format!("127.0.0.1:{}", port_alpha).parse().unwrap();
    let addr_bravo: std::net::SocketAddr = format!("127.0.0.1:{}", port_bravo).parse().unwrap();
    let addr_charlie: std::net::SocketAddr = format!("127.0.0.1:{}", port_charlie).parse().unwrap();

    let alpha = harness
        .create_automerge_backend_with_bind(Some(addr_alpha))
        .await
        .expect("alpha backend");
    let bravo = harness
        .create_automerge_backend_with_bind(Some(addr_bravo))
        .await
        .expect("bravo backend");
    let charlie = harness
        .create_automerge_backend_with_bind(Some(addr_charlie))
        .await
        .expect("charlie backend");

    println!("  ✓ 3 backends created");

    // Start sync before connecting. Connected events are broadcasts, so wiring
    // peers first loses the events and leaves this test dependent on the 5s
    // polling fallback in AutomergeBackend. Under parallel CI load that race
    // manifested as one receive handler never becoming ready (#1034).
    alpha
        .sync_engine()
        .start_sync()
        .await
        .expect("alpha start_sync");
    bravo
        .sync_engine()
        .start_sync()
        .await
        .expect("bravo start_sync");
    charlie
        .sync_engine()
        .start_sync()
        .await
        .expect("charlie start_sync");
    println!("  ✓ Sync started on all 3 nodes");

    // Hub-and-spoke wiring: alpha↔bravo and alpha↔charlie. NO bravo↔charlie.
    // A single authenticated Iroh connection provides both directions for
    // each edge.

    let bravo_endpoint = bravo.endpoint_id();
    let charlie_endpoint = charlie.endpoint_id();

    let bravo_info = peat_protocol::network::PeerInfo {
        name: "bravo".into(),
        node_id: hex::encode(bravo_endpoint.as_bytes()),
        addresses: vec![addr_bravo.to_string()],
        relay_url: None,
    };
    let charlie_info = peat_protocol::network::PeerInfo {
        name: "charlie".into(),
        node_id: hex::encode(charlie_endpoint.as_bytes()),
        addresses: vec![addr_charlie.to_string()],
        relay_url: None,
    };

    let key_alpha = alpha.formation_key().expect("alpha formation key");

    // alpha → bravo
    if let Some(conn) = alpha
        .transport()
        .connect_peer(&bravo_info)
        .await
        .expect("alpha→bravo connect")
    {
        peat_mesh::storage::respond_to_formation_auth(&key_alpha, &conn)
            .await
            .expect("alpha→bravo handshake");
        alpha.transport().emit_peer_connected(bravo_endpoint);
    }
    // alpha → charlie
    if let Some(conn) = alpha
        .transport()
        .connect_peer(&charlie_info)
        .await
        .expect("alpha→charlie connect")
    {
        peat_mesh::storage::respond_to_formation_auth(&key_alpha, &conn)
            .await
            .expect("alpha→charlie handshake");
        alpha.transport().emit_peer_connected(charlie_endpoint);
    }
    // Iroh connections are bidirectional. Reverse dials are unnecessary and
    // would only exercise conflict resolution rather than the topology under
    // test. DELIBERATELY NO bravo↔charlie edge: the only spoke-to-spoke path
    // is through alpha.
    println!("  ✓ Hub-and-spoke wired: alpha↔bravo, alpha↔charlie. NO direct bravo↔charlie edge.");

    // Subscribe to alpha's gossip broadcast directly so we can assert
    // the echo-filter string contract (peat#909 QA): the `Remote(src)`
    // string emitted by peat-mesh's coordinator MUST be character-for-
    // character identical to what `EndpointId::to_string()` produces on
    // the consumer side (the propagation task in
    // peat-protocol/src/storage/automerge_backend.rs L888). If the two
    // codepaths ever diverge — different formatting, different encoding,
    // a wrapper added on one side — the echo filter silently passes
    // everything through and the peat-mesh#115 ping-pong is reinstated
    // under attribution failure rather than suppression failure.
    // Convergence (assertions above) is idempotent at the CRDT layer
    // and would NOT catch a broken filter; this direct broadcast
    // observation does.
    let mut alpha_gossip_rx = alpha
        .storage_backend()
        .automerge_store()
        .subscribe_to_changes_with_origin();

    // Bravo → Charlie gossip leg.
    let bravo_to_charlie_doc_id = "gossip-from-bravo".to_string();
    let mut fields = HashMap::new();
    fields.insert("origin".to_string(), Value::String("bravo".into()));
    fields.insert("hop".to_string(), Value::String("via-alpha".into()));
    let bravo_doc = Document::with_id(bravo_to_charlie_doc_id.clone(), fields);

    bravo
        .document_store()
        .upsert("gossip_test", bravo_doc)
        .await
        .expect("bravo upsert");
    println!(
        "  ✓ Wrote '{}' on bravo — waiting for transitive convergence on charlie",
        bravo_to_charlie_doc_id
    );

    let bravo_to_charlie_landed =
        wait_for_doc(&charlie, "gossip_test", &bravo_to_charlie_doc_id, "charlie").await;
    assert!(
        bravo_to_charlie_landed,
        "peat#891 / peat#907 regression: doc '{}' written on bravo never reached charlie within {}s — \
         transitive gossip through alpha did not fire. bravo and charlie have no direct edge, so this \
         means receive_sync_message on alpha did not propagate bravo's change onward.",
        bravo_to_charlie_doc_id,
        TRANSITIVE_SYNC_TIMEOUT.as_secs()
    );
    println!("  ✓ Charlie observed bravo's doc via alpha — gossip works one direction");

    // peat#909 QA — string contract assertion. Drain alpha's gossip
    // broadcast for a `Remote(src)` event keyed at this doc and check
    // `src` against `bravo_endpoint.to_string()` exactly. Catches the
    // silent failure mode where peat-mesh's broadcast string and peat's
    // echo-filter comparison string drift apart.
    let expected_remote_src = bravo_endpoint.to_string();
    let observed_attribution = tokio::time::timeout(
        Duration::from_secs(5),
        find_remote_attribution_for_doc(
            &mut alpha_gossip_rx,
            "gossip_test",
            &bravo_to_charlie_doc_id,
        ),
    )
    .await
    .expect(
        "alpha's gossip_tx must surface a Remote(src) event for bravo's write within 5s — \
         either peat-mesh's broadcast didn't fire, or the doc never reached alpha at all",
    );
    assert_eq!(
        observed_attribution, expected_remote_src,
        "peat#909 QA: echo-filter string contract broken. peat-mesh emitted ChangeOrigin::Remote(\"{}\") \
         but peat's consumer compares against EndpointId::to_string() = \"{}\". A mismatch here silently \
         reinstates the peat-mesh#115 ping-pong: alpha would push bravo's just-received doc back to bravo.",
        observed_attribution,
        expected_remote_src
    );
    println!(
        "  ✓ Alpha's gossip_tx attributed bravo's write with the correct Remote(\"{}\") string — \
         echo-filter contract holds",
        &observed_attribution[..16]
    );

    // Charlie → Bravo gossip leg (symmetric).
    let charlie_to_bravo_doc_id = "gossip-from-charlie".to_string();
    let mut fields = HashMap::new();
    fields.insert("origin".to_string(), Value::String("charlie".into()));
    fields.insert("hop".to_string(), Value::String("via-alpha".into()));
    let charlie_doc = Document::with_id(charlie_to_bravo_doc_id.clone(), fields);

    charlie
        .document_store()
        .upsert("gossip_test", charlie_doc)
        .await
        .expect("charlie upsert");
    println!(
        "  ✓ Wrote '{}' on charlie — waiting for transitive convergence on bravo",
        charlie_to_bravo_doc_id
    );

    let charlie_to_alpha_landed =
        wait_for_doc(&alpha, "gossip_test", &charlie_to_bravo_doc_id, "alpha").await;
    assert!(
        charlie_to_alpha_landed,
        "peat#1034 regression: doc '{}' written on charlie never reached alpha within {}s — \
         the direct spoke-to-hub sync path failed before gossip could run.",
        charlie_to_bravo_doc_id,
        TRANSITIVE_SYNC_TIMEOUT.as_secs()
    );
    println!("  ✓ Alpha received charlie's doc over the direct spoke-to-hub edge");

    let expected_remote_src = charlie_endpoint.to_string();
    let observed_attribution = tokio::time::timeout(
        Duration::from_secs(5),
        find_remote_attribution_for_doc(
            &mut alpha_gossip_rx,
            "gossip_test",
            &charlie_to_bravo_doc_id,
        ),
    )
    .await
    .expect(
        "peat#1034 regression: alpha stored charlie's doc but did not broadcast its remote-change \
         attribution within 5s, so the gossip propagation task could not fire",
    );
    assert_eq!(
        observed_attribution, expected_remote_src,
        "peat#1034 regression: alpha attributed charlie's doc to the wrong source peer"
    );
    println!("  ✓ Alpha broadcast charlie's doc with correct remote attribution");

    let charlie_to_bravo_landed =
        wait_for_doc(&bravo, "gossip_test", &charlie_to_bravo_doc_id, "bravo").await;
    assert!(
        charlie_to_bravo_landed,
        "peat#891 / peat#907 regression: doc '{}' written on charlie never reached bravo within {}s — \
         gossip is asymmetric, working only one direction.",
        charlie_to_bravo_doc_id,
        TRANSITIVE_SYNC_TIMEOUT.as_secs()
    );
    println!("  ✓ Bravo observed charlie's doc via alpha — gossip works the other direction too");

    println!("=== peat#891 / peat#907 hub-and-spoke convergence test passed ===");
}

/// Drain the gossip broadcast until we observe a `Remote(_)` event for
/// the given `collection:doc_id` key and return the source-peer string.
/// Skips `Local` events (from the receiver's own writes) and unrelated
/// keys (e.g. nodes/discovery docs that may also flow through the
/// broadcast). Pinned to peat-mesh's `DocChange` shape — if either side
/// changes the broadcast format this helper stops compiling, surfacing
/// the contract drift at the test layer.
async fn find_remote_attribution_for_doc(
    rx: &mut tokio::sync::broadcast::Receiver<peat_mesh::storage::DocChange>,
    collection: &str,
    doc_id: &str,
) -> String {
    use peat_mesh::storage::ChangeOrigin;
    let target_key = format!("{}:{}", collection, doc_id);
    loop {
        match rx.recv().await {
            Ok(evt) if evt.key == target_key => {
                if let ChangeOrigin::Remote(src) = evt.origin {
                    return src;
                }
                // Local event for our target key — keep scanning;
                // we want the Remote attribution that drives gossip.
            }
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => panic!("alpha gossip_tx receiver closed unexpectedly"),
        }
    }
}

/// Wait for a specific document to appear on a backend using a combination
/// of `observe()` (event-driven) and `get()` polling (fallback).
async fn wait_for_doc<B: DataSyncBackend>(
    backend: &Arc<B>,
    collection: &str,
    doc_id: &str,
    node_name: &str,
) -> bool {
    let doc_id_owned = doc_id.to_string();

    // Already present?
    if let Ok(Some(_)) = backend
        .document_store()
        .get(collection, &doc_id_owned)
        .await
    {
        println!("    {}: '{}' already present", node_name, doc_id);
        return true;
    }

    let stream = backend
        .document_store()
        .observe(collection, &Query::All)
        .ok();

    let result = tokio::time::timeout(TRANSITIVE_SYNC_TIMEOUT, async {
        if let Some(mut stream) = stream {
            loop {
                tokio::select! {
                    event = stream.receiver.recv() => {
                        match event {
                            Some(ChangeEvent::Updated { document, .. }) => {
                                if document.id.as_deref() == Some(doc_id) {
                                    for _ in 0..10 {
                                        if let Ok(Some(_)) = backend
                                            .document_store()
                                            .get(collection, &doc_id_owned)
                                            .await
                                        {
                                            return true;
                                        }
                                        sleep(Duration::from_millis(50)).await;
                                    }
                                }
                            }
                            Some(ChangeEvent::Initial { documents, .. }) => {
                                if documents.iter().any(|d| d.id.as_deref() == Some(doc_id)) {
                                    if let Ok(Some(_)) = backend
                                        .document_store()
                                        .get(collection, &doc_id_owned)
                                        .await
                                    {
                                        return true;
                                    }
                                }
                            }
                            Some(_) => continue,
                            None => break,
                        }
                    }
                    _ = sleep(Duration::from_secs(1)) => {
                        if let Ok(Some(_)) = backend
                            .document_store()
                            .get(collection, &doc_id_owned)
                            .await
                        {
                            return true;
                        }
                    }
                }
            }
        }

        // Pure poll fallback if observe() was unavailable.
        loop {
            sleep(SYNC_POLL_INTERVAL).await;
            if let Ok(Some(_)) = backend
                .document_store()
                .get(collection, &doc_id_owned)
                .await
            {
                return true;
            }
        }
    })
    .await;

    matches!(result, Ok(true))
}
