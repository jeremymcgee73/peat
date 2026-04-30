//! Cursor-on-Target [`Translator`] impl — the second real codec wired
//! against the peat-mesh Slice 1 trait (ADR-059 Slice 1.5,
//! "trait-stability gate").
//!
//! Named after the wire format (CoT XML) rather than the transport
//! family (TAK), because TAK is an ecosystem with multiple wire
//! formats (CoT XML, CoT protobuf, Mission Package, Data Sync); this
//! module owns the XML codec specifically. The `transport_id` stays
//! `"tak"` (matches the trait-doc convention of naming by transport
//! family), so `CotTranslator` registers as the codec for the TAK
//! transport — future TAK-side codecs (Mission Package, Data Sync)
//! could register under finer `transport_id`s like `"tak-mp"`.
//!
//! This module is deliberately a *codec-only* layer: it converts between
//! the generic [`MeshDocument`] surface that `TransportManager`'s fan-out
//! hands it and Cursor-on-Target XML bytes. It does **not** own the TAK
//! radio or the TCP/SSL/UDP socket — that responsibility stays with the
//! existing [`TakServerTransport`] / [`MeshSaTransport`] /
//! [`PeatTakBridge`] stack in this crate. ADR-059's split is "translator
//! = codec, transport = radio"; the TAK adapter pre-dated the trait and
//! conflated both, so Slice 1.5's job is to stand up the codec half
//! cleanly while the radio-half migration follows in a later slice.
//!
//! ## Build-time codec selection
//!
//! Per ADR-059's codec-placement rule, the codec lives in the same
//! crate as its transport (peat-transport, alongside `TakServerTransport`
//! / `MeshSaTransport`) but is gated behind the `mesh-translator` Cargo
//! feature so consumers that only need the radio side — without
//! peat-mesh in their dep graph — can still depend on peat-transport.
//! Same shape as the future `peat-btle` `mesh-translator` feature
//! (tracked separately) so Bitchat-style BLE-only consumers keep
//! peat-btle standalone.
//!
//! ## Trait-stability findings
//!
//! Standing up the TAK translator surfaces two friction points worth
//! capturing here so future codecs (LoRa, SBD, mavlink) don't re-tread
//! them and so the rc.5 trait-revision discussion (peat-mesh #58) has
//! a concrete starting point.
//!
//! 1. **`ctx.collection` is required for both directions, not just
//!    encode.** The trait docs already note this for inbound — bridges
//!    must populate `collection` before calling `decode_inbound` so
//!    the codec can dispatch. For TAK the inbound bridge doesn't have
//!    that information natively (a CoT event off the wire identifies
//!    itself by `cot_type`, not by collection name); the codec maps
//!    `a-f-*` → `tracks`, `b-a-o-*` → `alerts`, etc. So either (a)
//!    bridges call `decode_inbound` with `collection = None` and the
//!    codec picks based on its own type→collection table, or (b) the
//!    bridge runs a pre-decode peek and stamps the collection. We pick
//!    (a) here — the codec owns the type→collection mapping because
//!    it owns the wire format. Treat this as a clarification, not a
//!    trait change: `ctx.collection.is_none()` is a legal inbound
//!    state for codecs that derive collection from payload, and the
//!    BleTranslator's "no collection → decline" behavior is *its*
//!    policy, not a trait invariant.
//!
//! 2. **Decline-vs-error split is right but underspecified for
//!    out-of-band failures.** The trait says encode returns `None`
//!    for declines (size limits, missing fields) and that real codec
//!    errors must be logged + metric'd before that `None`. For TAK,
//!    "missing required field" (no `lat`/`lon`/`uid`) is a decline —
//!    the orchestrator can't fix it by retrying. But "CoT XML
//!    serialization failed" is a real error worth surfacing. The
//!    trait's lack of an outbound `Err` channel means the codec must
//!    log + counter internally, exactly as the docs say. Slice 1.5
//!    confirms this is workable; it's not a reason to widen the trait.
//!
//! Net: the trait survives the second codec without a shape change.
//! Slice 2 work (allowed_transports, Query::AllowedTransport, delete
//! propagation) is unblocked from a trait-stability perspective.
//!
//! [`MeshDocument`]: peat_mesh::sync::Document
//! [`TakServerTransport`]: super::server::TakServerTransport
//! [`MeshSaTransport`]: super::mesh::MeshSaTransport
//! [`PeatTakBridge`]: super::bridge::PeatTakBridge

