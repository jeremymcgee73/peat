//! BLE Gateway — wiring `peat_mesh::Node` + [`BleTranslator`] for cross-transport
//! correctness.
//!
//! A *gateway node* is one that runs both BLE and iroh transports and bridges
//! mesh documents between them. Without explicit wiring, peat-btle's lightweight
//! CRDTs (BLE wire format, per ADR-041) and `peat-mesh::Node`'s Automerge document
//! store are two parallel state spaces that don't see each other's updates — a
//! chat from a watch never reaches an iroh-only sim node, even with a
//! BLE+iroh-capable tablet sitting in between.
//!
//! `BleGateway` resolves that. It owns a [`Node`] and a [`BleTranslator`], and
//! exposes:
//!
//! - **Inbound** methods (`ingest_position`, `ingest_peripheral`,
//!   `ingest_emergency`, `ingest_canned_message`) that take typed peat-btle
//!   structs (decoded by the BLE transport from wire bytes), translate them to
//!   Automerge documents via [`BleTranslator`], and publish into [`Node`].
//!   From there, iroh-bound peers receive the doc through Automerge sync —
//!   automatically.
//! - **Outbound** methods (`observe_positions`, `observe_peripherals`,
//!   `observe_emergencies`, `observe_canned_messages`) that subscribe to the
//!   appropriate collection on [`Node::observe`]. Each `ChangeEvent` from the
//!   resulting stream is then translated through `change_event_to_<type>s`
//!   (which yields a `Vec`, handling `Updated` / `Initial` snapshot / `Removed`
//!   uniformly) and the documents that should propagate to BLE peers are
//!   filtered through [`BleGateway::is_outbound_candidate`] to break the
//!   `BLE → Node → observer → BLE` loop.
//!
//! `BleGateway` is *transport-shape-agnostic*: it does not own the BLE radio
//! (peat-btle does) or the iroh socket (peat-mesh's transport manager does).
//! It is the *document-layer* bridge that callers (peat-ffi, gateway nodes)
//! plug between their two transports.
//!
//! ## What's intentionally out of scope
//!
//! - **Chat translation.** Chat is the only document type peat-btle handles
//!   today that [`BleTranslator`] does not yet cover. Adding it is a separate
//!   change (extend `ble_translation.rs`, then mirror the pattern below).
//! - **Default filter on the observer surface.** `change_event_to_<type>s`
//!   applies [`BleGateway::is_outbound_candidate`] by default — docs marked
//!   `ble_origin: true` by the translator are dropped from the returned
//!   `Vec`. The gateway's outbound surface is *only* for cross-transport
//!   bridging; encoding a BLE-origin doc back to BLE forms the
//!   `BLE → Node → observer → BLE` loop and is never the right answer for
//!   that wire. Consumers that need raw, unfiltered streams (e.g. plugin
//!   UI showing every doc regardless of origin) should subscribe to
//!   [`Node::observe`] directly — that is the unfiltered shape, and the
//!   gateway should not be the path for it.
//! - **Live BLE transport binding.** Wire the inbound methods to peat-btle's
//!   `Transport` event stream, and the outbound methods to peat-btle's send
//!   path, in the call site that owns both (peat-ffi is the natural home).

use std::sync::Arc;

use peat_mesh::sync::types::{ChangeEvent, ChangeStream, Document, DocumentId, Query};
use peat_mesh::Node;
use serde_json::Value;

use crate::sync::ble_translation::{
    BleCannedMessage, BleEmergencyEvent, BlePeripheral, BlePosition, BleTranslator,
};

/// Cross-transport document bridge between peat-btle CRDTs and `peat_mesh::Node`.
///
/// Cloning is cheap — internal state is shared via `Arc`.
#[derive(Clone)]
pub struct BleGateway {
    node: Arc<Node>,
    translator: Arc<BleTranslator>,
}

impl BleGateway {
    /// Construct a gateway over the given [`Node`] and [`BleTranslator`].
    pub fn new(node: Arc<Node>, translator: BleTranslator) -> Self {
        Self {
            node,
            translator: Arc::new(translator),
        }
    }

    /// Underlying [`Node`], for callers needing direct doc-store access.
    pub fn node(&self) -> &Arc<Node> {
        &self.node
    }

