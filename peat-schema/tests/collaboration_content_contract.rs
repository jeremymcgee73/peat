use peat_schema::type_registry::{BuiltinRegistry, TypeId, TypeRegistry};
use serde_json::{json, Value};

const CHAT_ID: &str = "peat.collaboration.geochat.v1";
const OVERLAY_ID: &str = "peat.collaboration.overlay.revision.v1";
const ATTACHMENT_ID: &str = "peat.collaboration.attachment.offer.v1";

fn registry() -> BuiltinRegistry {
    BuiltinRegistry::with_peat_schema_types()
}

fn direct_audience() -> Value {
    json!({"kind": "direct", "recipients": ["node-bravo"]})
}

fn chat() -> Value {
    json!({
        "message_id": "msg-001",
        "sender_id": "node-alpha",
        "audience": direct_audience(),
        "sent_at_ms": 1_800_000_000_000_u64,
        "expires_at_ms": 1_800_604_800_000_u64,
        "body": "status green",
        "thread_id": "thread-9",
        "reply_to_id": "msg-000",
        "delivery_state": "queued"
    })
}

fn overlay() -> Value {
    json!({
        "overlay_id": "overlay-001",
        "revision_id": "revision-003",
        "revision_seq": 3,
        "actor_id": "node-alpha",
        "owner_id": "node-alpha",
        "audience": {"kind": "group", "group_id": "team-blue", "recipients": ["node-bravo"]},
        "source_time_ms": 1_800_000_000_000_u64,
        "deleted": false,
        "geometry": {"kind": "polygon", "points": [
            {"latitude": 35.0, "longitude": -120.0, "altitude_m": 4.0},
            {"latitude": 35.1, "longitude": -120.0},
            {"latitude": 35.1, "longitude": -120.1}
        ]},
        "visual": {
            "title": "Landing zone",
            "color": "#FF336699",
            "icon_type": "landing-zone",
            "stroke_width": 2.0,
            "fill_color": "#44336699",
            "remarks": "wind from west",
            "visible": true
        }
    })
}

fn attachment() -> Value {
    json!({
        "offer_id": "offer-001",
        "sender_id": "node-alpha",
        "audience": direct_audience(),
        "created_at_ms": 1_800_000_000_000_u64,
        "expires_at_ms": 1_800_086_400_000_u64,
        "content_kind": "photo",
        "relation": {"kind": "overlay", "id": "overlay-001"},
        "file": {
            "name": "overview.jpg",
            "media_type": "image/jpeg",
            "size_bytes": 1048576,
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "blob_ref": "blob-full-001"
        },
        "thumbnail": {
            "media_type": "image/jpeg",
            "size_bytes": 16384,
            "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            "blob_ref": "blob-thumb-001"
        }
    })
}

#[test]
fn collaboration_descriptors_resolve_by_type_and_collection() {
    let registry = registry();
    for (id, collection) in [
        (CHAT_ID, "collaboration-geochat"),
        (OVERLAY_ID, "collaboration-overlay-revisions"),
        (ATTACHMENT_ID, "collaboration-attachment-offers"),
    ] {
        assert_eq!(
            registry.get(&TypeId::new(id)).map(|d| d.id.as_str()),
            Some(id)
        );
        assert_eq!(
            registry.for_collection(collection).map(|d| d.id.as_str()),
            Some(id)
        );
    }
}

#[test]
fn valid_content_families_pass_schema_validation() {
    let registry = registry();
    for (id, document) in [
        (CHAT_ID, chat()),
        (OVERLAY_ID, overlay()),
        (ATTACHMENT_ID, attachment()),
    ] {
        (registry.get(&TypeId::new(id)).unwrap().validate_json)(&document).unwrap();
    }
}

#[test]
fn audience_is_explicit_and_bounded() {
    let desc = registry().get(&TypeId::new(CHAT_ID)).unwrap().clone();
    for audience in [
        json!({"kind": "direct", "recipients": []}),
        json!({"kind": "broadcast", "recipients": ["node-bravo"]}),
        json!({"kind": "formation"}),
    ] {
        let mut value = chat();
        value["audience"] = audience;
        assert!((desc.validate_json)(&value).is_err());
    }
}

