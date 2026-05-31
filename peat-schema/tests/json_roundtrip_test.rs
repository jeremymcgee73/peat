//! Test protobuf ↔ JSON roundtrip serialization
//!
//! Validates that protobuf messages can be serialized to JSON and back
//! for Ditto CRDT storage (which requires structured JSON, not binary blobs).

use peat_schema::common::v1::Position;
use peat_schema::hierarchy::v1::CellSummary;

#[test]
fn test_cell_summary_json_roundtrip() {
    let original = CellSummary {
        cell_id: "cell-1".to_string(),
        leader_id: "node-123".to_string(),
        member_count: 5,
        member_ids: vec![
            "node-1".to_string(),
            "node-2".to_string(),
            "node-3".to_string(),
        ],
        position_centroid: Some(Position {
            latitude: 37.7749,
            longitude: -122.4194,
            altitude: 10.0,
        }),
        avg_fuel_minutes: 45.5,
        ..Default::default()
    };

    let json_value =
        serde_json::to_value(&original).expect("Failed to serialize CellSummary to JSON");

    println!(
        "JSON representation:\n{}",
        serde_json::to_string_pretty(&json_value).unwrap()
    );

    assert_eq!(json_value["cell_id"], "cell-1");
    assert_eq!(json_value["leader_id"], "node-123");
    assert_eq!(json_value["member_count"], 5);
    assert!(json_value["member_ids"].is_array());
    assert_eq!(json_value["member_ids"].as_array().unwrap().len(), 3);
    assert!(json_value["position_centroid"].is_object());
    assert_eq!(json_value["position_centroid"]["latitude"], 37.7749);

    let roundtrip: CellSummary =
        serde_json::from_value(json_value).expect("Failed to deserialize JSON to CellSummary");

    assert_eq!(roundtrip.cell_id, original.cell_id);
    assert_eq!(roundtrip.leader_id, original.leader_id);
    assert_eq!(roundtrip.member_count, original.member_count);
    assert_eq!(roundtrip.member_ids, original.member_ids);
    assert!(roundtrip.position_centroid.is_some());
    let pos = roundtrip.position_centroid.unwrap();
    assert_eq!(pos.latitude, 37.7749);
    assert_eq!(pos.longitude, -122.4194);
    assert_eq!(pos.altitude, 10.0);
}

#[test]
fn test_json_structure_for_ditto_crdt() {
    let summary = CellSummary {
        cell_id: "cell-1".to_string(),
        leader_id: "node-123".to_string(),
        member_count: 5,
        member_ids: vec!["node-1".to_string(), "node-2".to_string()],
        ..Default::default()
    };

    let json = serde_json::to_value(&summary).unwrap();

    let members = json["member_ids"]
        .as_array()
        .expect("member_ids should be array");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0], "node-1");
    assert_eq!(members[1], "node-2");

    assert_eq!(json["leader_id"].as_str().unwrap(), "node-123");
    assert_eq!(json["member_count"].as_i64().unwrap(), 5);
}

#[test]
fn test_nested_message_expansion() {
    let summary = CellSummary {
        cell_id: "cell-1".to_string(),
        position_centroid: Some(Position {
            latitude: 37.7749,
            longitude: -122.4194,
            altitude: 10.0,
        }),
        ..Default::default()
    };

    let json = serde_json::to_value(&summary).unwrap();

    let pos = json["position_centroid"]
        .as_object()
        .expect("position_centroid should be object");
    assert!(pos.contains_key("latitude"));
    assert!(pos.contains_key("longitude"));
    assert!(pos.contains_key("altitude"));
}
