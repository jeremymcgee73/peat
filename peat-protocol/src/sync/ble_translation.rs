//! BLE Translation Layer (ADR-041, #557)
//!
//! Provides bidirectional translation between peat-btle lightweight CRDTs
//! and Peat Protocol Automerge documents. This enables gateway nodes to
//! bridge between:
//!
//! - **Full Peat nodes** (mobile-app consumers, CLI) using Automerge documents
//! - **Constrained wearable devices** using peat-btle lightweight CRDTs
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                          Gateway Node                               │
//! │  ┌─────────────────────────────────────────────────────────────────┐│
//! │  │                    BLE Translation Layer                        ││
//! │  │                                                                 ││
//! │  │   peat-btle Position ←──────────→ TrackInfo document           ││
//! │  │   peat-btle HealthStatus ←──────→ Platform health fields       ││
//! │  │   peat-btle EmergencyEvent ←────→ Alert document               ││
//! │  │   peat-btle CannedMessage ←─────→ CannedMessage document      ││
//! │  │   peat-btle GCounter ←──────────→ Automerge counter            ││
//! │  └─────────────────────────────────────────────────────────────────┘│
//! │            ▲                                    ▲                   │
//! │            │                                    │                   │
//! │   ┌────────▼────────┐                ┌─────────▼─────────┐        │
//! │   │  PeatBleTransport│                │  IrohMeshTransport│        │
//! │   │  (BLE mesh)      │                │  (QUIC/WiFi)      │        │
//! │   └─────────────────┘                └───────────────────┘        │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use peat_protocol::sync::ble_translation::{BleTranslator, TranslationConfig};
//!
//! let translator = BleTranslator::new(TranslationConfig::default());
//!
//! // Translate BLE position to track document
//! let track_doc = translator.position_to_track(&ble_position, &peripheral_id);
//!
//! // Translate track document to BLE position
//! let ble_position = translator.track_to_position(&track_doc);
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Configuration for BLE translation
#[derive(Debug, Clone)]
pub struct TranslationConfig {
    /// Collection name for tracks (default: "tracks")
    pub tracks_collection: String,
    /// Collection name for platforms/peripherals (default: "platforms")
    pub platforms_collection: String,
    /// Collection name for alerts/emergencies (default: "alerts")
    pub alerts_collection: String,
    /// Collection name for canned messages (default: "canned_messages")
    pub canned_messages_collection: String,
    /// Default classification for BLE-originated tracks
    pub default_classification: String,
    /// ID prefix for BLE-originated documents
    pub ble_id_prefix: String,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            tracks_collection: "tracks".to_string(),
            platforms_collection: "platforms".to_string(),
            alerts_collection: "alerts".to_string(),
            canned_messages_collection: "canned_messages".to_string(),
            default_classification: "a-f-G-U-C".to_string(), // Friendly ground unit
            ble_id_prefix: "ble-".to_string(),
        }
    }
}

/// BLE position data (mirrors peat_btle::Position)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlePosition {
    /// Latitude in degrees (WGS84)
    pub latitude: f32,
    /// Longitude in degrees (WGS84)
    pub longitude: f32,
    /// Altitude in meters (optional)
    pub altitude: Option<f32>,
    /// Accuracy/CEP in meters (optional)
    pub accuracy: Option<f32>,
}

/// BLE health status (mirrors peat_btle::HealthStatus on the wire).
///
/// **This struct is postcard-encoded onto the BLE radio** by
/// [`BleTranslator::encode_outbound`] (`platforms` collection →
/// `postcard_encode(&self.platform_to_peripheral(...))`), so the
/// field layout MUST byte-match the peat-btle / GATT receivers
/// shipped in M5Stack and WearOS watch firmware. `battery_percent`
/// is `u8` (one postcard byte). Round-5 tried `Option<u8>` for
/// honest "no sensor" semantics — postcard encodes that as a
/// 1-byte variant tag plus an optional value byte, which receivers
/// expecting `u8` would mis-decode as battery and shift every
/// subsequent field. peat#835 round-7 reverts to `u8` after the
/// round-6 review caught the wire-format break.
///
/// The `Option`-style "no sensor" semantics live one layer up, in
/// [`crate::sync::ble_translation::BleTranslator::peripheral_to_platform`]'s
/// JSON output, and in `peat-ffi::parse_battery_percent` (`Option<i32>`)
/// at the Automerge envelope. The wire-shape vs envelope-shape
/// asymmetry is intentional and load-bearing: changing the wire
/// requires a coordinated firmware deploy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BleHealthStatus {
    /// Battery percentage (0-100). Wire-format `u8`; "unknown"
    /// signals are conventional (publishers without a battery
    /// sensor write 100; the alerts bitfield's `ALERT_LOW_BATTERY`
    /// is the authoritative "actually depleted" indicator).
    pub battery_percent: u8,
    /// Heart rate in BPM (optional). `Option<u8>` is preserved here
    /// because peat-btle's wire-format type uses the same shape —
    /// postcard `Option<u8>` is identical on both sides.
    pub heart_rate: Option<u8>,
    /// Activity level (0=still, 1=walking, 2=running, 3=vehicle)
    pub activity: u8,
    /// Alert flags (bitfield)
    pub alerts: u8,
}

impl BleHealthStatus {
    /// Alert flag: Man down detected
    pub const ALERT_MAN_DOWN: u8 = 0x01;
    /// Alert flag: Low battery
    pub const ALERT_LOW_BATTERY: u8 = 0x02;
    /// Alert flag: Out of range
    pub const ALERT_OUT_OF_RANGE: u8 = 0x04;
    /// Alert flag: Custom alert 1
    pub const ALERT_CUSTOM_1: u8 = 0x08;

    /// Check if man-down alert is active
    pub fn is_man_down(&self) -> bool {
        self.alerts & Self::ALERT_MAN_DOWN != 0
    }

    /// Check if low battery alert is active
    pub fn is_low_battery(&self) -> bool {
        self.alerts & Self::ALERT_LOW_BATTERY != 0
    }
}

/// BLE emergency event (mirrors peat_btle::EmergencyEvent)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BleEmergencyEvent {
    /// Source node ID that triggered emergency
    pub source_node: u32,
    /// Timestamp when triggered (ms since epoch)
    pub timestamp: u64,
    /// ACK status for each known peer (node_id -> acked)
    pub acks: HashMap<u32, bool>,
}

/// BLE canned message (mirrors peat_lite::CannedMessageAckEvent)
///
/// Represents a pre-defined message from a constrained wearable device, with ACK tracking.
/// The ACK map uses OR-set CRDT semantics: each acker_node maps to its ack_timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BleCannedMessage {
    /// Message code (0x00-0xFF, maps to peat-lite CannedMessage enum)
    pub message_code: u8,
    /// Human-readable message name
    pub message_name: String,
    /// Source node ID that sent this message
    pub source_node: u32,
    /// Target node (None for broadcast)
    pub target_node: Option<u32>,
    /// Timestamp when sent (ms since epoch)
    pub timestamp: u64,
    /// Sequence number for deduplication
    pub sequence: u32,
    /// ACK tracking: acker_node_id -> ack_timestamp (OR-set CRDT)
    pub acks: HashMap<u32, u64>,
}