#[test]
fn missing_oversized_and_expired_chat_fields_fail() {
    let desc = registry().get(&TypeId::new(CHAT_ID)).unwrap().clone();
    let mut missing = chat();
    missing.as_object_mut().unwrap().remove("sender_id");
    assert!((desc.validate_json)(&missing).is_err());

    let mut oversized = chat();
    oversized["body"] = Value::String("x".repeat(8193));
    assert!((desc.validate_json)(&oversized).is_err());

    let mut expired = chat();
    expired["expires_at_ms"] = json!(1_799_999_999_999_u64);
    assert!((desc.validate_json)(&expired).is_err());
}

#[test]
fn geometry_shapes_and_tombstones_are_bounded() {
    let desc = registry().get(&TypeId::new(OVERLAY_ID)).unwrap().clone();
    for geometry in [
        json!({"kind": "point", "point": {"latitude": 91.0, "longitude": 0.0}}),
        json!({"kind": "line", "points": [{"latitude": 1.0, "longitude": 2.0}]}),
        json!({"kind": "circle", "center": {"latitude": 1.0, "longitude": 2.0}, "radius_m": 0.0}),
        json!({"kind": "rectangle", "points": []}),
    ] {
        let mut value = overlay();
        value["geometry"] = geometry;
        assert!((desc.validate_json)(&value).is_err());
    }

    let mut tombstone = overlay();
    tombstone["deleted"] = json!(true);
    tombstone.as_object_mut().unwrap().remove("geometry");
    tombstone.as_object_mut().unwrap().remove("visual");
    (desc.validate_json)(&tombstone).expect("bounded tombstone");
}

#[test]
fn attachment_names_hashes_sizes_and_thumbnails_are_safe() {
    let desc = registry().get(&TypeId::new(ATTACHMENT_ID)).unwrap().clone();
    for (path, bad) in [
        ("name", json!("../secret.txt")),
        ("sha256", json!("not-a-sha256")),
        ("size_bytes", json!(268_435_457_u64)),
    ] {
        let mut value = attachment();
        value["file"][path] = bad;
        assert!((desc.validate_json)(&value).is_err());
    }

    let mut missing_thumbnail = attachment();
    missing_thumbnail
        .as_object_mut()
        .unwrap()
        .remove("thumbnail");
    assert!((desc.validate_json)(&missing_thumbnail).is_err());
}

#[test]
fn unknown_and_raw_protocol_fields_are_rejected() {
    for (id, mut document) in [
        (CHAT_ID, chat()),
        (OVERLAY_ID, overlay()),
        (ATTACHMENT_ID, attachment()),
    ] {
        document["raw_protocol"] = json!("<event/>");
        let registry = registry();
        let desc = registry.get(&TypeId::new(id)).unwrap();
        assert!((desc.validate_json)(&document).is_err());
    }
}

fn canonical_roundtrip(id: &str, source: &str) -> Value {
    let parsed: Value = serde_json::from_str(source).expect("canonical JSON fixture");
    let encoded = serde_json::to_string(&parsed).expect("JSON serialization");
    let decoded: Value = serde_json::from_str(&encoded).expect("JSON deserialization");
    assert_eq!(decoded, parsed);
    let registry = registry();
    let descriptor = registry
        .get(&TypeId::new(id))
        .expect("known schema version");
    (descriptor.validate_json)(&decoded).expect("valid interoperable document");
    decoded
}

#[test]
fn serde_only_reader_distinguishes_chat_audiences_and_reply_metadata() {
    let fixtures = [
        r#"{"message_id":"direct-1","sender_id":"node-alpha","audience":{"kind":"direct","recipients":["node-bravo"]},"sent_at_ms":1800000000000,"expires_at_ms":1800604800000,"body":"direct","thread_id":"thread-1","reply_to_id":"direct-0","delivery_state":"sent"}"#,
        r#"{"message_id":"group-1","sender_id":"node-alpha","audience":{"kind":"group","group_id":"team-blue","recipients":["node-bravo","node-charlie"]},"sent_at_ms":1800000000000,"expires_at_ms":1800604800000,"body":"group","delivery_state":"delivered"}"#,
        r#"{"message_id":"broadcast-1","sender_id":"node-alpha","audience":{"kind":"broadcast"},"sent_at_ms":1800000000000,"expires_at_ms":1800604800000,"body":"broadcast","delivery_state":"queued"}"#,
    ];
    let documents: Vec<Value> = fixtures
        .into_iter()
        .map(|fixture| canonical_roundtrip(CHAT_ID, fixture))
        .collect();
    assert_eq!(documents[0]["audience"]["kind"], "direct");
    assert_eq!(documents[0]["reply_to_id"], "direct-0");
    assert_eq!(documents[1]["audience"]["group_id"], "team-blue");
    assert_eq!(documents[2]["audience"]["kind"], "broadcast");
    assert!(documents[2]["audience"].get("recipients").is_none());
}

