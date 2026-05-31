//! Tests for Cell CRDT operations

use super::*;
use crate::models::{Capability, CapabilityType};

#[test]
fn test_cell_add_remove_member() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    // Add members
    assert!(cell.add_member("node_1".to_string()));
    assert!(cell.add_member("node_2".to_string()));
    assert_eq!(cell.member_count(), 2);

    // Try to add duplicate
    assert!(!cell.add_member("node_1".to_string()));
    assert_eq!(cell.member_count(), 2);

    // Remove member
    assert!(cell.remove_member("node_1"));
    assert_eq!(cell.member_count(), 1);

    // Try to remove non-existent member
    assert!(!cell.remove_member("node_3"));
}

#[test]
fn test_cell_capacity() {
    let config = CellConfig::new(2);
    let mut cell = CellState::new(config);

    assert!(cell.add_member("node_1".to_string()));
    assert!(cell.add_member("node_2".to_string()));
    assert!(cell.is_full());

    // Can't add more when full
    assert!(!cell.add_member("node_3".to_string()));
}

#[test]
fn test_cell_leader_election() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.add_member("node_1".to_string());
    cell.add_member("node_2".to_string());

    // Set leader
    assert!(cell.set_leader("node_1".to_string()).is_ok());
    assert_eq!(cell.leader_id, Some("node_1".to_string()));
    assert!(cell.is_leader("node_1"));
    assert!(!cell.is_leader("node_2"));

    // Try to set non-member as leader
    assert!(cell.set_leader("node_3".to_string()).is_err());

    // Clear leader
    cell.clear_leader();
    assert_eq!(cell.leader_id, None);
}

#[test]
fn test_cell_leader_removal() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.add_member("node_1".to_string());
    cell.set_leader("node_1".to_string()).unwrap();

    // Remove leader - should clear leader_id
    cell.remove_member("node_1");
    assert_eq!(cell.leader_id, None);
}

#[test]
fn test_cell_capabilities() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    let cap1 = Capability::new(
        "camera_1".to_string(),
        "HD Camera".to_string(),
        CapabilityType::Sensor,
        0.9,
    );
    let cap2 = Capability::new(
        "gps_1".to_string(),
        "GPS".to_string(),
        CapabilityType::Sensor,
        1.0,
    );
    let cap3 = Capability::new(
        "compute_1".to_string(),
        "Edge Compute".to_string(),
        CapabilityType::Compute,
        0.8,
    );

    cell.add_capability(cap1.clone());
    cell.add_capability(cap2);
    cell.add_capability(cap3);

    assert_eq!(cell.capabilities.len(), 3);

    // Try to add duplicate
    cell.add_capability(cap1);
    assert_eq!(cell.capabilities.len(), 3);

    // Check capability types
    assert!(cell.has_capability_type(CapabilityType::Sensor));
    assert!(cell.has_capability_type(CapabilityType::Compute));
    assert!(!cell.has_capability_type(CapabilityType::Mobility));

    // Get by type
    let sensors = cell.get_capabilities_by_type(CapabilityType::Sensor);
    assert_eq!(sensors.len(), 2);
}

#[test]
fn test_cell_cohort_assignment() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    assert_eq!(cell.cohort_id, None);

    cell.assign_cohort("cohort_1".to_string());
    assert_eq!(cell.cohort_id, Some("cohort_1".to_string()));

    cell.leave_cohort();
    assert_eq!(cell.cohort_id, None);
}

#[test]
fn test_cell_merge() {
    let config = CellConfig::new(5);
    let mut cell1 = CellState::new(config.clone());
    let cell2 = CellState::new(config);

    // Cell1 has some members
    cell1.add_member("node_1".to_string());
    cell1.add_member("node_2".to_string());

    // Cell2 has different members
    let mut cell2_temp = cell2.clone();
    cell2_temp.add_member("node_2".to_string());
    cell2_temp.add_member("node_3".to_string());

    // Merge cell2 into cell1
    cell1.merge(&cell2_temp);

    // Should have union of members
    assert_eq!(cell1.member_count(), 3);
    assert!(cell1.is_member("node_1"));
    assert!(cell1.is_member("node_2"));
    assert!(cell1.is_member("node_3"));
}

#[test]
fn test_cell_merge_leader() {
    let config = CellConfig::new(5);
    let mut cell1 = CellState::new(config.clone());
    let mut cell2 = CellState::new(config);

    cell1.add_member("node_1".to_string());
    cell2.add_member("node_2".to_string());

    cell1.set_leader("node_1".to_string()).unwrap();

    // Cell2 has a later leader update
    std::thread::sleep(std::time::Duration::from_secs(1));
    cell2.set_leader("node_2".to_string()).unwrap();

    // Merge - cell2's leader should win (newer timestamp)
    cell1.merge(&cell2);
    assert_eq!(cell1.leader_id, Some("node_2".to_string()));
}

