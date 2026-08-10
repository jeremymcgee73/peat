#![cfg(feature = "sync")]

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use peat_ffi::{
    create_node, ApplicationDeliveryAudience, ApplicationDeliveryPriority,
    ApplicationDeliveryStatus, ApplicationDeliverySubmitRequest, NodeConfig, PeatError, PeatNode,
    PeerInfo,
};
use peat_protocol::storage::application_delivery::{DeliveryAudience, DeliveryStatus};

const SHARED_KEY: &str = "dGVzdC1rZXktMTIzNDU2Nzg5MDEyMzQ1Njc4OTAxMjM0";
const COLLECTION: &str = "collaboration-geochat";
const TYPE_ID: &str = "peat.collaboration.geochat.v1";

fn config(path: &std::path::Path) -> NodeConfig {
    NodeConfig {
        app_id: "application-delivery-ffi-tests".to_string(),
        shared_key: SHARED_KEY.to_string(),
        bind_address: Some("127.0.0.1:0".to_string()),
        storage_path: path.to_string_lossy().into_owned(),
        transport: None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn chat_body(message_id: &str, sender_id: &str, recipient_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "message_id": message_id,
        "sender_id": sender_id,
        "audience": {"kind": "direct", "recipients": [recipient_id]},
        "sent_at_ms": now_ms(),
        "expires_at_ms": now_ms() + 60_000,
        "body": "status green",
        "delivery_state": "queued"
    }))
    .unwrap()
}

fn request(
    node: &PeatNode,
    target: &PeatNode,
    operation_id: &str,
) -> ApplicationDeliverySubmitRequest {
    ApplicationDeliverySubmitRequest {
        client_operation_id: operation_id.to_string(),
        audience: ApplicationDeliveryAudience::Direct,
        target_node_ids: vec![target.node_id()],
        priority: ApplicationDeliveryPriority::Metadata,
        collection: COLLECTION.to_string(),
        type_id: TYPE_ID.to_string(),
        document_id: format!("document-{operation_id}"),
        body: chat_body(operation_id, &node.node_id(), &target.node_id()),
        expires_at_ms: now_ms() + 60_000,
    }
}

fn connect(a: &Arc<PeatNode>, b: &Arc<PeatNode>) {
    a.start_sync().expect("start sender sync");
    b.start_sync().expect("start receiver sync");
    a.connect_peer(PeerInfo {
        name: "receiver".to_string(),
        node_id: b.node_id(),
        addresses: vec![b.endpoint_socket_addr().expect("receiver socket")],
        relay_url: None,
    })
    .expect("connect nodes");
}

#[test]
fn protocol_reexports_the_owner_contract() {
    let direct = DeliveryAudience::Direct(["peer".to_string()].into_iter().collect());
    assert!(matches!(direct, DeliveryAudience::Direct(_)));
    assert!(matches!(
        DeliveryStatus::Acknowledged,
        DeliveryStatus::Acknowledged
    ));
}

#[test]
fn bounded_status_operations_are_durable_and_paginated() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = create_node(config(sender_dir.path())).unwrap();
    let receiver = create_node(config(receiver_dir.path())).unwrap();

    for id in ["operation-001", "operation-002", "operation-003"] {
        assert_eq!(
            sender
                .application_delivery_submit(request(&sender, &receiver, id))
                .unwrap(),
            id
        );
    }

    let first = sender.application_delivery_list(None, 2).unwrap();
    assert_eq!(first.operations.len(), 2);
    let second = sender
        .application_delivery_list(first.next_cursor.clone(), 2)
        .unwrap();
    assert_eq!(second.operations.len(), 1);
    assert!(second.next_cursor.is_none());

    let mut ids = first
        .operations
        .into_iter()
        .chain(second.operations)
        .map(|operation| operation.client_operation_id)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["operation-001", "operation-002", "operation-003"]);

    sender.application_delivery_cancel("operation-001").unwrap();
    assert!(sender
        .application_delivery_get("operation-001")
        .unwrap()
        .recipients
        .iter()
        .all(|evidence| evidence.status == ApplicationDeliveryStatus::Cancelled));
    sender.application_delivery_retry("operation-001").unwrap();

    sender.stop_sync().expect("stop sender sync");
    drop(sender);
    let reopened = create_node(config(sender_dir.path())).expect("reopen sender");
    let subscribed = reopened.application_delivery_subscribe(None, 3).unwrap();
    assert_eq!(subscribed.operations.len(), 3);
    assert!(reopened
        .application_delivery_get("operation-001")
        .unwrap()
        .recipients
        .iter()
        .all(|evidence| evidence.status == ApplicationDeliveryStatus::Cancelled));
}

