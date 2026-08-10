#![cfg(feature = "sync")]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use peat_ffi::{create_node, NodeConfig, PeatError, PeatNode, PeerInfo};

const SHARED_KEY: &str = "dGVzdC1rZXktMTIzNDU2Nzg5MDEyMzQ1Njc4OTAxMjM0";
const COLLECTION: &str = "peat.collaboration.geochat.v1";

fn config(path: &std::path::Path) -> NodeConfig {
    NodeConfig {
        app_id: "application-document-query-tests".to_string(),
        shared_key: SHARED_KEY.to_string(),
        bind_address: Some("127.0.0.1:0".to_string()),
        storage_path: path.to_string_lossy().into_owned(),
        transport: None,
    }
}

fn publish(node: &PeatNode, id: &str, text: &str) {
    node.put_document(
        COLLECTION,
        id,
        &serde_json::json!({"message_id": id, "text": text}).to_string(),
    )
    .expect("publish application document");
}

fn connect(a: &Arc<PeatNode>, b: &Arc<PeatNode>) {
    a.start_sync().expect("start node A sync");
    b.start_sync().expect("start node B sync");
    a.connect_peer(PeerInfo {
        name: "node-b".to_string(),
        node_id: b.node_id(),
        addresses: vec![b.endpoint_socket_addr().expect("node B socket")],
        relay_url: None,
    })
    .expect("connect synchronized nodes");
}

fn wait_for_document(node: &PeatNode, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        node.request_sync().expect("request document sync");
        if node
            .node_get_application_document(COLLECTION, id)
            .expect("query synchronized document")
            .is_some()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "document {id} did not synchronize"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn query_all(node: &PeatNode, page_size: u32) -> Vec<(String, serde_json::Value)> {
    let mut cursor = None;
    let mut documents = Vec::new();
    loop {
        let page = node
            .node_query_application_documents(COLLECTION, cursor.clone(), page_size)
            .expect("query catch-up page");
        documents.extend(page.documents.into_iter().map(|document| {
            (
                document.id,
                serde_json::from_str(&document.json_data).expect("application document JSON"),
            )
        }));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return documents,
        }
    }
}

#[test]
fn local_and_peer_synced_documents_use_the_public_node_layer() {
    let dir_a = tempfile::tempdir().expect("node A tempdir");
    let dir_b = tempfile::tempdir().expect("node B tempdir");
    let node_a = create_node(config(dir_a.path())).expect("create node A");
    let node_b = create_node(config(dir_b.path())).expect("create node B");

    publish(&node_a, "local-001", "published locally");
    let local = node_a
        .node_get_application_document(COLLECTION, "local-001")
        .expect("get local document")
        .expect("local document exists");
    assert_eq!(local.id, "local-001");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&local.json_data).unwrap()["text"],
        "published locally"
    );

    publish(&node_b, "remote-001", "published by peer");
    connect(&node_a, &node_b);
    wait_for_document(&node_a, "remote-001");

    let remote = node_a
        .node_get_application_document(COLLECTION, "remote-001")
        .expect("get peer document")
        .expect("peer document exists");
    assert_eq!(remote.id, "remote-001");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&remote.json_data).unwrap()["text"],
        "published by peer"
    );

    node_a.stop_sync().expect("stop node A sync");
    node_b.stop_sync().expect("stop node B sync");
}

#[test]
fn cursor_pages_are_bounded_stable_and_complete() {
    let dir = tempfile::tempdir().expect("node tempdir");
    let node = create_node(config(dir.path())).expect("create node");
    for id in ["doc-004", "doc-001", "doc-005", "doc-002", "doc-003"] {
        publish(&node, id, id);
    }

    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = node
            .node_query_application_documents(COLLECTION, cursor.clone(), 2)
            .expect("query page");
        assert!(page.documents.len() <= 2, "page limit must be enforced");
        ids.extend(page.documents.into_iter().map(|doc| doc.id));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert_eq!(ids, ["doc-001", "doc-002", "doc-003", "doc-004", "doc-005"]);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
}