    /// Underlying [`BleTranslator`], for callers needing the conversion
    /// utilities directly.
    pub fn translator(&self) -> &BleTranslator {
        &self.translator
    }

    // =========================================================================
    // Inbound: BLE-typed structs → Automerge documents → Node
    // =========================================================================

    /// Ingest a peat-btle [`BlePosition`] received from a BLE peer.
    /// Translates to a track document and publishes into [`Node`].
    pub async fn ingest_position(
        &self,
        position: &BlePosition,
        peripheral_id: u32,
        callsign: Option<&str>,
        mesh_id: Option<&str>,
    ) -> anyhow::Result<DocumentId> {
        let value =
            self.translator
                .position_to_track_in_cell(position, peripheral_id, callsign, mesh_id);
        let doc = value_to_document(value)?;
        self.node
            .publish(self.translator.tracks_collection(), doc)
            .await
    }

    /// Ingest a peat-btle [`BlePeripheral`] (peer-discovered device record).
    /// Translates to a platform document and publishes.
    pub async fn ingest_peripheral(
        &self,
        peripheral: &BlePeripheral,
        mesh_id: Option<&str>,
    ) -> anyhow::Result<DocumentId> {
        let value = self
            .translator
            .peripheral_to_platform_in_cell(peripheral, mesh_id);
        let doc = value_to_document(value)?;
        self.node
            .publish(self.translator.platforms_collection(), doc)
            .await
    }

    /// Ingest a peat-btle [`BleEmergencyEvent`].
    /// Translates to an alert document and publishes.
    pub async fn ingest_emergency(
        &self,
        emergency: &BleEmergencyEvent,
        callsign: Option<&str>,
    ) -> anyhow::Result<DocumentId> {
        let value = self.translator.emergency_to_alert(emergency, callsign);
        let doc = value_to_document(value)?;
        self.node
            .publish(self.translator.alerts_collection(), doc)
            .await
    }

    /// Ingest a peat-btle [`BleCannedMessage`].
    /// Translates to a canned-message document and publishes.
    pub async fn ingest_canned_message(
        &self,
        message: &BleCannedMessage,
        callsign: Option<&str>,
        mesh_id: Option<&str>,
    ) -> anyhow::Result<DocumentId> {
        let value = self
            .translator
            .canned_message_to_doc_in_cell(message, callsign, mesh_id);
        let doc = value_to_document(value)?;
        self.node
            .publish(self.translator.canned_messages_collection(), doc)
            .await
    }

    // =========================================================================
    // Outbound: Node observer → Automerge documents → BLE-typed structs
    // =========================================================================

    /// Observe the tracks collection. Pair with
    /// [`BleGateway::change_event_to_positions`] to translate each event's
    /// documents into typed [`BlePosition`]s ready for BLE encoding —
    /// `Updated` / `Initial` snapshot / `Removed` are all handled, and
    /// BLE-origin docs are filtered by default.
    pub fn observe_positions(&self) -> anyhow::Result<ChangeStream> {
        self.node
            .observe(self.translator.tracks_collection(), &Query::All)
    }

    /// Observe the platforms collection.
    pub fn observe_peripherals(&self) -> anyhow::Result<ChangeStream> {
        self.node
            .observe(self.translator.platforms_collection(), &Query::All)
    }

    /// Observe the alerts collection.
    pub fn observe_emergencies(&self) -> anyhow::Result<ChangeStream> {
        self.node
            .observe(self.translator.alerts_collection(), &Query::All)
    }

    /// Observe the canned-messages collection.
    pub fn observe_canned_messages(&self) -> anyhow::Result<ChangeStream> {
        self.node
            .observe(self.translator.canned_messages_collection(), &Query::All)
    }