fn overlay_with_geometry(kind: &str, geometry: Value) -> Value {
    let mut value = overlay();
    value["overlay_id"] = json!(format!("overlay-{kind}"));
    value["revision_id"] = json!(format!("revision-{kind}-2"));
    value["revision_seq"] = json!(2);
    value["geometry"] = geometry;
    value
}

#[test]
fn serde_only_reader_handles_every_overlay_shape_update_and_delete() {
    let shapes = [
        overlay_with_geometry(
            "point",
            json!({"kind": "point", "point": {"latitude": 35.0, "longitude": -120.0}}),
        ),
        overlay_with_geometry(
            "line",
            json!({"kind": "line", "points": [
                {"latitude": 35.0, "longitude": -120.0},
                {"latitude": 35.1, "longitude": -120.1}
            ]}),
        ),
        overlay_with_geometry(
            "polygon",
            json!({"kind": "polygon", "points": [
                {"latitude": 35.0, "longitude": -120.0},
                {"latitude": 35.1, "longitude": -120.0},
                {"latitude": 35.1, "longitude": -120.1}
            ]}),
        ),
        overlay_with_geometry(
            "circle",
            json!({"kind": "circle", "center": {"latitude": 35.0, "longitude": -120.0}, "radius_m": 125.0}),
        ),
        overlay_with_geometry(
            "route",
            json!({"kind": "route", "points": [
                {"latitude": 35.0, "longitude": -120.0},
                {"latitude": 35.2, "longitude": -120.2}
            ]}),
        ),
    ];

    for shape in shapes {
        let source = serde_json::to_string(&shape).unwrap();
        let decoded = canonical_roundtrip(OVERLAY_ID, &source);
        assert_eq!(decoded["revision_seq"], 2);
        assert!(decoded["geometry"]["kind"].is_string());
    }

    let mut deleted = overlay();
    deleted["revision_id"] = json!("revision-delete-4");
    deleted["revision_seq"] = json!(4);
    deleted["deleted"] = json!(true);
    deleted.as_object_mut().unwrap().remove("geometry");
    deleted.as_object_mut().unwrap().remove("visual");
    let source = serde_json::to_string(&deleted).unwrap();
    let decoded = canonical_roundtrip(OVERLAY_ID, &source);
    assert_eq!(decoded["deleted"], true);
    assert!(decoded.get("geometry").is_none());
}

#[test]
fn serde_only_reader_distinguishes_application_file_and_thumbnail_contracts() {
    let photo_source = serde_json::to_string(&attachment()).unwrap();
    let photo = canonical_roundtrip(ATTACHMENT_ID, &photo_source);
    assert_eq!(photo["content_kind"], "photo");
    assert_ne!(photo["file"]["blob_ref"], photo["thumbnail"]["blob_ref"]);
    assert!(
        photo["file"]["size_bytes"].as_u64().unwrap()
            > photo["thumbnail"]["size_bytes"].as_u64().unwrap()
    );

    let file = canonical_roundtrip(
        ATTACHMENT_ID,
        r#"{"offer_id":"offer-file-1","sender_id":"node-alpha","audience":{"kind":"broadcast"},"created_at_ms":1800000000000,"expires_at_ms":1800086400000,"content_kind":"file","relation":{"kind":"none"},"file":{"name":"mission-plan.pdf","media_type":"application/pdf","size_bytes":4096,"sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","blob_ref":"blob-file-001"}}"#,
    );
    assert_eq!(file["content_kind"], "file");
    assert_eq!(file["file"]["media_type"], "application/pdf");
    assert!(file.get("thumbnail").is_none());
    assert!(
        file.get("body").is_none(),
        "application documents reference blobs rather than embedding file bytes"
    );
}

#[test]
fn unknown_schema_versions_are_rejected_deterministically() {
    let registry = registry();
    for id in [
        "peat.collaboration.geochat.v2",
        "peat.collaboration.overlay.revision.v0",
        "peat.collaboration.attachment.offer.v99",
    ] {
        assert!(registry.get(&TypeId::new(id)).is_none(), "{id}");
    }
}