#[test]
fn malformed_inputs_and_oversized_requests_fail_closed() {
    let dir = tempfile::tempdir().expect("node tempdir");
    let node = create_node(config(dir.path())).expect("create node");

    for result in [
        node.node_get_application_document("", "doc-001")
            .map(|_| ()),
        node.node_get_application_document("valid", "bad:id")
            .map(|_| ()),
        node.node_query_application_documents("bad:collection", None, 1)
            .map(|_| ()),
        node.node_query_application_documents(COLLECTION, Some("not-a-cursor".into()), 1)
            .map(|_| ()),
        node.node_query_application_documents(COLLECTION, None, 0)
            .map(|_| ()),
        node.node_query_application_documents(COLLECTION, None, 101)
            .map(|_| ()),
    ] {
        assert!(matches!(result, Err(PeatError::InvalidInput { .. })));
    }

    let page = node
        .node_query_application_documents(COLLECTION, None, 1)
        .expect("empty collection query");
    assert!(page.documents.is_empty());
    assert!(page.next_cursor.is_none());

    publish(&node, "cursor-source", "cursor source");
    publish(&node, "cursor-tail", "cursor tail");
    let cursor = node
        .node_query_application_documents(COLLECTION, None, 1)
        .expect("first cursor page")
        .next_cursor
        .expect("cursor for remaining document");
    assert!(matches!(
        node.node_query_application_documents("peat.collaboration.overlay.v1", Some(cursor), 1),
        Err(PeatError::InvalidInput { .. })
    ));

    node.put_document(
        COLLECTION,
        "oversized-001",
        &serde_json::json!({"payload": "x".repeat(1024 * 1024 + 1)}).to_string(),
    )
    .expect("publish oversized fixture through compatibility writer");
    assert!(matches!(
        node.node_get_application_document(COLLECTION, "oversized-001"),
        Err(PeatError::InvalidInput { .. })
    ));
}

#[test]
fn restart_catch_up_is_complete_without_notification_delivery() {
    let dir_a = tempfile::tempdir().expect("node A tempdir");
    let dir_b = tempfile::tempdir().expect("node B tempdir");
    let node_a = create_node(config(dir_a.path())).expect("create node A");
    let node_b = create_node(config(dir_b.path())).expect("create node B");

    let first_subscription = node_b.subscribe_poll().expect("subscribe before sync");
    publish(&node_a, "message-001", "before connection");
    connect(&node_a, &node_b);
    wait_for_document(&node_b, "message-001");
    let _drained_wake_signals = first_subscription.poll_changes();
    first_subscription.cancel();

    publish(&node_a, "message-002", "while notifications are cancelled");
    node_a
        .put_document(
            COLLECTION,
            "tombstone-003",
            &serde_json::json!({
                "message_id": "message-003",
                "deleted": true,
                "deleted_at": 1_786_049_722_000_i64
            })
            .to_string(),
        )
        .expect("publish immutable tombstone application record");
    wait_for_document(&node_b, "message-002");
    wait_for_document(&node_b, "tombstone-003");

    let recreated_subscription = node_b.subscribe_poll().expect("recreate subscription");
    assert!(
        recreated_subscription.poll_changes().is_empty(),
        "a recreated subscription must not be required to replay missed payloads"
    );
    recreated_subscription.cancel();

    node_a.stop_sync().expect("stop node A sync");
    node_b.stop_sync().expect("stop node B sync");
    drop(node_b);

    let reopened = create_node(config(dir_b.path())).expect("reopen receiving node");
    let first_scan = query_all(&reopened, 2);
    let second_scan = query_all(&reopened, 2);
    assert_eq!(
        first_scan, second_scan,
        "repeated catch-up must be idempotent"
    );

    let ids: Vec<_> = first_scan.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["message-001", "message-002", "tombstone-003"]);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
    assert_eq!(first_scan[0].1["text"], "before connection");
    assert_eq!(first_scan[1].1["text"], "while notifications are cancelled");
    assert_eq!(first_scan[2].1["deleted"], true);
    assert_eq!(first_scan[2].1["message_id"], "message-003");
}