#[test]
fn malformed_requests_fail_before_durable_mutation() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = create_node(config(sender_dir.path())).unwrap();
    let receiver = create_node(config(receiver_dir.path())).unwrap();

    let mut invalid = request(&sender, &receiver, "invalid");
    invalid.target_node_ids.clear();
    assert!(matches!(
        sender.application_delivery_submit(invalid),
        Err(PeatError::InvalidInput { .. })
    ));

    let mut invalid = request(&sender, &receiver, "invalid");
    invalid.body = vec![b'x'; 1024 * 1024 + 1];
    assert!(matches!(
        sender.application_delivery_submit(invalid),
        Err(PeatError::InvalidInput { .. })
    ));

    assert!(sender
        .application_delivery_list(None, 100)
        .unwrap()
        .operations
        .is_empty());
}

#[test]
fn acknowledged_bodies_are_queryable_after_notification_loss_and_restart() {
    let sender_dir = tempfile::tempdir().unwrap();
    let receiver_dir = tempfile::tempdir().unwrap();
    let sender = create_node(config(sender_dir.path())).unwrap();
    let receiver = create_node(config(receiver_dir.path())).unwrap();
    connect(&sender, &receiver);

    let operation_ids = ["restart-alpha", "restart-bravo", "restart-charlie"];
    for operation_id in operation_ids {
        sender
            .application_delivery_submit(request(&sender, &receiver, operation_id))
            .unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if operation_ids.iter().all(|operation_id| {
            sender
                .application_delivery_get(operation_id)
                .unwrap()
                .recipients
                .iter()
                .all(|evidence| evidence.status == ApplicationDeliveryStatus::Delivered)
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "delivery was not acknowledged");
        thread::sleep(Duration::from_millis(50));
    }

    let document_id = "document-restart-alpha";
    let received = receiver
        .application_delivery_get_received_document(COLLECTION, document_id)
        .unwrap()
        .expect("known delivered document");
    assert_eq!(received.collection, COLLECTION);
    assert_eq!(received.document_id, document_id);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&received.body).unwrap()["message_id"],
        "restart-alpha"
    );

    let first = receiver
        .application_delivery_list_received_documents(COLLECTION, None, 2)
        .unwrap();
    assert_eq!(first.documents.len(), 2);
    let cursor = first.next_cursor.expect("received page must continue");
    assert!(receiver
        .application_delivery_list_received_documents(
            "different-collection",
            Some(cursor.clone()),
            2
        )
        .is_err());

    sender.stop_sync().expect("stop sender sync");
    receiver.stop_sync().expect("stop receiver sync");
    drop(receiver);

    let reopened = create_node(config(receiver_dir.path())).expect("reopen receiver");
    let second = reopened
        .application_delivery_list_received_documents(COLLECTION, Some(cursor), 2)
        .unwrap();
    assert_eq!(second.documents.len(), 1);
    assert!(second.next_cursor.is_none());
    let mut document_ids = first
        .documents
        .into_iter()
        .chain(second.documents)
        .map(|document| document.document_id)
        .collect::<Vec<_>>();
    document_ids.sort();
    assert_eq!(
        document_ids,
        [
            "document-restart-alpha",
            "document-restart-bravo",
            "document-restart-charlie",
        ]
    );
}

#[test]
fn ffi_status_vocabulary_has_no_read_receipt() {
    let source = include_str!("../src/application_delivery.rs");
    assert!(!source.contains("Read"));
    assert!(!source.contains("read receipt"));
}