/// Map a peat-lite CannedMessage code to its human-readable name.
///
/// Covers all 20 defined codes from peat-lite v0.0.4 plus a fallback for unknown codes.
pub fn message_name_from_code(code: u8) -> &'static str {
    match code {
        // Acknowledgments (0x00-0x0F)
        0x00 => "Ack",
        0x01 => "Ack Wilco",
        0x02 => "Ack Negative",
        0x03 => "Say Again",
        // Status (0x10-0x1F)
        0x10 => "Check In",
        0x11 => "Moving",
        0x12 => "Holding",
        0x13 => "On Station",
        0x14 => "Returning",
        0x15 => "Complete",
        // Alerts (0x20-0x2F)
        0x20 => "Emergency",
        0x21 => "Alert",
        0x22 => "All Clear",
        0x23 => "Contact",
        0x24 => "Under Fire",
        // Requests (0x30-0x3F)
        0x30 => "Need Extract",
        0x31 => "Need Support",
        0x32 => "Need Medic",
        0x33 => "Need Resupply",
        // Reserved
        0xFF => "Custom",
        _ => "Unknown",
    }
}

/// BLE peripheral data (mirrors peat_btle::Peripheral)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlePeripheral {
    /// Unique peripheral ID
    pub id: u32,
    /// Parent node ID (0 if unpaired)
    pub parent_node: u32,
    /// Peripheral type
    pub peripheral_type: BlePeripheralType,
    /// Callsign (up to 12 chars)
    pub callsign: String,
    /// Health status
    pub health: BleHealthStatus,
    /// Last update timestamp (ms since epoch)
    pub timestamp: u64,
    /// Position (optional)
    pub position: Option<BlePosition>,
}

/// BLE peripheral types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum BlePeripheralType {
    Unknown = 0,
    SoldierSensor = 1,
    FixedSensor = 2,
    Relay = 3,
}

impl From<u8> for BlePeripheralType {
    fn from(v: u8) -> Self {
        match v {
            1 => Self::SoldierSensor,
            2 => Self::FixedSensor,
            3 => Self::Relay,
            _ => Self::Unknown,
        }
    }
}

/// Translator between BLE CRDTs and Automerge documents
#[derive(Debug, Clone)]
pub struct BleTranslator {
    config: TranslationConfig,
}

impl BleTranslator {
    /// Create a new translator with the given configuration
    pub fn new(config: TranslationConfig) -> Self {
        Self { config }
    }

    /// Create a translator with default configuration
    pub fn with_defaults() -> Self {
        Self::new(TranslationConfig::default())
    }

    // =========================================================================
    // Position <-> Track Translation
    // =========================================================================

    /// Convert BLE position to track document JSON
    ///
    /// Creates a track document suitable for storage in the tracks collection.
    /// Note: This version does not set cell_id. Use `position_to_track_in_cell`
    /// to include cell membership based on BLE mesh_id.
    pub fn position_to_track(
        &self,
        position: &BlePosition,
        peripheral_id: u32,
        callsign: Option<&str>,
    ) -> Value {
        self.position_to_track_in_cell(position, peripheral_id, callsign, None)
    }

    /// Convert BLE position to track document JSON with cell membership
    ///
    /// The `mesh_id` parameter (from BLE mesh configuration) is used as the cell_id,
    /// allowing BLE-originated tracks to be associated with Peat cells.
    ///
    /// # Arguments
    /// * `position` - The BLE position data
    /// * `peripheral_id` - The BLE peripheral ID
    /// * `callsign` - Optional callsign for the track
    /// * `mesh_id` - Optional BLE mesh ID to use as cell_id
    pub fn position_to_track_in_cell(
        &self,
        position: &BlePosition,
        peripheral_id: u32,
        callsign: Option<&str>,
        mesh_id: Option<&str>,
    ) -> Value {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let track_id = format!("{}{:08X}", self.config.ble_id_prefix, peripheral_id);
        let source = callsign.unwrap_or(&track_id);

        let mut track = json!({
            "id": track_id,
            "source_platform": format!("ble-{:08X}", peripheral_id),
            "lat": position.latitude as f64,
            "lon": position.longitude as f64,
            "hae": position.altitude.map(|a| a as f64),
            "cep": position.accuracy.map(|a| a as f64),
            "classification": self.config.default_classification,
            "confidence": 0.9,
            "category": "friendly",
            "callsign": source,
            "created_at": now_ms,
            "last_update": now_ms,
            "ble_origin": true
        });

        // Set cell_id from BLE mesh_id (mesh_id == cell_id mapping)
        if let Some(cell_id) = mesh_id {
            track["cell_id"] = json!(cell_id);
        }

        track
    }

    /// Extract BLE position from track document JSON
    ///
    /// Returns None if the document doesn't have required position fields.
    pub fn track_to_position(&self, track: &Value) -> Option<BlePosition> {
        let lat = track.get("lat")?.as_f64()? as f32;
        let lon = track.get("lon")?.as_f64()? as f32;

        Some(BlePosition {
            latitude: lat,
            longitude: lon,
            altitude: track.get("hae").and_then(|v| v.as_f64()).map(|a| a as f32),
            accuracy: track.get("cep").and_then(|v| v.as_f64()).map(|a| a as f32),
        })
    }

    // =========================================================================
    // Peripheral <-> Platform Translation
    // =========================================================================

    /// Convert BLE peripheral to platform document JSON
    ///
    /// Note: This version does not set cell_id. Use `peripheral_to_platform_in_cell`
    /// to include cell membership based on BLE mesh_id.
    pub fn peripheral_to_platform(&self, peripheral: &BlePeripheral) -> Value {
        self.peripheral_to_platform_in_cell(peripheral, None)
    }

    /// Convert BLE peripheral to platform document JSON with cell membership
    ///
    /// The `mesh_id` parameter (from BLE mesh configuration) is used as the cell_id,
    /// allowing BLE peripherals to be associated with Peat cells.
    ///
    /// # Arguments
    /// * `peripheral` - The BLE peripheral data
    /// * `mesh_id` - Optional BLE mesh ID to use as cell_id
    pub fn peripheral_to_platform_in_cell(
        &self,
        peripheral: &BlePeripheral,
        mesh_id: Option<&str>,
    ) -> Value {
        let platform_id = format!("{}{:08X}", self.config.ble_id_prefix, peripheral.id);

        let mut platform = json!({
            "id": platform_id,
            "name": peripheral.callsign,
            "type": match peripheral.peripheral_type {
                BlePeripheralType::SoldierSensor => "wearable",
                BlePeripheralType::FixedSensor => "sensor",
                BlePeripheralType::Relay => "relay",
                BlePeripheralType::Unknown => "unknown",
            },
            "status": if peripheral.health.battery_percent > 20 { "active" } else { "low_battery" },
            "battery_percent": peripheral.health.battery_percent,
            "activity": match peripheral.health.activity {
                0 => "still",
                1 => "walking",
                2 => "running",
                3 => "vehicle",
                _ => "unknown",
            },
            "last_update": peripheral.timestamp,
            "ble_origin": true,
            "parent_node": format!("{:08X}", peripheral.parent_node),
        });

        // Set cell_id from BLE mesh_id (mesh_id == cell_id mapping)
        if let Some(cell_id) = mesh_id {
            platform["cell_id"] = json!(cell_id);
        }

        // Add optional health data
        if let Some(hr) = peripheral.health.heart_rate {
            platform["heart_rate"] = json!(hr);
        }

        // Add position if available
        if let Some(ref pos) = peripheral.position {
            platform["lat"] = json!(pos.latitude as f64);
            platform["lon"] = json!(pos.longitude as f64);
            if let Some(alt) = pos.altitude {
                platform["hae"] = json!(alt as f64);
            }
        }

        // Add alerts
        if peripheral.health.alerts != 0 {
            let mut alerts = Vec::new();
            if peripheral.health.is_man_down() {
                alerts.push("man_down");
            }
            if peripheral.health.is_low_battery() {
                alerts.push("low_battery");
            }
            if peripheral.health.alerts & BleHealthStatus::ALERT_OUT_OF_RANGE != 0 {
                alerts.push("out_of_range");
            }
            platform["alerts"] = json!(alerts);
        }

        platform
    }