#[test]
fn test_cell_merge_capabilities() {
    let config = CellConfig::new(5);
    let mut cell1 = CellState::new(config.clone());
    let mut cell2 = CellState::new(config);

    let cap1 = Capability::new(
        "camera".to_string(),
        "Camera".to_string(),
        CapabilityType::Sensor,
        0.9,
    );
    let cap2 = Capability::new(
        "gps".to_string(),
        "GPS".to_string(),
        CapabilityType::Sensor,
        1.0,
    );

    cell1.add_capability(cap1);
    cell2.add_capability(cap2);

    cell1.merge(&cell2);

    // Should have union of capabilities
    assert_eq!(cell1.capabilities.len(), 2);
}

#[test]
fn test_cell_is_valid() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    // Not valid with 0 members (min_size is 2)
    assert!(!cell.is_valid());

    cell.add_member("node_1".to_string());
    assert!(!cell.is_valid());

    cell.add_member("node_2".to_string());
    assert!(cell.is_valid());
}

#[test]
fn test_cell_config_with_id() {
    let custom_id = "custom_cell_id".to_string();
    let config = CellConfig::with_id(custom_id.clone(), 10);

    assert_eq!(config.id, custom_id);
    assert_eq!(config.max_size, 10);
    assert_eq!(config.min_size, 2);
    assert!(config.created_at.is_some());
}

#[test]
fn test_cell_config_new_generates_uuid() {
    let config1 = CellConfig::new(5);
    let config2 = CellConfig::new(5);

    // Each config should have a unique ID
    assert_ne!(config1.id, config2.id);
}

#[test]
fn test_cell_state_get_id() {
    let config = CellConfig::with_id("test_id".to_string(), 5);
    let cell = CellState::new(config);

    assert_eq!(cell.get_id(), Some("test_id"));
}

#[test]
fn test_cell_state_get_id_no_config() {
    let mut cell = CellState::new(CellConfig::new(5));
    cell.config = None;

    assert_eq!(cell.get_id(), None);
}

#[test]
fn test_cell_add_member_when_full() {
    let config = CellConfig::new(2);
    let mut cell = CellState::new(config);

    assert!(cell.add_member("node_1".to_string()));
    assert!(cell.add_member("node_2".to_string()));
    assert!(cell.is_full());

    // Try to add when full - should fail
    assert!(!cell.add_member("node_3".to_string()));
    assert_eq!(cell.member_count(), 2);
}

#[test]
fn test_cell_remove_non_existent_member() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.add_member("node_1".to_string());

    // Try to remove member that doesn't exist
    assert!(!cell.remove_member("node_2"));
    assert!(!cell.remove_member(""));
    assert_eq!(cell.member_count(), 1);
}

#[test]
fn test_cell_set_leader_not_member() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.add_member("node_1".to_string());

    // Try to set leader who isn't a member
    let result = cell.set_leader("node_2".to_string());
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Leader must be a cell member");
}

#[test]
fn test_cell_clear_leader() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.add_member("node_1".to_string());
    cell.set_leader("node_1".to_string()).unwrap();
    assert!(cell.is_leader("node_1"));

    cell.clear_leader();
    assert!(!cell.is_leader("node_1"));
    assert_eq!(cell.leader_id, None);
}

#[test]
fn test_cell_is_member() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.add_member("node_1".to_string());
    cell.add_member("node_2".to_string());

    assert!(cell.is_member("node_1"));
    assert!(cell.is_member("node_2"));
    assert!(!cell.is_member("node_3"));
    assert!(!cell.is_member(""));
}

#[test]
fn test_cell_capabilities_duplicate_handling() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    let cap = Capability::new(
        "cap_1".to_string(),
        "Capability 1".to_string(),
        CapabilityType::Sensor,
        0.9,
    );

    cell.add_capability(cap.clone());
    assert_eq!(cell.capabilities.len(), 1);

    // Add duplicate - should not increase count
    cell.add_capability(cap.clone());
    assert_eq!(cell.capabilities.len(), 1);

    // Add capability with different ID
    let cap2 = Capability::new(
        "cap_2".to_string(),
        "Capability 2".to_string(),
        CapabilityType::Sensor,
        0.8,
    );
    cell.add_capability(cap2);
    assert_eq!(cell.capabilities.len(), 2);
}

#[test]
fn test_cell_has_capability_type_empty() {
    let config = CellConfig::new(5);
    let cell = CellState::new(config);

    // No capabilities initially
    assert!(!cell.has_capability_type(CapabilityType::Sensor));
    assert!(!cell.has_capability_type(CapabilityType::Compute));
}