use std::collections::HashMap;

use anyhow::Context;
use async_trait::async_trait;
use peat_mesh::sync::Document as MeshDocument;
use peat_mesh::transport::{TranslationContext, Translator};
use peat_protocol::cot::{CotEvent, CotPoint, CotType};
use serde_json::{json, Value};

/// Stable transport identifier for TAK. Compile-time literal — the
/// `Translator` trait requires `&'static str` to lock the wire-format
/// identity at the type level (see translator trait docs).
const TAK_TRANSPORT_ID: &str = "tak";

/// Configuration for [`CotTranslator`]. Extracted so tests and operators
/// can override the collection name (multi-tenant deployments rename
/// `tracks` to per-cell collection names) and the stale window without
/// hitting hardcoded strings inside the codec.
#[derive(Debug, Clone)]
pub struct CotTranslatorConfig {
    /// Collection name carrying TAK-shaped track documents.
    pub tracks_collection: String,
    /// CoT type code applied to outbound tracks that don't specify
    /// `cot_type` themselves. Default `a-f-G-U-C` (friendly atom,
    /// ground, unit, combat) — pessimistically friendly, since
    /// classification mistakes on the un-classified end of the wire
    /// are preferable to mis-flagging a friendly as hostile.
    pub default_cot_type: String,
    /// How long an emitted CoT event remains "fresh" — TAK clients use
    /// this for icon decay. 300s matches the existing `CotEncoder`
    /// default.
    pub stale_secs: i64,
}

impl Default for CotTranslatorConfig {
    fn default() -> Self {
        Self {
            tracks_collection: "tracks".to_string(),
            default_cot_type: "a-f-G-U-C".to_string(),
            stale_secs: 300,
        }
    }
}

/// Codec-only [`Translator`] impl bridging mesh `Document`s and CoT
/// XML. See module docs for trait-stability findings.
///
/// Stateless on the hot path — every encode/decode call is a pure
/// function of `(doc, ctx)` / `(bytes, ctx)`. No callsign cache, no
/// session table; the existing TAK adapter's per-connection state
/// lives one layer down in the radio transports.
#[derive(Debug, Default, Clone)]
pub struct CotTranslator {
    config: CotTranslatorConfig,
}

impl CotTranslator {
    /// Construct a translator with default config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct with caller-supplied config (collection rename, custom
    /// default cot_type, etc.).
    pub fn with_config(config: CotTranslatorConfig) -> Self {
        Self { config }
    }

    /// Tracks-collection accessor — mirrors `BleTranslator::tracks_collection`
    /// so the orchestrator can source the collection list from each
    /// translator without hardcoding strings.
    pub fn tracks_collection(&self) -> &str {
        &self.config.tracks_collection
    }
}