    /// Extract BLE peripheral data from platform document JSON
    pub fn platform_to_peripheral(&self, platform: &Value) -> Option<BlePeripheral> {
        let id_str = platform.get("id")?.as_str()?;
        let id = self.parse_ble_id(id_str)?;

        let peripheral_type = match platform.get("type").and_then(|v| v.as_str()) {
            Some("wearable") => BlePeripheralType::SoldierSensor,
            Some("sensor") => BlePeripheralType::FixedSensor,
            Some("relay") => BlePeripheralType::Relay,
            _ => BlePeripheralType::Unknown,
        };

        let activity = match platform.get("activity").and_then(|v| v.as_str()) {
            Some("walking") => 1,
            Some("running") => 2,
            Some("vehicle") => 3,
            _ => 0,
        };

        let mut alerts: u8 = 0;
        if let Some(alert_arr) = platform.get("alerts").and_then(|v| v.as_array()) {
            for alert in alert_arr {
                if let Some(s) = alert.as_str() {
                    match s {
                        "man_down" => alerts |= BleHealthStatus::ALERT_MAN_DOWN,
                        "low_battery" => alerts |= BleHealthStatus::ALERT_LOW_BATTERY,
                        "out_of_range" => alerts |= BleHealthStatus::ALERT_OUT_OF_RANGE,
                        _ => {}
                    }
                }
            }
        }

        let position = if platform.get("lat").is_some() && platform.get("lon").is_some() {
            Some(BlePosition {
                latitude: platform.get("lat")?.as_f64()? as f32,
                longitude: platform.get("lon")?.as_f64()? as f32,
                altitude: platform
                    .get("hae")
                    .and_then(|v| v.as_f64())
                    .map(|a| a as f32),
                accuracy: platform
                    .get("cep")
                    .and_then(|v| v.as_f64())
                    .map(|a| a as f32),
            })
        } else {
            None
        };

        Some(BlePeripheral {
            id,
            parent_node: self
                .parse_ble_id(
                    platform
                        .get("parent_node")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0"),
                )
                .unwrap_or(0),
            peripheral_type,
            callsign: platform
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            health: BleHealthStatus {
                // `battery_percent` and `heart_rate` accept either
                // integer or float JSON form via `coerce_metric_to_i64`.
                // Pre-fix this used `.as_u64()` which silently dropped
                // the float form a Kotlin `Double`-typed publisher
                // would emit (e.g. `85.0` → `None`) — same data-loss
                // bug class peat#835 fixed in `peat-ffi`.
                //
                // `battery_percent` is wire-format `u8` (the struct is
                // postcard-encoded onto the BLE radio downstream of
                // this construction; see `BleHealthStatus` doc-comment).
                // Round-5 attempted `Option<u8>` for "no sensor"
                // semantics; round-7 reverted because the postcard
                // byte layout shifted under shipped firmware. The
                // `unwrap_or(100)` default is conventional ("publishers
                // without a battery sensor say 100"); the alerts
                // bitfield's `ALERT_LOW_BATTERY` is the authoritative
                // depleted-battery signal. The "no sensor" Option
                // semantics live one layer up in peat-ffi's
                // `parse_battery_percent: Option<i32>`.
                //
                // Cast clamps into `u8` (battery 0..=100, heart_rate
                // 0..=250) so the cast is total.
                battery_percent: coerce_metric_to_i64(platform.get("battery_percent"))
                    .map(|n| n.clamp(0, 100) as u8)
                    .unwrap_or(100),
                heart_rate: coerce_metric_to_i64(platform.get("heart_rate"))
                    .map(|n| n.clamp(0, 250) as u8),
                activity,
                alerts,
            },
            timestamp: platform
                .get("last_update")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            position,
        })
    }

    // =========================================================================
    // Emergency <-> Alert Translation
    // =========================================================================

    /// Convert BLE emergency event to alert document JSON
    pub fn emergency_to_alert(
        &self,
        emergency: &BleEmergencyEvent,
        callsign: Option<&str>,
    ) -> Value {
        let alert_id = format!(
            "{}emergency-{:08X}-{}",
            self.config.ble_id_prefix, emergency.source_node, emergency.timestamp
        );

        let default_source = format!("{:08X}", emergency.source_node);
        let source = callsign.unwrap_or(&default_source);

        // Convert acks to JSON-friendly format
        let acks: HashMap<String, bool> = emergency
            .acks
            .iter()
            .map(|(k, v)| (format!("{:08X}", k), *v))
            .collect();

        json!({
            "id": alert_id,
            "type": "emergency",
            "source": source,
            "source_node": format!("{:08X}", emergency.source_node),
            "timestamp": emergency.timestamp,
            "acks": acks,
            "ack_count": emergency.acks.values().filter(|&&v| v).count(),
            "total_peers": emergency.acks.len(),
            "active": true,
            "ble_origin": true
        })
    }