#[test]
fn test_cell_get_capabilities_by_type_empty() {
    let config = CellConfig::new(5);
    let cell = CellState::new(config);

    let caps = cell.get_capabilities_by_type(CapabilityType::Sensor);
    assert_eq!(caps.len(), 0);
}

#[test]
fn test_cell_merge_empty_cells() {
    let config = CellConfig::new(5);
    let mut cell1 = CellState::new(config.clone());
    let cell2 = CellState::new(config);

    // Both empty
    cell1.merge(&cell2);
    assert_eq!(cell1.member_count(), 0);
    assert_eq!(cell1.capabilities.len(), 0);
}

#[test]
fn test_cell_merge_with_older_timestamp() {
    let config = CellConfig::new(5);
    let mut cell1 = CellState::new(config.clone());
    let mut cell2 = CellState::new(config);

    cell1.add_member("node_1".to_string());

    // Update cell1's timestamp to be newer
    std::thread::sleep(std::time::Duration::from_millis(10));
    cell1.set_leader("node_1".to_string()).unwrap();

    // cell2 is older - its leader shouldn't win
    cell2.add_member("node_2".to_string());

    // Merge older cell2 into newer cell1
    cell1.merge(&cell2);

    // cell1's leader should remain
    assert_eq!(cell1.leader_id, Some("node_1".to_string()));

    // But members should be merged
    assert_eq!(cell1.member_count(), 2);
}

#[test]
fn test_cell_cohort_assignment_multiple_times() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    cell.assign_cohort("cohort_1".to_string());
    assert_eq!(cell.cohort_id, Some("cohort_1".to_string()));

    // Reassign to different cohort
    cell.assign_cohort("cohort_2".to_string());
    assert_eq!(cell.cohort_id, Some("cohort_2".to_string()));

    // Leave cohort
    cell.leave_cohort();
    assert_eq!(cell.cohort_id, None);
}

#[test]
fn test_cell_is_full_no_config() {
    let mut cell = CellState::new(CellConfig::new(5));
    cell.config = None;

    // Should return false when no config
    assert!(!cell.is_full());
}

#[test]
fn test_cell_is_valid_no_config() {
    let mut cell = CellState::new(CellConfig::new(5));
    cell.add_member("node_1".to_string());
    cell.add_member("node_2".to_string());

    cell.config = None;

    // Should return false when no config
    assert!(!cell.is_valid());
}

#[test]
fn test_cell_update_timestamp() {
    let config = CellConfig::new(5);
    let mut cell = CellState::new(config);

    let initial_ts = cell.timestamp.as_ref().map(|t| t.seconds).unwrap_or(0);

    std::thread::sleep(std::time::Duration::from_millis(10));
    cell.update_timestamp();

    let new_ts = cell.timestamp.as_ref().map(|t| t.seconds).unwrap_or(0);
    assert!(new_ts >= initial_ts);
}

#[test]
fn test_cell_config_default_min_size() {
    let config = CellConfig::new(10);

    // min_size should always be 2
    assert_eq!(config.min_size, 2);
}

#[test]
fn test_cell_merge_capabilities_union() {
    let config = CellConfig::new(5);
    let mut cell1 = CellState::new(config.clone());
    let mut cell2 = CellState::new(config);

    let cap1 = Capability::new(
        "cap_1".to_string(),
        "Cap 1".to_string(),
        CapabilityType::Sensor,
        0.9,
    );
    let cap2 = Capability::new(
        "cap_2".to_string(),
        "Cap 2".to_string(),
        CapabilityType::Compute,
        0.8,
    );
    let cap3 = Capability::new(
        "cap_3".to_string(),
        "Cap 3".to_string(),
        CapabilityType::Mobility,
        0.7,
    );

    cell1.add_capability(cap1.clone());
    cell1.add_capability(cap2.clone());

    cell2.add_capability(cap2.clone()); // Duplicate
    cell2.add_capability(cap3);

    cell1.merge(&cell2);

    // Should have 3 unique capabilities
    assert_eq!(cell1.capabilities.len(), 3);
}

#[test]
fn test_cell_merge_members_union() {
    let config = CellConfig::new(10);
    let mut cell1 = CellState::new(config.clone());
    let mut cell2 = CellState::new(config);

    cell1.add_member("node_1".to_string());
    cell1.add_member("node_2".to_string());

    cell2.add_member("node_2".to_string()); // Duplicate
    cell2.add_member("node_3".to_string());
    cell2.add_member("node_4".to_string());

    cell1.merge(&cell2);

    // Should have 4 unique members
    assert_eq!(cell1.member_count(), 4);
    assert!(cell1.is_member("node_1"));
    assert!(cell1.is_member("node_2"));
    assert!(cell1.is_member("node_3"));
    assert!(cell1.is_member("node_4"));
}