    /// Convert a track-collection [`ChangeEvent`] to typed [`BlePosition`]s
    /// **filtered for outbound BLE transmission**.
    ///
    /// Handles all three event variants:
    /// - [`ChangeEvent::Updated`] — yields a `Vec` of 0 or 1 element (0 if
    ///   the document is BLE-origin or its fields don't translate).
    /// - [`ChangeEvent::Initial`] — yields the snapshot batch translated
    ///   per-document, with BLE-origin and untranslatable docs filtered out.
    /// - [`ChangeEvent::Removed`] — empty `Vec` (deletes carry no payload
    ///   to translate; callers wanting delete propagation must match the
    ///   variant themselves on the raw [`ChangeStream`]).
    ///
    /// The BLE-origin filter (via [`BleGateway::is_outbound_candidate`]) is
    /// applied unconditionally — this surface is the BLE outbound path, and
    /// encoding a BLE-origin doc back to BLE forms the
    /// `BLE → Node → observer → BLE` loop. Consumers wanting unfiltered
    /// streams (e.g. UI rendering of every doc regardless of origin) should
    /// subscribe to [`Node::observe`] directly.
    pub fn change_event_to_positions(&self, event: &ChangeEvent) -> Vec<BlePosition> {
        self.outbound_documents(event)
            .filter_map(|d| self.translator.track_to_position(&document_to_value(d)))
            .collect()
    }

    /// Convert a platform-collection event to typed [`BlePeripheral`]s,
    /// filtered for outbound BLE. Same semantics as
    /// [`BleGateway::change_event_to_positions`].
    pub fn change_event_to_peripherals(&self, event: &ChangeEvent) -> Vec<BlePeripheral> {
        self.outbound_documents(event)
            .filter_map(|d| {
                self.translator
                    .platform_to_peripheral(&document_to_value(d))
            })
            .collect()
    }

    /// Convert an alert-collection event to typed [`BleEmergencyEvent`]s,
    /// filtered for outbound BLE. Same semantics as
    /// [`BleGateway::change_event_to_positions`].
    pub fn change_event_to_emergencies(&self, event: &ChangeEvent) -> Vec<BleEmergencyEvent> {
        self.outbound_documents(event)
            .filter_map(|d| self.translator.alert_to_emergency(&document_to_value(d)))
            .collect()
    }

    /// Convert a canned-message-collection event to typed
    /// [`BleCannedMessage`]s, filtered for outbound BLE. Same semantics as
    /// [`BleGateway::change_event_to_positions`].
    pub fn change_event_to_canned_messages(&self, event: &ChangeEvent) -> Vec<BleCannedMessage> {
        self.outbound_documents(event)
            .filter_map(|d| self.translator.doc_to_canned_message(&document_to_value(d)))
            .collect()
    }

    /// Returns `true` if a document is a candidate for outbound BLE
    /// transmission — i.e. it does *not* carry the `ble_origin: true` marker
    /// the translator stamps on BLE-originated docs.
    ///
    /// Used internally by `change_event_to_*` to break the
    /// `BLE → Node → observer → BLE` loop. Exposed publicly because callers
    /// that hand-roll their own outbound encoder (bypassing
    /// `change_event_to_*`) still need the same check.
    ///
    /// Delegates to [`BleTranslator::has_ble_marker`] so the marker-check
    /// stays a single concept owned by the translator. If the translator's
    /// storage shape ever changes (different key, nested under metadata,
    /// etc.) this method follows automatically.
    pub fn is_outbound_candidate(&self, document: &Document) -> bool {
        !self.translator.has_ble_marker(&document_to_value(document))
    }

    /// Iterate the documents of a [`ChangeEvent`] filtered through
    /// [`Self::is_outbound_candidate`]. Internal helper used by every
    /// `change_event_to_*` method so the filter is uniform across doc types.
    fn outbound_documents<'a>(
        &'a self,
        event: &'a ChangeEvent,
    ) -> impl Iterator<Item = &'a Document> + 'a {
        documents_for_event(event)
            .iter()
            .filter(move |d| self.is_outbound_candidate(d))
    }
}

// =========================================================================
// Document <-> Value conversion helpers
// =========================================================================

/// Convert a JSON Value (translator output) into a [`Document`] suitable for
/// [`Node::publish`]. The Value's `"id"` field, if present, becomes
/// [`Document::id`]; all other top-level keys go into [`Document::fields`].
fn value_to_document(value: Value) -> anyhow::Result<Document> {
    let mut obj = match value {
        Value::Object(map) => map,
        other => {
            return Err(anyhow::anyhow!(
                "translator emitted non-object Value: {:?}",
                other
            ))
        }
    };

    // The translator emits `id` as a `String` for every doc type it produces
    // (track ids are formatted via `format!("{}{:08X}", ble_id_prefix, ...)`,
    // alert ids via `format!("{}emergency-{:08X}-{}", ...)`, etc.). Anything
    // else here is malformed translator output, not a "tolerate-and-coerce"
    // case — let it fall through to None and surface as a publish-side
    // validation issue.
    let id = obj.remove("id").and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    });

    let fields = obj.into_iter().collect();

    Ok(match id {
        Some(id) => Document::with_id(id, fields),
        None => Document::new(fields),
    })
}