#[async_trait]
impl Translator for CotTranslator {
    fn transport_id(&self) -> &'static str {
        TAK_TRANSPORT_ID
    }

    async fn encode_outbound(
        &self,
        doc: &MeshDocument,
        ctx: &TranslationContext,
    ) -> Option<Vec<u8>> {
        // Decline: no collection (orchestrator didn't tell us what
        // this doc is) or a collection this codec doesn't carry. Per
        // ADR-059 declines are normal traffic; the orchestrator drops
        // them silently.
        let collection = ctx.collection.as_deref()?;
        if collection != self.tracks_collection() {
            return None;
        }

        // Required fields. Missing any of these means the document
        // can't be expressed as a CoT track — decline rather than
        // emit a malformed event.
        let uid = doc.id.as_deref()?;
        let lat = doc.fields.get("lat").and_then(Value::as_f64)?;
        let lon = doc.fields.get("lon").and_then(Value::as_f64)?;

        let callsign = doc
            .fields
            .get("callsign")
            .and_then(Value::as_str)
            .or(ctx.local_callsign.as_deref());

        // Documents may pin their CoT classification (e.g. red-track
        // scenarios pre-tag as `a-h-*`); fall back to the configured
        // default for everything else.
        let cot_type_str = doc
            .fields
            .get("cot_type")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.default_cot_type);

        // Build the CoT point once. `CotPoint::new(lat, lon)` defaults
        // hae=0; the explicit struct form is only needed when the doc
        // supplies a non-zero hae. Setting `.point(...)` on the builder
        // twice (once with default hae, then with the real hae) would be
        // a load-bearing reliance on builder replace-semantics — pick
        // the right value first instead.
        let hae = doc.fields.get("hae").and_then(Value::as_f64).unwrap_or(0.0);
        let point = if hae != 0.0 {
            CotPoint {
                lat,
                lon,
                hae,
                ce: 9_999_999.0,
                le: 9_999_999.0,
            }
        } else {
            CotPoint::new(lat, lon)
        };

        let mut builder = CotEvent::builder()
            .uid(uid)
            .cot_type(CotType::new(cot_type_str))
            .stale_duration(chrono::Duration::seconds(self.config.stale_secs))
            .point(point);

        if let Some(cs) = callsign {
            builder = builder.callsign(cs);
        }

        let event = match builder.build() {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, uid = %uid, "tak: CoT builder failed");
                return None;
            }
        };

        match event.to_xml() {
            Ok(xml) => Some(xml.into_bytes()),
            Err(e) => {
                // Real codec error per trait docs — log + (TODO) metric.
                // Returning `None` mirrors the trait contract: the
                // orchestrator wouldn't know what to do with an Err
                // from outbound that the codec couldn't already do.
                tracing::warn!(error = %e, uid = %uid, "tak: CoT XML serialization failed");
                None
            }
        }
    }

    async fn decode_inbound(
        &self,
        bytes: &[u8],
        _ctx: &TranslationContext,
    ) -> anyhow::Result<Option<MeshDocument>> {
        // Wire-format-drift diagnostics use Err — the trait reserves
        // Ok(None) for "well-formed but not for me" and Err for
        // "malformed/corrupt input".
        let xml = std::str::from_utf8(bytes).context("tak: bytes not valid UTF-8")?;
        let event = CotEvent::from_xml(xml).context("tak: parse CoT XML")?;

        // Codec owns the cot_type → collection mapping. Today only
        // friendly/hostile/unknown atoms route to `tracks`; control
        // frames (`t-*`) and drawings (`u-d-*`) are well-formed but
        // not for this collection — return Ok(None).
        if !event.cot_type.is_atom() {
            return Ok(None);
        }

        let mut fields = HashMap::new();
        fields.insert("lat".into(), json!(event.point.lat));
        fields.insert("lon".into(), json!(event.point.lon));
        if event.point.hae != 0.0 {
            fields.insert("hae".into(), json!(event.point.hae));
        }
        fields.insert(
            "cot_type".into(),
            Value::String(event.cot_type.as_str().to_string()),
        );
        if let Some(cs) = &event.detail.contact_callsign {
            fields.insert("callsign".into(), Value::String(cs.clone()));
        }
        fields.insert("timestamp_ms".into(), json!(event.time.timestamp_millis()));
        // Origin marker mirrors the BLE side's `ble_origin` so listeners
        // can attribute cross-transport traffic without re-decoding the
        // wire bytes. The peat-sim track_listener already prints a
        // similar marker for `ble_origin`; equivalent symmetry here.
        fields.insert("tak_origin".into(), Value::Bool(true));

        Ok(Some(MeshDocument::with_id(event.uid, fields)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_doc() -> MeshDocument {
        let mut fields = HashMap::new();
        fields.insert("lat".into(), json!(34.123));
        fields.insert("lon".into(), json!(-118.456));
        fields.insert("callsign".into(), json!("ALPHA-1"));
        MeshDocument::with_id("track-uid-1".to_string(), fields)
    }

    #[test]
    fn transport_id_is_tak_static() {
        let t = CotTranslator::new();
        // Lock the &'static str shape — see translator trait docs.
        let id: &'static str = t.transport_id();
        assert_eq!(id, "tak");
    }

    #[tokio::test]
    async fn encode_declines_when_no_collection() {
        let t = CotTranslator::new();
        let doc = track_doc();
        let ctx = TranslationContext::outbound();
        assert_eq!(t.encode_outbound(&doc, &ctx).await, None);
    }

    #[tokio::test]
    async fn encode_declines_unknown_collection() {
        let t = CotTranslator::new();
        let doc = track_doc();
        let ctx = TranslationContext::outbound().with_collection("alerts");
        assert_eq!(t.encode_outbound(&doc, &ctx).await, None);
    }

    #[tokio::test]
    async fn encode_declines_when_required_fields_missing() {
        let t = CotTranslator::new();
        let mut fields = HashMap::new();
        fields.insert("callsign".into(), json!("ALPHA-1"));
        let doc = MeshDocument::with_id("track-uid-2".to_string(), fields);
        let ctx = TranslationContext::outbound().with_collection("tracks");
        assert_eq!(
            t.encode_outbound(&doc, &ctx).await,
            None,
            "missing lat/lon must decline, not panic"
        );
    }

    #[tokio::test]
    async fn encode_emits_cot_xml_with_uid_and_callsign() {
        let t = CotTranslator::new();
        let doc = track_doc();
        let ctx = TranslationContext::outbound().with_collection("tracks");

        let bytes = t
            .encode_outbound(&doc, &ctx)
            .await
            .expect("track must encode");
        let xml = std::str::from_utf8(&bytes).expect("xml is utf-8");
        assert!(
            xml.contains("uid=\"track-uid-1\""),
            "xml missing uid: {xml}"
        );
        assert!(
            xml.contains("type=\"a-f-G-U-C\""),
            "xml missing default cot_type: {xml}"
        );
        assert!(xml.contains("ALPHA-1"), "xml missing callsign: {xml}");
    }

    #[tokio::test]
    async fn encode_uses_ctx_local_callsign_when_doc_has_none() {
        // Trait-level fallback: documents without a `callsign` field
        // pick up the local node's callsign from `ctx.local_callsign`.
        // Covers the doc-vs-ctx resolution branch that production
        // codepaths exercise (orchestrator stamps local_callsign per
        // fan-out cycle for nodes that don't denormalize callsign onto
        // every doc).
        let t = CotTranslator::new();
        let mut fields = HashMap::new();
        fields.insert("lat".into(), json!(0.0));
        fields.insert("lon".into(), json!(0.0));
        let doc = MeshDocument::with_id("no-callsign-doc".to_string(), fields);
        let ctx = TranslationContext::outbound()
            .with_collection("tracks")
            .with_callsign("CTX-FALLBACK");

        let bytes = t
            .encode_outbound(&doc, &ctx)
            .await
            .expect("doc without callsign + ctx.local_callsign must encode");
        let xml = std::str::from_utf8(&bytes).unwrap();
        assert!(
            xml.contains("CTX-FALLBACK"),
            "ctx.local_callsign must be used when doc has no callsign field: {xml}"
        );
    }

    #[tokio::test]
    async fn encode_doc_callsign_wins_over_ctx_callsign() {
        // Doc-level callsign takes precedence — the ctx fallback is
        // only consulted when the doc is silent. This is the contract
        // operators expect: per-message overrides beat per-node
        // defaults.
        let t = CotTranslator::new();
        let mut fields = HashMap::new();
        fields.insert("lat".into(), json!(0.0));
        fields.insert("lon".into(), json!(0.0));
        fields.insert("callsign".into(), json!("DOC-WINS"));
        let doc = MeshDocument::with_id("doc-callsign-1".to_string(), fields);
        let ctx = TranslationContext::outbound()
            .with_collection("tracks")
            .with_callsign("CTX-LOSES");

        let bytes = t.encode_outbound(&doc, &ctx).await.unwrap();
        let xml = std::str::from_utf8(&bytes).unwrap();
        assert!(
            xml.contains("DOC-WINS"),
            "doc-supplied callsign missing: {xml}"
        );
        assert!(
            !xml.contains("CTX-LOSES"),
            "ctx fallback must not appear when doc supplies callsign: {xml}"
        );
    }

    #[tokio::test]
    async fn encode_honors_doc_supplied_cot_type() {
        let t = CotTranslator::new();
        let mut fields = HashMap::new();
        fields.insert("lat".into(), json!(0.0));
        fields.insert("lon".into(), json!(0.0));
        fields.insert("cot_type".into(), json!("a-h-G")); // hostile
        let doc = MeshDocument::with_id("hostile-1".to_string(), fields);
        let ctx = TranslationContext::outbound().with_collection("tracks");

        let bytes = t
            .encode_outbound(&doc, &ctx)
            .await
            .expect("hostile track must encode");
        let xml = std::str::from_utf8(&bytes).unwrap();
        assert!(
            xml.contains("type=\"a-h-G\""),
            "doc-supplied cot_type must override default: {xml}"
        );
    }

    #[tokio::test]
    async fn decode_produces_document_with_origin_marker() {
        let t = CotTranslator::new();
        let doc = track_doc();
        let ctx_out = TranslationContext::outbound().with_collection("tracks");
        let bytes = t.encode_outbound(&doc, &ctx_out).await.unwrap();

        let ctx_in = TranslationContext::inbound("tak-server-1");
        let decoded = t
            .decode_inbound(&bytes, &ctx_in)
            .await
            .expect("decode must not error")
            .expect("atom CoT must produce a doc");

        assert_eq!(decoded.id.as_deref(), Some("track-uid-1"));
        assert_eq!(
            decoded.fields.get("tak_origin").and_then(Value::as_bool),
            Some(true),
            "tak_origin marker required so listeners can attribute"
        );
    }

    #[tokio::test]
    async fn roundtrip_preserves_uid_lat_lon_callsign_cot_type() {
        let t = CotTranslator::new();
        let doc = track_doc();
        let ctx_out = TranslationContext::outbound().with_collection("tracks");

        let bytes = t.encode_outbound(&doc, &ctx_out).await.unwrap();
        let ctx_in = TranslationContext::inbound("tak-server-1");
        let decoded = t.decode_inbound(&bytes, &ctx_in).await.unwrap().unwrap();

        assert_eq!(decoded.id.as_deref(), Some("track-uid-1"));
        assert_eq!(
            decoded.fields.get("lat").and_then(Value::as_f64),
            Some(34.123)
        );
        assert_eq!(
            decoded.fields.get("lon").and_then(Value::as_f64),
            Some(-118.456)
        );
        assert_eq!(
            decoded.fields.get("callsign").and_then(Value::as_str),
            Some("ALPHA-1")
        );
        assert_eq!(
            decoded.fields.get("cot_type").and_then(Value::as_str),
            Some("a-f-G-U-C")
        );
    }

    #[tokio::test]
    async fn decode_returns_ok_none_for_non_atom_cot_type() {
        // A drawing/shape (`u-d-r` = drawing rectangle) is well-formed
        // CoT but not a track — Ok(None) is the trait-correct answer.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<event version="2.0" uid="drawing-1" type="u-d-r" time="2026-01-01T00:00:00Z" start="2026-01-01T00:00:00Z" stale="2026-01-01T00:05:00Z" how="m-g">
  <point lat="0.0" lon="0.0" hae="0.0" ce="9999999.0" le="9999999.0"/>
  <detail/>
</event>"#;
        let t = CotTranslator::new();
        let ctx = TranslationContext::inbound("peer");
        let result = t.decode_inbound(xml.as_bytes(), &ctx).await.unwrap();
        assert!(
            result.is_none(),
            "drawing CoT must Ok(None), not Err — non-atom is well-formed but not a track"
        );
    }

    #[tokio::test]
    async fn decode_errors_on_malformed_xml() {
        let t = CotTranslator::new();
        let ctx = TranslationContext::inbound("peer");
        let result = t.decode_inbound(b"<not-cot/>garbage", &ctx).await;
        assert!(
            result.is_err(),
            "malformed CoT must Err — wire-format drift diagnostic per trait docs"
        );
    }

    #[tokio::test]
    async fn decode_errors_on_non_utf8_bytes() {
        let t = CotTranslator::new();
        let ctx = TranslationContext::inbound("peer");
        let result = t.decode_inbound(&[0xff, 0xfe, 0xfd], &ctx).await;
        assert!(result.is_err(), "non-UTF-8 bytes must Err");
    }
}