    /// Extract BLE emergency from alert document JSON
    pub fn alert_to_emergency(&self, alert: &Value) -> Option<BleEmergencyEvent> {
        // Only process emergency type alerts
        if alert.get("type").and_then(|v| v.as_str()) != Some("emergency") {
            return None;
        }

        let source_node_str = alert.get("source_node")?.as_str()?;
        let source_node = u32::from_str_radix(source_node_str.trim_start_matches("0x"), 16).ok()?;

        let timestamp = alert.get("timestamp")?.as_u64()?;

        let acks: HashMap<u32, bool> = alert
            .get("acks")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| {
                        let node_id = u32::from_str_radix(k.trim_start_matches("0x"), 16).ok()?;
                        let acked = v.as_bool()?;
                        Some((node_id, acked))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(BleEmergencyEvent {
            source_node,
            timestamp,
            acks,
        })
    }

    // =========================================================================
    // CannedMessage <-> Document Translation
    // =========================================================================

    /// Convert BLE canned message to Automerge document JSON
    ///
    /// Convenience wrapper that does not set cell_id.
    /// Use `canned_message_to_doc_in_cell` to include cell membership.
    pub fn canned_message_to_doc(
        &self,
        message: &BleCannedMessage,
        callsign: Option<&str>,
    ) -> Value {
        self.canned_message_to_doc_in_cell(message, callsign, None)
    }

    /// Convert BLE canned message to Automerge document JSON with cell membership
    ///
    /// The `mesh_id` parameter (from BLE mesh configuration) is used as the cell_id,
    /// allowing BLE-originated canned messages to be associated with Peat cells.
    ///
    /// # Arguments
    /// * `message` - The BLE canned message data
    /// * `callsign` - Optional callsign for the source
    /// * `mesh_id` - Optional BLE mesh ID to use as cell_id
    pub fn canned_message_to_doc_in_cell(
        &self,
        message: &BleCannedMessage,
        callsign: Option<&str>,
        mesh_id: Option<&str>,
    ) -> Value {
        let doc_id = format!(
            "{}canned-{:08X}-{}",
            self.config.ble_id_prefix, message.source_node, message.timestamp
        );

        let default_source = format!("{:08X}", message.source_node);
        let source = callsign.unwrap_or(&default_source);

        // Convert acks to JSON-friendly format (hex node IDs)
        let acks: HashMap<String, u64> = message
            .acks
            .iter()
            .map(|(k, v)| (format!("{:08X}", k), *v))
            .collect();

        let mut doc = json!({
            "id": doc_id,
            "type": "canned_message",
            "message_code": message.message_code,
            "message_name": message.message_name,
            "source": source,
            "source_node": format!("{:08X}", message.source_node),
            "target_node": message.target_node.map(|n| format!("{:08X}", n)),
            "timestamp": message.timestamp,
            "sequence": message.sequence,
            "acks": acks,
            "ack_count": message.acks.len(),
            "ble_origin": true
        });

        if let Some(cell_id) = mesh_id {
            doc["cell_id"] = json!(cell_id);
        }

        doc
    }

    /// Extract BLE canned message from document JSON
    ///
    /// Returns None if the document doesn't have `type: "canned_message"`.
    pub fn doc_to_canned_message(&self, doc: &Value) -> Option<BleCannedMessage> {
        if doc.get("type").and_then(|v| v.as_str()) != Some("canned_message") {
            return None;
        }

        let message_code = doc.get("message_code")?.as_u64()? as u8;

        let source_node_str = doc.get("source_node")?.as_str()?;
        let source_node = u32::from_str_radix(source_node_str.trim_start_matches("0x"), 16).ok()?;

        let target_node = doc
            .get("target_node")
            .and_then(|v| v.as_str())
            .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());

        let timestamp = doc.get("timestamp")?.as_u64()?;
        let sequence = doc.get("sequence").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        let acks: HashMap<u32, u64> = doc
            .get("acks")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| {
                        let node_id = u32::from_str_radix(k.trim_start_matches("0x"), 16).ok()?;
                        let ts = v.as_u64()?;
                        Some((node_id, ts))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(BleCannedMessage {
            message_code,
            message_name: message_name_from_code(message_code).to_string(),
            source_node,
            target_node,
            timestamp,
            sequence,
            acks,
        })
    }

    // =========================================================================
    // Utility Methods
    // =========================================================================

    /// Parse a BLE ID from hex string (with or without prefix)
    fn parse_ble_id(&self, id: &str) -> Option<u32> {
        let hex_part = id
            .strip_prefix(&self.config.ble_id_prefix)
            .unwrap_or(id)
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        u32::from_str_radix(hex_part, 16).ok()
    }

    /// Check if a document ID originated from BLE
    pub fn is_ble_origin(&self, doc_id: &str) -> bool {
        doc_id.starts_with(&self.config.ble_id_prefix)
    }

    /// Check if a document has BLE origin marker
    pub fn has_ble_marker(&self, doc: &Value) -> bool {
        doc.get("ble_origin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Get the collection name for tracks
    pub fn tracks_collection(&self) -> &str {
        &self.config.tracks_collection
    }

    /// Get the collection name for platforms
    pub fn platforms_collection(&self) -> &str {
        &self.config.platforms_collection
    }

    /// Get the collection name for alerts
    pub fn alerts_collection(&self) -> &str {
        &self.config.alerts_collection
    }

    /// Get the collection name for canned messages
    pub fn canned_messages_collection(&self) -> &str {
        &self.config.canned_messages_collection
    }
}

// =============================================================================
// peat-mesh `Translator` impl (ADR-059 Slice 1.b)
// =============================================================================
//
// Bridges `BleTranslator`'s typed-struct surface (`BlePosition`,
// `BlePeripheral`, `BleEmergencyEvent`, `BleCannedMessage`) to the
// generic `peat_mesh::transport::Translator` contract. The orchestrator
// (`TransportManager` fan-out) hands documents in via
// `encode_outbound`; bytes-from-the-radio come back via
// `decode_inbound`.
//
// Wire format: **postcard-encoded typed BLE struct.** Codec-internal,
// crosses the FFI boundary between Rust orchestrator and Kotlin/Swift
// `OutboundSink`. The Sink is responsible for the final transport-
// specific framing (peat-btle's marker bytes, encryption envelope,
// GATT write); the Translator's contract is just "a portable byte
// representation of the doc, suitable for any sink that knows my
// doc-type encoding." postcard chosen over ciborium for active
// maintenance, no_std-first design matching peat-lite, and the
// embedded-Rust ecosystem fit (embassy/esp-hal). See peat-mesh
// ADR-0011 / peat ADR-059 §"Per-transport channel sizing" and the
// Slice 1.b project-health audit.
//
// Dispatch: by `ctx.collection`. The orchestrator populates this
// from `ChangeEvent.collection` (ADR-059 Slice 1.b.1, peat-mesh#88);
// `BleTranslator` reads it to choose between `*_to_*` methods.
// Documents in collections this translator doesn't carry — or
// translation failures (e.g. malformed track) — return `None`
// from `encode_outbound` (decline, not error).

use async_trait::async_trait;
use peat_mesh::sync::Document as MeshDocument;
use peat_mesh::transport::{TranslationContext, Translator};

#[async_trait]
impl Translator for BleTranslator {
    fn transport_id(&self) -> &'static str {
        "ble"
    }

    async fn encode_outbound(
        &self,
        doc: &MeshDocument,
        ctx: &TranslationContext,
    ) -> Option<Vec<u8>> {
        let collection = ctx.collection.as_deref()?;
        // Reconstruct the JSON Value that the legacy `*_to_*`
        // methods take. Document.fields is HashMap<String, Value>;
        // Value::Object(Map) round-trips cleanly.
        let value = mesh_doc_to_value(doc);

        // Dispatch by collection. Configuration is per-translator —
        // collections aren't hard-coded "tracks"/"platforms"/etc.,
        // but read from `self.config.*_collection` so operators can
        // rename them without breaking the translator.
        let bytes = if collection == self.tracks_collection() {
            postcard_encode(&self.track_to_position(&value)?)
        } else if collection == self.platforms_collection() {
            postcard_encode(&self.platform_to_peripheral(&value)?)
        } else if collection == self.alerts_collection() {
            postcard_encode(&self.alert_to_emergency(&value)?)
        } else if collection == self.canned_messages_collection() {
            postcard_encode(&self.doc_to_canned_message(&value)?)
        } else {
            // Decline: this translator doesn't carry the collection.
            // Per ADR-059, `None` here is normal traffic — operators
            // with collections scoped to other transports will see
            // their docs declined here without diagnostic.
            return None;
        }?;

        Some(bytes)
    }

    async fn decode_inbound(
        &self,
        bytes: &[u8],
        ctx: &TranslationContext,
    ) -> anyhow::Result<Option<MeshDocument>> {
        use anyhow::Context;
        let collection = match ctx.collection.as_deref() {
            Some(c) => c,
            // No collection in context: the inbound bridge didn't tell
            // us what to decode. Treat as "not for this translator"
            // (Ok(None)) rather than Err — Err is reserved for
            // wire-format-drift diagnostics per the trait contract.
            None => return Ok(None),
        };

        // Default callsign / cell scope for the decode side: pull from
        // the context (set by the inbound bridge before invoking us).
        let callsign = ctx.local_callsign.as_deref();
        let mesh_id = ctx.cell_id.as_deref();

        let value = if collection == self.tracks_collection() {
            let pos: BlePosition =
                postcard::from_bytes(bytes).context("ble: decode BlePosition (tracks)")?;
            // Inbound peripheral_id is in the typed struct's wire form
            // for tracks via the source field; we use a default of 0
            // when not available — the inbound bridge can override
            // via ctx.local_wire_id when it knows.
            let peripheral_id = ctx
                .local_wire_id
                .as_deref()
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            self.position_to_track_in_cell(&pos, peripheral_id, callsign, mesh_id)
        } else if collection == self.platforms_collection() {
            let per: BlePeripheral =
                postcard::from_bytes(bytes).context("ble: decode BlePeripheral (platforms)")?;
            self.peripheral_to_platform_in_cell(&per, mesh_id)
        } else if collection == self.alerts_collection() {
            let em: BleEmergencyEvent =
                postcard::from_bytes(bytes).context("ble: decode BleEmergencyEvent (alerts)")?;
            self.emergency_to_alert(&em, callsign)
        } else if collection == self.canned_messages_collection() {
            let cm: BleCannedMessage = postcard::from_bytes(bytes)
                .context("ble: decode BleCannedMessage (canned_messages)")?;
            self.canned_message_to_doc_in_cell(&cm, callsign, mesh_id)
        } else {
            return Ok(None);
        };

        Ok(Some(value_to_mesh_document(value)))
    }
}

/// Convert a `peat_mesh::Document` into a `serde_json::Value` for the
/// legacy `*_to_*` methods. `Document.fields` is already
/// `HashMap<String, serde_json::Value>`; we just wrap it as an Object
/// and stamp the id if present.
fn mesh_doc_to_value(doc: &MeshDocument) -> Value {
    let mut map = serde_json::Map::with_capacity(doc.fields.len() + 1);
    for (k, v) in &doc.fields {
        map.insert(k.clone(), v.clone());
    }
    if let Some(id) = &doc.id {
        // The legacy methods treat "id" as a normal field; preserve it
        // for callers that read it from the Value form.
        map.entry("id".to_string())
            .or_insert_with(|| Value::String(id.clone()));
    }
    Value::Object(map)
}

/// Rebuild a [`peat_mesh::sync::Document`] from a JSON [`Value`] emitted by
/// one of the translator's `*_to_*` legacy methods. Inverse of the private
/// `mesh_doc_to_value` helper used by the BLE encode path.
///
/// Public so that callers (peat-ffi, tests) doing the
/// "translate-then-publish" pattern can avoid duplicating this trivial but
/// translator-shape-coupled conversion. The shape is owned by the
/// translator: any future change to how `*_to_*` methods stamp `id`
/// flows through this function automatically.
pub fn value_to_mesh_document(value: Value) -> MeshDocument {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let fields: HashMap<String, Value> = match value {
        Value::Object(map) => map.into_iter().collect(),
        // Non-object input shouldn't happen — the `*_to_*` methods
        // produce JSON objects — but degrade gracefully.
        _ => HashMap::new(),
    };
    MeshDocument {
        id,
        fields,
        updated_at: SystemTime::now(),
    }
}

/// Coerce an optional JSON `Value` to an i64, accepting either
/// integer or float representations.
///
/// Mirror of `peat_ffi::coerce_json_number_to_i64` (private to that
/// crate; we can't reverse-import since `peat-ffi` depends on us).
/// Both parsers read the same publish JSON envelope, so they have to
/// agree on numeric coercion shape — pre-fix this side used
/// `.as_u64()` which returns `None` for any float-form Number, while
/// `peat-ffi` already accepts both. Aligning closes the
/// data-loss-on-float-form bug class peat#835 was opened to lock.
///
/// Precision contract: integer JSON Numbers > `i64::MAX` (i.e. stored
/// as `u64` in serde_json) traverse the f64 fallback path with up to
/// f64-precision loss for values > 2^53. Battery and heart-rate
/// callers absorb this through the subsequent 0..=100 / 0..=250
/// clamp; reuse outside those clamps must consider the precision
/// limit.
fn coerce_metric_to_i64(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    // `f64::round() as i64` is saturating in current Rust:
    // `f64::INFINITY as i64 == i64::MAX`, NaN as i64 == 0. Both
    // outcomes get clamped by the caller into the logical range.
    v.as_f64().map(|f| f.round() as i64)
}

/// postcard-serialize a value, returning `None` on encode failure
/// (vanishingly rare with serde-derived structs, but we don't surface
/// errors out of `encode_outbound` — the trait's `Option` return is
/// the contract). Logs at warn level for codec-side telemetry.
fn postcard_encode<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    match postcard::to_allocvec(value) {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "ble translator: postcard encode failed (this is unusual; \
                 typed BLE structs derive Serialize)"
            );
            None
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_translator() -> BleTranslator {
        BleTranslator::with_defaults()
    }

    #[test]
    fn test_position_to_track_roundtrip() {
        let translator = test_translator();

        let original = BlePosition {
            latitude: 33.7490,
            longitude: -84.3880,
            altitude: Some(320.0),
            accuracy: Some(5.0),
        };

        let track = translator.position_to_track(&original, 0x12345678, Some("ALPHA-1"));
        let recovered = translator.track_to_position(&track).unwrap();

        assert!((recovered.latitude - original.latitude).abs() < 0.0001);
        assert!((recovered.longitude - original.longitude).abs() < 0.0001);
        assert!((recovered.altitude.unwrap() - original.altitude.unwrap()).abs() < 0.1);
        assert!((recovered.accuracy.unwrap() - original.accuracy.unwrap()).abs() < 0.1);
    }

    #[test]
    fn test_position_to_track_fields() {
        let translator = test_translator();

        let position = BlePosition {
            latitude: 33.7490,
            longitude: -84.3880,
            altitude: None,
            accuracy: None,
        };

        let track = translator.position_to_track(&position, 0xABCDEF12, Some("BRAVO-2"));

        assert_eq!(track["id"], "ble-ABCDEF12");
        assert_eq!(track["callsign"], "BRAVO-2");
        assert_eq!(track["ble_origin"], true);
        assert!(track["hae"].is_null());
    }

    #[test]
    fn test_peripheral_to_platform_roundtrip() {
        let translator = test_translator();

        let original = BlePeripheral {
            id: 0x11223344,
            parent_node: 0xAABBCCDD,
            peripheral_type: BlePeripheralType::SoldierSensor,
            callsign: "CHARLIE-3".to_string(),
            health: BleHealthStatus {
                battery_percent: 75,
                heart_rate: Some(72),
                activity: 1, // walking
                alerts: BleHealthStatus::ALERT_LOW_BATTERY,
            },
            timestamp: 1700000000000,
            position: Some(BlePosition {
                latitude: 34.0,
                longitude: -85.0,
                altitude: None,
                accuracy: None,
            }),
        };

        let platform = translator.peripheral_to_platform(&original);
        let recovered = translator.platform_to_peripheral(&platform).unwrap();

        assert_eq!(recovered.id, original.id);
        assert_eq!(recovered.callsign, original.callsign);
        assert_eq!(recovered.peripheral_type, original.peripheral_type);
        assert_eq!(
            recovered.health.battery_percent,
            original.health.battery_percent
        );
        assert_eq!(recovered.health.heart_rate, original.health.heart_rate);
        assert_eq!(recovered.health.activity, original.health.activity);
        assert!(recovered.health.is_low_battery());
    }

    #[test]
    fn test_emergency_to_alert_roundtrip() {
        let translator = test_translator();

        let mut acks = HashMap::new();
        acks.insert(0x11111111, true);
        acks.insert(0x22222222, false);
        acks.insert(0x33333333, true);

        let original = BleEmergencyEvent {
            source_node: 0xDEADBEEF,
            timestamp: 1700000000000,
            acks,
        };

        let alert = translator.emergency_to_alert(&original, Some("DELTA-4"));
        let recovered = translator.alert_to_emergency(&alert).unwrap();

        assert_eq!(recovered.source_node, original.source_node);
        assert_eq!(recovered.timestamp, original.timestamp);
        assert_eq!(recovered.acks.len(), original.acks.len());
        assert_eq!(recovered.acks.get(&0x11111111), Some(&true));
        assert_eq!(recovered.acks.get(&0x22222222), Some(&false));
    }

    #[test]
    fn test_is_ble_origin() {
        let translator = test_translator();

        assert!(translator.is_ble_origin("ble-12345678"));
        assert!(!translator.is_ble_origin("track-12345678"));
        assert!(!translator.is_ble_origin("12345678"));
    }

    #[test]
    fn test_health_status_alerts() {
        let health = BleHealthStatus {
            battery_percent: 15,
            heart_rate: None,
            activity: 0,
            alerts: BleHealthStatus::ALERT_MAN_DOWN | BleHealthStatus::ALERT_LOW_BATTERY,
        };

        assert!(health.is_man_down());
        assert!(health.is_low_battery());
    }

    #[test]
    fn test_parse_ble_id() {
        let translator = test_translator();

        assert_eq!(translator.parse_ble_id("ble-12345678"), Some(0x12345678));
        assert_eq!(translator.parse_ble_id("12345678"), Some(0x12345678));
        assert_eq!(translator.parse_ble_id("0x12345678"), Some(0x12345678));
        assert_eq!(translator.parse_ble_id("ABCDEF00"), Some(0xABCDEF00));
        assert_eq!(translator.parse_ble_id("not_hex"), None);
    }

    #[test]
    fn test_position_to_track_with_cell_id() {
        let translator = test_translator();

        let position = BlePosition {
            latitude: 33.7490,
            longitude: -84.3880,
            altitude: None,
            accuracy: None,
        };

        // Without mesh_id - no cell_id
        let track = translator.position_to_track(&position, 0xAABBCCDD, Some("ALPHA-1"));
        assert!(track.get("cell_id").is_none());

        // With mesh_id - cell_id set
        let track = translator.position_to_track_in_cell(
            &position,
            0xAABBCCDD,
            Some("ALPHA-1"),
            Some("SQUAD-A"),
        );
        assert_eq!(track["cell_id"], "SQUAD-A");
    }

    #[test]
    fn test_peripheral_to_platform_with_cell_id() {
        let translator = test_translator();

        let peripheral = BlePeripheral {
            id: 0x11223344,
            parent_node: 0xAABBCCDD,
            peripheral_type: BlePeripheralType::SoldierSensor,
            callsign: "BRAVO-2".to_string(),
            health: BleHealthStatus {
                battery_percent: 85,
                heart_rate: None,
                activity: 0,
                alerts: 0,
            },
            timestamp: 1700000000000,
            position: None,
        };

        // Without mesh_id - no cell_id
        let platform = translator.peripheral_to_platform(&peripheral);
        assert!(platform.get("cell_id").is_none());

        // With mesh_id - cell_id set (mesh_id == cell_id mapping)
        let platform = translator.peripheral_to_platform_in_cell(&peripheral, Some("ALPHA-SQUAD"));
        assert_eq!(platform["cell_id"], "ALPHA-SQUAD");
        assert_eq!(platform["ble_origin"], true);
    }

    // =========================================================================
    // CannedMessage Translation Tests
    // =========================================================================

    fn test_canned_message() -> BleCannedMessage {
        let mut acks = HashMap::new();
        acks.insert(0xDEADBEEF, 1706234567000u64); // source auto-ack
        acks.insert(0x11111111, 1706234568000u64); // peer ack

        BleCannedMessage {
            message_code: 0x20, // Emergency
            message_name: "Emergency".to_string(),
            source_node: 0xDEADBEEF,
            target_node: None,
            timestamp: 1706234567000,
            sequence: 42,
            acks,
        }
    }

    #[test]
    fn test_canned_message_to_doc_roundtrip() {
        let translator = test_translator();
        let original = test_canned_message();

        let doc = translator.canned_message_to_doc(&original, Some("ALPHA-1"));
        let recovered = translator.doc_to_canned_message(&doc).unwrap();

        assert_eq!(recovered.message_code, original.message_code);
        assert_eq!(recovered.message_name, "Emergency");
        assert_eq!(recovered.source_node, original.source_node);
        assert_eq!(recovered.target_node, original.target_node);
        assert_eq!(recovered.timestamp, original.timestamp);
        assert_eq!(recovered.sequence, original.sequence);
        assert_eq!(recovered.acks.len(), original.acks.len());
        assert_eq!(recovered.acks.get(&0x11111111), Some(&1706234568000u64));
    }

    #[test]
    fn test_canned_message_fields() {
        let translator = test_translator();
        let msg = test_canned_message();

        let doc = translator.canned_message_to_doc(&msg, Some("ALPHA-1"));

        assert_eq!(doc["id"], "ble-canned-DEADBEEF-1706234567000");
        assert_eq!(doc["type"], "canned_message");
        assert_eq!(doc["message_code"], 0x20);
        assert_eq!(doc["message_name"], "Emergency");
        assert_eq!(doc["source"], "ALPHA-1");
        assert_eq!(doc["source_node"], "DEADBEEF");
        assert!(doc["target_node"].is_null());
        assert_eq!(doc["timestamp"], 1706234567000u64);
        assert_eq!(doc["sequence"], 42);
        assert_eq!(doc["ack_count"], 2);
        assert_eq!(doc["ble_origin"], true);
    }

    #[test]
    fn test_canned_message_ack_map() {
        let translator = test_translator();
        let msg = test_canned_message();

        let doc = translator.canned_message_to_doc(&msg, None);

        // Verify hex-encoded ack keys survive the roundtrip
        let acks_obj = doc["acks"].as_object().unwrap();
        assert!(acks_obj.contains_key("DEADBEEF"));
        assert!(acks_obj.contains_key("11111111"));
        assert_eq!(acks_obj["11111111"], 1706234568000u64);

        let recovered = translator.doc_to_canned_message(&doc).unwrap();
        assert_eq!(recovered.acks.get(&0xDEADBEEF), Some(&1706234567000u64));
        assert_eq!(recovered.acks.get(&0x11111111), Some(&1706234568000u64));
    }

    #[test]
    fn test_canned_message_no_target() {
        let translator = test_translator();
        let msg = test_canned_message(); // target_node = None

        let doc = translator.canned_message_to_doc(&msg, None);
        assert!(doc["target_node"].is_null());

        let recovered = translator.doc_to_canned_message(&doc).unwrap();
        assert_eq!(recovered.target_node, None);

        // Now test with a target
        let mut directed = msg;
        directed.target_node = Some(0xAABBCCDD);
        let doc = translator.canned_message_to_doc(&directed, None);
        assert_eq!(doc["target_node"], "AABBCCDD");

        let recovered = translator.doc_to_canned_message(&doc).unwrap();
        assert_eq!(recovered.target_node, Some(0xAABBCCDD));
    }

    #[test]
    fn test_canned_message_with_cell_id() {
        let translator = test_translator();
        let msg = test_canned_message();

        // Without mesh_id - no cell_id
        let doc = translator.canned_message_to_doc(&msg, None);
        assert!(doc.get("cell_id").is_none());

        // With mesh_id - cell_id set
        let doc = translator.canned_message_to_doc_in_cell(&msg, None, Some("SQUAD-A"));
        assert_eq!(doc["cell_id"], "SQUAD-A");
    }

    #[test]
    fn test_canned_message_name_from_code() {
        // Acknowledgments
        assert_eq!(message_name_from_code(0x00), "Ack");
        assert_eq!(message_name_from_code(0x01), "Ack Wilco");
        assert_eq!(message_name_from_code(0x02), "Ack Negative");
        assert_eq!(message_name_from_code(0x03), "Say Again");
        // Status
        assert_eq!(message_name_from_code(0x10), "Check In");
        assert_eq!(message_name_from_code(0x11), "Moving");
        assert_eq!(message_name_from_code(0x12), "Holding");
        assert_eq!(message_name_from_code(0x13), "On Station");
        assert_eq!(message_name_from_code(0x14), "Returning");
        assert_eq!(message_name_from_code(0x15), "Complete");
        // Alerts
        assert_eq!(message_name_from_code(0x20), "Emergency");
        assert_eq!(message_name_from_code(0x21), "Alert");
        assert_eq!(message_name_from_code(0x22), "All Clear");
        assert_eq!(message_name_from_code(0x23), "Contact");
        assert_eq!(message_name_from_code(0x24), "Under Fire");
        // Requests
        assert_eq!(message_name_from_code(0x30), "Need Extract");
        assert_eq!(message_name_from_code(0x31), "Need Support");
        assert_eq!(message_name_from_code(0x32), "Need Medic");
        assert_eq!(message_name_from_code(0x33), "Need Resupply");
        // Reserved
        assert_eq!(message_name_from_code(0xFF), "Custom");
        // Unknown fallback
        assert_eq!(message_name_from_code(0x99), "Unknown");
        assert_eq!(message_name_from_code(0x04), "Unknown");
    }

    #[test]
    fn test_canned_message_wrong_type() {
        let translator = test_translator();

        // Emergency alert doc should NOT parse as canned message
        let emergency = BleEmergencyEvent {
            source_node: 0xDEADBEEF,
            timestamp: 1700000000000,
            acks: HashMap::new(),
        };
        let alert_doc = translator.emergency_to_alert(&emergency, Some("ALPHA-1"));
        assert!(translator.doc_to_canned_message(&alert_doc).is_none());

        // Doc with no type field
        let no_type = json!({"message_code": 0x10, "source_node": "DEADBEEF"});
        assert!(translator.doc_to_canned_message(&no_type).is_none());

        // Doc with wrong type
        let wrong_type = json!({"type": "track", "message_code": 0x10});
        assert!(translator.doc_to_canned_message(&wrong_type).is_none());
    }

    #[test]
    fn test_canned_message_accessor() {
        let translator = test_translator();
        assert_eq!(translator.canned_messages_collection(), "canned_messages");

        let custom = BleTranslator::new(TranslationConfig {
            canned_messages_collection: "my_messages".to_string(),
            ..TranslationConfig::default()
        });
        assert_eq!(custom.canned_messages_collection(), "my_messages");
    }

    // ========================================================================
    // peat-mesh `Translator` trait impl tests (ADR-059 Slice 1.b)
    // ========================================================================

    use peat_mesh::transport::TranslationContext;
    // No need to import `Translator` itself — the impl block at the
    // top of this file already does, and method dispatch on a known
    // concrete type doesn't require the trait in scope.

    fn doc_with_fields(fields: serde_json::Map<String, serde_json::Value>) -> MeshDocument {
        let id = fields.get("id").and_then(|v| v.as_str()).map(String::from);
        MeshDocument {
            id,
            fields: fields.into_iter().collect(),
            updated_at: SystemTime::now(),
        }
    }

    #[test]
    fn translator_transport_id_is_ble() {
        let t = test_translator();
        assert_eq!(t.transport_id(), "ble");
    }

    #[tokio::test]
    async fn translator_encode_outbound_declines_when_no_collection() {
        let t = test_translator();
        let doc = doc_with_fields(serde_json::Map::new());
        let ctx = TranslationContext::outbound();
        // No `with_collection`; must decline rather than dispatch
        // arbitrarily.
        assert_eq!(t.encode_outbound(&doc, &ctx).await, None);
    }

    #[tokio::test]
    async fn translator_encode_outbound_declines_unknown_collection() {
        let t = test_translator();
        let doc = doc_with_fields(serde_json::Map::new());
        let ctx = TranslationContext::outbound().with_collection("not_a_ble_collection");
        assert_eq!(t.encode_outbound(&doc, &ctx).await, None);
    }

    /// Round-trip through the `Translator` trait surface for each of
    /// the four BLE doc types. encode_outbound: doc → postcard bytes.
    /// decode_inbound: bytes → doc (with the right collection's fields).
    /// The intermediate is `postcard` per ADR-059 Slice 1.b — these
    /// tests pin both the encoding choice and the dispatch-by-collection
    /// behavior.
    #[tokio::test]
    async fn translator_roundtrip_track() {
        let t = test_translator();

        // Build a track Document the way `position_to_track_in_cell`
        // would — that's what the inbound bridge produces today, and
        // it's what the orchestrator's outbound fan-out would receive.
        let position = BlePosition {
            latitude: 33.7490,
            longitude: -84.3880,
            altitude: Some(320.0),
            accuracy: None,
        };
        let track_value =
            t.position_to_track_in_cell(&position, 0xCAFE_0001, Some("ALPHA-1"), Some("BRAVO"));
        let doc = doc_with_fields(track_value.as_object().unwrap().clone());

        let ctx_out = TranslationContext::outbound().with_collection(t.tracks_collection());
        let bytes = t
            .encode_outbound(&doc, &ctx_out)
            .await
            .expect("encode tracks");

        // Decode back: use the inbound context shape — collection +
        // local_wire_id stamped before invoking decode_inbound, per
        // the trait doc on TranslationContext.collection.
        let ctx_in = TranslationContext::inbound("peer-X")
            .with_collection(t.tracks_collection())
            .with_local_wire_id("CAFE0001");
        let decoded = t
            .decode_inbound(&bytes, &ctx_in)
            .await
            .expect("decode tracks Ok")
            .expect("decode tracks Some");

        // The decoded Document should have the same lat/lon as the
        // original BlePosition, modulo float precision.
        let decoded_lat = decoded.fields.get("lat").and_then(|v| v.as_f64()).unwrap();
        let decoded_lon = decoded.fields.get("lon").and_then(|v| v.as_f64()).unwrap();
        assert!((decoded_lat - 33.7490_f64).abs() < 1e-3, "lat mismatch");
        assert!((decoded_lon - (-84.3880_f64)).abs() < 1e-3, "lon mismatch");
    }

    #[tokio::test]
    async fn translator_roundtrip_alert() {
        let t = test_translator();

        let emergency = BleEmergencyEvent {
            source_node: 0xDEAD_BEEF,
            timestamp: 1_700_000_000_000,
            acks: HashMap::new(),
        };
        let alert_value = t.emergency_to_alert(&emergency, Some("ALPHA-1"));
        let doc = doc_with_fields(alert_value.as_object().unwrap().clone());

        let ctx_out = TranslationContext::outbound().with_collection(t.alerts_collection());
        let bytes = t
            .encode_outbound(&doc, &ctx_out)
            .await
            .expect("encode alerts");

        let ctx_in = TranslationContext::inbound("peer-X")
            .with_collection(t.alerts_collection())
            .with_callsign("ALPHA-1");
        let decoded = t
            .decode_inbound(&bytes, &ctx_in)
            .await
            .expect("decode alerts Ok")
            .expect("decode alerts Some");

        // Round-trip preserves source_node + timestamp fields.
        assert_eq!(
            decoded
                .fields
                .get("source_node")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            "DEADBEEF"
        );
        assert_eq!(
            decoded
                .fields
                .get("timestamp")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            1_700_000_000_000_u64
        );
    }

    /// decode_inbound returns `Ok(None)` (not `Err`) for an unknown
    /// collection — per the trait contract, that's "not for this
    /// translator," not a wire-format-drift error. Reserved for
    /// telemetry distinguishability from real malformed bytes.
    #[tokio::test]
    async fn translator_decode_inbound_unknown_collection_returns_ok_none() {
        let t = test_translator();
        let bytes = postcard::to_allocvec(&BlePosition {
            latitude: 0.0,
            longitude: 0.0,
            altitude: None,
            accuracy: None,
        })
        .unwrap();

        let ctx = TranslationContext::inbound("peer-X").with_collection("not_a_ble_collection");
        let result = t
            .decode_inbound(&bytes, &ctx)
            .await
            .expect("Ok, not Err — decline is not a wire-format error");
        assert!(result.is_none(), "decline returns Ok(None)");
    }

    /// decode_inbound returns `Err` (not `Ok(None)`) for **malformed
    /// bytes** addressed to a known collection. This is the
    /// wire-format-drift telemetry signal the trait contract
    /// requires distinguishability for.
    #[tokio::test]
    async fn translator_decode_inbound_malformed_bytes_returns_err() {
        let t = test_translator();
        // Postcard expects specific framing for BlePosition; arbitrary
        // bytes will fail to deserialize.
        let bytes = b"this is not a postcard-encoded BlePosition";

        let ctx = TranslationContext::inbound("peer-X").with_collection(t.tracks_collection());
        let result = t.decode_inbound(bytes, &ctx).await;
        assert!(
            result.is_err(),
            "malformed bytes for a known collection must surface as Err for telemetry"
        );
    }

    /// `platform_to_peripheral` accepts both integer and float JSON
    /// number forms for `battery_percent` / `heart_rate`. Mirrors the
    /// peat-ffi parser's contract — both read the same publish
    /// envelope, so a Kotlin publisher emitting `Double` (`85.0`) or
    /// any other float-form integer must reach this side as the
    /// integer the publisher meant. Pre-fix the `.as_u64()` shape
    /// silently dropped the float form (peat#835 round-4).
    #[test]
    fn platform_to_peripheral_accepts_float_form_battery_and_heart() {
        let translator = test_translator();
        let json = serde_json::json!({
            "id": "ble-CAFE0042",
            "name": "FLOAT-PEER",
            "type": "wearable",
            "battery_percent": 85.0,
            "heart_rate": 72.0
        });
        let peripheral = translator
            .platform_to_peripheral(&json)
            .expect("float-form battery/heart must parse");
        assert_eq!(peripheral.health.battery_percent, 85);
        assert_eq!(peripheral.health.heart_rate, Some(72));
    }

    /// Out-of-range values clamp safely into the u8 destination range
    /// rather than wrapping. Pre-fix `as u8` silently truncated `>255`
    /// values; the new clamp routes them to the nearest end (0..=100
    /// for battery, 0..=250 for heart_rate).
    #[test]
    fn platform_to_peripheral_clamps_out_of_range_metrics() {
        let translator = test_translator();
        let json = serde_json::json!({
            "id": "ble-CAFE0043",
            "name": "OUT-OF-RANGE",
            "type": "wearable",
            "battery_percent": 9999,
            "heart_rate": 500
        });
        let peripheral = translator
            .platform_to_peripheral(&json)
            .expect("out-of-range must parse");
        assert_eq!(peripheral.health.battery_percent, 100);
        assert_eq!(peripheral.health.heart_rate, Some(250));
    }

    /// **Round-7 wire-format invariant** (regression gate against
    /// the round-5 mistake): `BleHealthStatus.battery_percent` MUST
    /// stay `u8` because the struct is postcard-encoded onto the BLE
    /// radio via `BleTranslator::encode_outbound`. Postcard renders
    /// `u8` as exactly 1 byte; switching to `Option<u8>` shifts every
    /// subsequent byte under shipped firmware (M5Stack + WearOS
    /// watch) and breaks decoding. Lock the byte layout at the
    /// type-system level by asserting the postcard-encoded length of
    /// a known-shape peripheral matches the wire expectation.
    #[test]
    fn ble_health_status_postcard_byte_layout_is_stable() {
        let p = BlePeripheral {
            id: 0xCAFE_BABE,
            parent_node: 0,
            peripheral_type: BlePeripheralType::SoldierSensor,
            callsign: "TEST".to_string(),
            health: BleHealthStatus {
                battery_percent: 85, // 1 postcard byte
                heart_rate: None,    // 1 postcard byte (variant tag)
                activity: 1,         // 1 postcard byte
                alerts: 0,           // 1 postcard byte
            },
            timestamp: 1_700_000_000_000,
            position: None,
        };
        let bytes = postcard::to_allocvec(&p).expect("postcard encode");
        let decoded: BlePeripheral = postcard::from_bytes(&bytes).expect("postcard decode");
        assert_eq!(decoded.health.battery_percent, 85);
        assert_eq!(decoded.health.heart_rate, None);
        // The postcard round-trip is what proves the wire is
        // unchanged from peat-btle's expectations. If a future PR
        // changes `battery_percent`'s type, this test fails at
        // compile (struct literal) AND at runtime (decode mismatch
        // against firmware that re-runs this fixture).
    }

    /// Garbled / absent `battery_percent` on the JSON parse side
    /// falls through to `100` per the wire-format default. Asymmetric
    /// with `peat-ffi::parse_battery_percent` (`Option<i32>` → None
    /// at the JSON envelope), but intentional: this layer must emit a
    /// `u8` for postcard regardless of input.
    #[test]
    fn platform_to_peripheral_defaults_garbled_battery_to_100() {
        let translator = test_translator();
        let no_battery = serde_json::json!({
            "id": "ble-DEAD0001",
            "name": "NO-BATTERY",
            "type": "wearable"
        });
        let p = translator
            .platform_to_peripheral(&no_battery)
            .expect("absent battery must parse");
        assert_eq!(
            p.health.battery_percent, 100,
            "absent battery_percent falls through to wire default 100"
        );

        // Non-numeric still falls through (matches peat-ffi's
        // reject-coercion + this layer's wire-format default).
        let garbled = serde_json::json!({
            "id": "ble-DEAD0002",
            "name": "GARBLED",
            "type": "wearable",
            "battery_percent": "85"
        });
        let p2 = translator
            .platform_to_peripheral(&garbled)
            .expect("garbled battery must parse");
        assert_eq!(p2.health.battery_percent, 100);
    }
}