/// Reconstruct the JSON Value form of a [`Document`] for translator
/// `<doc>_to_<bletype>` calls. Inverse of [`value_to_document`].
fn document_to_value(doc: &Document) -> Value {
    let mut obj = serde_json::Map::with_capacity(doc.fields.len() + 1);
    if let Some(id) = &doc.id {
        obj.insert("id".to_string(), Value::String(id.clone()));
    }
    for (k, v) in &doc.fields {
        obj.insert(k.clone(), v.clone());
    }
    Value::Object(obj)
}

/// Borrow the document slice from a [`ChangeEvent`], unifying the per-event
/// (`Updated`) and per-batch (`Initial`) shapes. `Removed` yields an empty
/// slice (no payload to translate).
fn documents_for_event(event: &ChangeEvent) -> &[Document] {
    match event {
        ChangeEvent::Updated { document, .. } => std::slice::from_ref(document),
        ChangeEvent::Initial { documents, .. } => documents.as_slice(),
        ChangeEvent::Removed { .. } => &[],
        // peat-mesh's `ChangeEvent` is `#[non_exhaustive]` (peat-mesh
        // 0.9.0-rc.3+); future variants (e.g. when Slice 2 enables
        // delete propagation) carry no document payload to translate.
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peat_mesh::sync::traits::DataSyncBackend;
    use peat_mesh::sync::InMemoryBackend;

    fn gateway() -> BleGateway {
        let backend: Arc<dyn DataSyncBackend> = Arc::new(InMemoryBackend::new_initialized());
        let node = Arc::new(Node::new(backend));
        BleGateway::new(node, BleTranslator::with_defaults())
    }

    fn position(lat: f32, lon: f32) -> BlePosition {
        BlePosition {
            latitude: lat,
            longitude: lon,
            altitude: Some(100.0),
            accuracy: Some(5.0),
        }
    }

    /// Build a "non-BLE-origin" track-shaped document — i.e. a track that
    /// looks like it came from an iroh peer, not from a local BLE ingest.
    /// Same field shape as `BleTranslator::position_to_track` but without the
    /// `ble_origin: true` marker (so it survives outbound filtering).
    fn iroh_track(lat: f64, lon: f64, callsign: &str, peripheral: u32) -> Document {
        let value = serde_json::json!({
            "id": format!("track-{:08X}", peripheral),
            "source_platform": format!("iroh-{:08X}", peripheral),
            "lat": lat,
            "lon": lon,
            "hae": 100.0,
            "cep": 5.0,
            "classification": "a-f-G-U-C",
            "confidence": 0.9,
            "category": "friendly",
            "callsign": callsign,
            "created_at": 1_700_000_000_000_i64,
            "last_update": 1_700_000_000_000_i64,
            // deliberately no `ble_origin` marker
        });
        value_to_document(value).expect("doc")
    }

    /// Inbound: a peat-btle position lands on the gateway, gets published as
    /// a track document, and an in-process observer of the tracks collection
    /// receives it. Proves the BLE → Automerge → Node path AND that the
    /// outbound surface filters this BLE-origin doc out (loop suppression).
    #[tokio::test]
    async fn ingest_position_publishes_to_tracks_observer() {
        let gw = gateway();
        let mut tracks = gw.observe_positions().expect("observe");

        gw.ingest_position(&position(40.0, -74.0), 0xCAFE0001, Some("SCOUT-CAFE"), None)
            .await
            .expect("ingest");

        let event = tracks.receiver.recv().await.expect("event");
        match &event {
            ChangeEvent::Updated {
                collection,
                document,
                ..
            } => {
                assert_eq!(collection, "tracks");
                // The translator stamps `ble_origin: true`; verify it survived
                // the Document round-trip.
                assert_eq!(
                    document.fields.get("ble_origin"),
                    Some(&Value::Bool(true)),
                    "ble_origin marker must survive Value -> Document"
                );
            }
            other => panic!("unexpected event: {:?}", other),
        }

        // Outbound for BLE: the gateway suppresses BLE-origin docs to break
        // the BLE → Node → observer → BLE loop. The raw observer above DID
        // see the event (proving the publish landed); the outbound translator
        // returns empty for the same event.
        let positions = gw.change_event_to_positions(&event);
        assert!(
            positions.is_empty(),
            "BLE-origin doc must be suppressed from outbound BLE; got {:?}",
            positions
        );
    }

    /// An Updated event for an iroh-origin doc round-trips out to a single
    /// `BlePosition` ready for BLE encoding.
    #[test]
    fn change_event_to_positions_translates_iroh_origin_updated() {
        let gw = gateway();

        let updated = ChangeEvent::Updated {
            collection: "tracks".to_string(),
            document: iroh_track(40.0, -74.0, "ALPHA-1", 0x0000_0001),
            origin: None,
        };

        let positions = gw.change_event_to_positions(&updated);
        assert_eq!(
            positions.len(),
            1,
            "iroh-origin Updated yields one position"
        );
        assert!((positions[0].latitude - 40.0).abs() < 1e-3);
        assert!((positions[0].longitude - (-74.0)).abs() < 1e-3);
    }

    /// `change_event_to_*` must surface `ChangeEvent::Initial` snapshot
    /// documents, otherwise a gateway booting into a populated mesh silently
    /// drops snapshot delivery to BLE peers (PR #802 QA review WARNING).
    /// Snapshot filtering: BLE-origin docs in the batch are suppressed;
    /// iroh-origin docs propagate.
    #[test]
    fn change_event_to_positions_handles_initial_snapshot() {
        let gw = gateway();

        // Two iroh-origin docs that should propagate, plus one BLE-origin doc
        // that should be filtered out by the outbound suppressor.
        let iroh_a = iroh_track(40.0, -74.0, "ALPHA-1", 0x0000_0001);
        let iroh_b = iroh_track(41.0, -75.0, "ALPHA-2", 0x0000_0002);
        let ble_c = value_to_document(gw.translator().position_to_track(
            &position(42.0, -76.0),
            0xCAFE_0003,
            Some("SCOUT-CAFE"),
        ))
        .expect("ble doc");

        let initial = ChangeEvent::Initial {
            collection: "tracks".to_string(),
            documents: vec![iroh_a, ble_c, iroh_b],
        };

        let positions = gw.change_event_to_positions(&initial);
        assert_eq!(
            positions.len(),
            2,
            "Initial yields the iroh-origin docs only; BLE-origin is suppressed"
        );
        assert!((positions[0].latitude - 40.0).abs() < 1e-3);
        assert!((positions[1].latitude - 41.0).abs() < 1e-3);
    }

    /// `Removed` yields an empty Vec — deletes don't carry a payload to
    /// translate. Callers wanting delete propagation must match the event
    /// variant themselves on the raw `ChangeStream`.
    #[test]
    fn change_event_to_positions_removed_yields_empty() {
        let gw = gateway();
        let removed = ChangeEvent::Removed {
            collection: "tracks".to_string(),
            doc_id: "any".to_string(),
            origin: None,
        };
        assert!(gw.change_event_to_positions(&removed).is_empty());
    }

    /// `is_outbound_candidate` filters out docs the translator marked
    /// `ble_origin: true` — the basis for breaking the
    /// `BLE → Node → observer → BLE` loop. Naïve callers wiring outbound to
    /// a BLE encoder must apply this filter (the gateway deliberately
    /// surfaces raw streams; see module doc).
    #[test]
    fn is_outbound_candidate_suppresses_ble_origin_docs() {
        let gw = gateway();

        let ble_originated = value_to_document(serde_json::json!({
            "id": "ble-CAFE0001",
            "lat": 40.0,
            "lon": -74.0,
            "ble_origin": true,
        }))
        .expect("doc");
        assert!(
            !gw.is_outbound_candidate(&ble_originated),
            "ble_origin docs must be suppressed (loop break)"
        );

        let iroh_originated = value_to_document(serde_json::json!({
            "id": "track-001",
            "lat": 40.0,
            "lon": -74.0,
        }))
        .expect("doc");
        assert!(
            gw.is_outbound_candidate(&iroh_originated),
            "non-BLE-origin docs propagate outbound"
        );

        // Explicit `ble_origin: false` also propagates (the marker is the
        // signal; absence is treated identically).
        let explicit_false = value_to_document(serde_json::json!({
            "id": "track-002",
            "ble_origin": false,
        }))
        .expect("doc");
        assert!(gw.is_outbound_candidate(&explicit_false));
    }

    /// Inbound for emergencies: ingest publishes the alert doc and the raw
    /// observer sees it. Outbound surface suppresses it as BLE-origin.
    #[tokio::test]
    async fn ingest_emergency_publishes_and_outbound_suppresses() {
        let gw = gateway();
        let mut alerts = gw.observe_emergencies().expect("observe");

        let ev = BleEmergencyEvent {
            source_node: 0xDEADBEEF,
            timestamp: 1_700_000_000_000,
            acks: Default::default(),
        };
        gw.ingest_emergency(&ev, Some("WEAROS-7347"))
            .await
            .expect("ingest");

        let event = alerts.receiver.recv().await.expect("event");
        // Raw observer saw the doc.
        match &event {
            ChangeEvent::Updated {
                collection,
                document,
                ..
            } => {
                assert_eq!(collection, "alerts");
                assert_eq!(document.fields.get("ble_origin"), Some(&Value::Bool(true)));
            }
            other => panic!("unexpected event: {:?}", other),
        }
        // Outbound surface filters the BLE-origin doc.
        assert!(gw.change_event_to_emergencies(&event).is_empty());
    }

    /// An iroh-origin alert (no `ble_origin` marker) round-trips through the
    /// outbound surface to a typed `BleEmergencyEvent`.
    #[test]
    fn change_event_to_emergencies_translates_iroh_origin_updated() {
        let gw = gateway();

        // Build an alert-shaped doc without the `ble_origin` marker, mirroring
        // the field shape `emergency_to_alert` emits but clearly iroh-origin.
        let value = serde_json::json!({
            "id": "alert-emergency-DEADBEEF-1700000000000",
            "type": "emergency",
            "source": "TANGO-1",
            "source_node": "DEADBEEF",
            "timestamp": 1_700_000_000_000_u64,
            "acks": serde_json::Map::new(),
            "ack_count": 0,
            "total_peers": 0,
            "active": true,
            // no ble_origin
        });
        let updated = ChangeEvent::Updated {
            collection: "alerts".to_string(),
            document: value_to_document(value).expect("doc"),
            origin: None,
        };

        let recovered = gw.change_event_to_emergencies(&updated);
        assert_eq!(
            recovered.len(),
            1,
            "iroh-origin Updated yields one emergency"
        );
        assert_eq!(recovered[0].source_node, 0xDEADBEEF);
        assert_eq!(recovered[0].timestamp, 1_700_000_000_000);
    }

    #[test]
    fn value_to_document_preserves_id_and_fields() {
        let value = serde_json::json!({
            "id": "ble-CAFE0001",
            "lat": 40.0,
            "lon": -74.0,
            "ble_origin": true,
        });
        let doc = value_to_document(value).expect("convert");
        assert_eq!(doc.id.as_deref(), Some("ble-CAFE0001"));
        assert_eq!(doc.fields.get("lat").and_then(Value::as_f64), Some(40.0));
        assert!(doc.fields.contains_key("ble_origin"));
        assert!(!doc.fields.contains_key("id"), "id must not double up");
    }

    #[test]
    fn document_to_value_round_trips_id_and_fields() {
        let original = serde_json::json!({
            "id": "ble-CAFE0001",
            "lat": 40.0,
            "lon": -74.0,
        });
        let doc = value_to_document(original.clone()).expect("convert");
        let back = document_to_value(&doc);
        // Field order isn't preserved by HashMap; compare as JSON object.
        assert_eq!(back["id"], original["id"]);
        assert_eq!(back["lat"], original["lat"]);
        assert_eq!(back["lon"], original["lon"]);
    }

    #[test]
    fn value_to_document_rejects_non_object() {
        let result = value_to_document(serde_json::json!([1, 2, 3]));
        assert!(result.is_err(), "non-object Value must error");
    }
}
