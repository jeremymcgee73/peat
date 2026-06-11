# ADR-070: SAPIENT Protocol Bridge

**Status**: Accepted
**Date**: 2026-06-10
**Authors**: Kit Plummer
**Organization**: Defense Unicorns (https://defenseunicorns.com)
**Relates To**: ADR-029 (TAK Transport Adapter), ADR-038 (Protocol-Level Format Transformation Primitives), ADR-058 (MAVLink Protocol Bridge), ADR-012 (Schema Definition & Protocol Extensibility)

---

## Executive Summary

This ADR defines the architecture for `peat-sapient`, a Rust **library crate** providing bidirectional integration between the UK Dstl SAPIENT (Sensor And Platform Integration Extended from NATO Technology) protocol and the Peat mesh ecosystem. Like `peat-mavlink`, `peat-sapient` is a **protocol bridge** — it translates between SAPIENT's sensor/autonomous-platform message vocabulary and Peat document types defined in `peat-schema`. The crate has **no dependency on `peat-mesh`** and follows the external-crate composition pattern established by ADR-058.

---

## Context

### What Is SAPIENT?

SAPIENT is a UK Ministry of Defence open standard, developed and maintained by Dstl (Defence Science and Technology Laboratory), for integrating heterogeneous sensors and autonomous platforms into C2 systems. It is published under a permissive open licence and is actively progressing through NATO standardisation.

The current release is **SAPIENT v8**, which uses Protocol Buffers (proto3) as its wire format. Earlier versions (v6, v7) used JSON/XML; v8's move to protobuf makes integration with Peat's schema layer natural.

SAPIENT defines two logical roles:

| Role | SAPIENT Term | Description |
|------|-------------|-------------|
| Manager | HLDMM (High-Level Decision Making Module) | Sends tasks; receives detections, status, alerts |
| Sensor / Autonomous Platform | DLMM (Detection-Level Multi-sensor Management Module) | Registers, reports status, emits detections, accepts tasks |

The wire topology is hub-spoke: an ASM (Autonomous System Manager) broker sits between HLDMMs and DLMMs. Peat's mesh is a natural peer layer for the HLDMM role — it can act as a distributed manager that federates tasks and aggregates detections across multiple ASMs.

### SAPIENT Message Vocabulary (v8)

| Message | Direction | Purpose |
|---------|-----------|---------|
| `RegisterNode` | DLMM → ASM | Sensor registers; announces identity and node type |
| `NodeDescription` | DLMM → ASM | Sensor describes its capabilities, FOV, supported task types |
| `StatusReport` | DLMM → ASM | Periodic heartbeat with operational status and health |
| `DetectionReport` | DLMM → ASM | Sensor detection event (location, class, confidence) |
| `Task` | ASM → DLMM | Tasking command (scan, track, follow, alert-on, idle) |
| `TaskAck` | DLMM → ASM | Task acknowledgment with acceptance/rejection reason |
| `Alert` | DLMM → ASM | Out-of-band alert (e.g. intrusion, fault) |
| `Error` | either | Error notification |

### Why Peat Needs SAPIENT Integration

1. **UK and NATO programmes of record.** SAPIENT-compliant sensors and autonomous platforms are deployed across UK MoD and are entering NATO operational use. Peat interoperates or is irrelevant.

2. **Sensor data belongs on the mesh.** SAPIENT `DetectionReport` data — position, classification, confidence — is exactly the track and situational awareness data that Peat was built to propagate and aggregate across a distributed mesh of consumers. Leaving it inside a single ASM's hub-spoke topology wastes the mesh.

3. **Peat can act as a distributed HLDMM.** Any Peat node with the `peat-sapient` bridge can issue tasks to SAPIENT DLMMs and relay responses back across the mesh. This enables multi-operator, multi-domain tasking with CRDT-backed conflict resolution.

4. **Structural alignment with peat-schema.** SAPIENT v8 uses protobuf; peat-schema uses protobuf. The type mappings are non-trivial but largely mechanical, not architectural. A bridge crate is the right layer to own them.

5. **Follows the protocol-bridge pattern.** ADR-038 established format adapters as a first-class concept. ADR-058 instantiated that pattern for MAVLink. SAPIENT is the same pattern applied to the UK/NATO sensor integration domain.

### Why Not a Transport Adapter?

SAPIENT messages are semantically typed (detections, tasks, alerts) — they are not opaque bytes to be carried over a physical link. The same argument that ruled out MAVLink-as-transport (ADR-058) applies here: tunnelling Peat sync bytes through SAPIENT `Error` or custom extensions would be fragile and would miss the point. The value is **translating** SAPIENT's domain model into Peat's CRDT-backed mesh.

### Relationship to ADR-029 (TAK Transport Adapter)

`peat-tak-bridge` translates CoT/TAK events ↔ Peat CRDTs. `peat-sapient` does the same thing for SAPIENT messages. The data flow analogy:

```
                    ┌─────────────────────────┐
SAPIENT DLMM ──────►│  peat-sapient bridge    ├──────► peat-mesh (CRDT sync)
(sensor/platform)   │  (DetectionReport,      │        (Track, NodeHealth,
                    │   StatusReport, Alert)  │         Capability, Alert)
SAPIENT ASM ───────►│                         │
(broker, optional)  └─────────────────────────┘
```

---

## Decision

### License

`peat-sapient` is released under the **Apache License 2.0**. The vendored Dstl SAPIENT proto files (`dstl/SAPIENT-Proto-Files`) are also Apache 2.0 — no compatibility issue.

### Crate: `peat-sapient` (External Library Crate)

`peat-sapient` is a standalone Rust library crate, external to the `peat` workspace, following the pattern of `peat-mavlink`, `peat-btle`, and `peat-lora`. It depends on `peat-schema` for Peat document types but has **no dependency on `peat-mesh`**.

The integrator's mission application composes both crates:

```
mission-app (Cargo.toml)
├── peat-sapient = "0.1"   # SAPIENT parsing + Peat document mapping
├── peat-mesh    = "..."   # Mesh participation, CRDT storage
└── (mission-specific deps)
```

### Supported SAPIENT Versions

| Version | Support level |
|---------|--------------|
| v8 (protobuf) | Full — primary target |
| v7 (JSON) | Optional via `feature = "v7"` |
| v6 (XML/JSON) | Out of scope for v0.1 |

v8 is the forward-looking standard; v7 support is included behind a feature flag to ease migration for deployments that have not yet upgraded their DLMMs.

### Message Mapping (SAPIENT ↔ Peat)

#### Inbound: SAPIENT DLMM → Peat

| SAPIENT Message | Peat Document Type | Notes |
|----------------|-------------------|-------|
| `RegisterNode` | `Capability` (registration) | Node identity, type, initial capability advertisement |
| `NodeDescription` | `Capability` (updated) | FOV, sensor modes, supported task types → capability fields |
| `StatusReport` | `NodeHealth` + `Capability` | Operational status → health; mode/region changes → capability |
| `DetectionReport` | `Track` | Location, classification, confidence, velocity → track fields |
| `Alert` | `Alert` | Alert type, location, severity → Peat alert |
| `TaskAck` | command acknowledgment | Acceptance/rejection + reason stored against the originating task |
| `Error` | `NodeHealth` (error state) | Error code and message recorded as health event |

#### Outbound: Peat → SAPIENT HLDMM / ASM

| Peat Document Type | SAPIENT Message | Notes |
|-------------------|----------------|-------|
| Tasking / command | `Task` | Task type, region, target node ID → SAPIENT Task |
| Task cancellation | `Task` (with IDLE / CANCEL) | Cancel represented as idle or explicit cancel task |

### Transport Connectivity

SAPIENT v8 operates over TCP (point-to-point between DLMM and ASM). `peat-sapient` connects as either a **DLMM** (sensor-side, reporting into a Peat-backed HLDMM) or an **HLDMM** (manager-side, issuing tasks to external DLMMs and relaying their data onto the mesh).

| Role | Connection model | Use case |
|------|-----------------|----------|
| HLDMM (manager) | TCP server; DLMMs connect to it | Peat mesh acts as the distributed manager |
| DLMM (sensor/platform) | TCP client; connects to an ASM or another HLDMM | Peat relays sensor data from an existing SAPIENT deployment |
| Pass-through relay | TCP both sides | Peat sits between an ASM and a DLMM, observing and injecting |

---

## Architecture

### Crate Structure

```
peat-sapient/
├── Cargo.toml
├── build.rs                        # Compile SAPIENT v8 proto definitions
├── proto/
│   └── sapient_v8/                 # SAPIENT v8 proto files (Dstl public release)
│       ├── sapient.proto
│       ├── types.proto
│       └── ...
├── src/
│   ├── lib.rs                      # Public API, re-exports, feature gates
│   ├── bridge.rs                   # Core bridge: SAPIENT ↔ Peat translation
│   ├── config.rs                   # Bridge configuration
│   ├── connection.rs               # TCP connection management (HLDMM / DLMM roles)
│   ├── node_registry.rs            # Tracks registered SAPIENT nodes and their capabilities
│   ├── mapping/
│   │   ├── detection.rs            # DetectionReport → Track
│   │   ├── status.rs               # StatusReport → NodeHealth + Capability
│   │   ├── registration.rs         # RegisterNode + NodeDescription → Capability
│   │   ├── alert.rs                # Alert → peat Alert
│   │   └── task.rs                 # Peat command → Task; TaskAck → command ack
│   └── error.rs                    # Error types
├── examples/
│   ├── hldmm_mission_app.rs        # Peat as HLDMM: task sensors, relay detections to mesh
│   ├── dlmm_relay.rs               # Peat bridging an existing ASM into the mesh
│   └── multi_sensor.rs             # Multiple SAPIENT sensors feeding one Peat node
└── tests/
    ├── mapping_tests.rs
    ├── connection_tests.rs
    └── integration_tests.rs
```

### Core Types

```rust
/// Bridge configuration
#[derive(Debug, Clone)]
pub struct SapientBridgeConfig {
    /// Whether this bridge acts as HLDMM (manager) or DLMM (sensor-side relay)
    pub role: BridgeRole,
    /// TCP endpoints
    pub connections: Vec<SapientConnectionConfig>,
    /// SAPIENT protocol version to speak
    pub protocol_version: SapientVersion,
    /// Rate-limit detection relay to the mesh (detections per second; None = unlimited)
    pub detection_rate_limit: Option<f64>,
    /// Which SAPIENT message types to bridge (None = all)
    pub message_filter: Option<Vec<SapientMessageType>>,
}

#[derive(Debug, Clone)]
pub enum BridgeRole {
    /// This bridge accepts DLMM connections and acts as the HLDMM
    Hldmm {
        /// TCP address to listen on
        listen_addr: std::net::SocketAddr,
    },
    /// This bridge connects to an existing ASM or HLDMM and relays into Peat
    Dlmm {
        /// TCP address of the ASM / HLDMM
        remote_addr: std::net::SocketAddr,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SapientVersion {
    V8,
    #[cfg(feature = "v7")]
    V7,
}

/// Update emitted to the integrator's mission application
#[derive(Debug, Clone)]
pub enum SapientUpdate {
    /// A new or updated SAPIENT node registered / described itself
    NodeRegistered {
        node_id: Uuid,
        capability: peat_schema::Capability,
    },
    /// Periodic status update from a SAPIENT node
    StatusReceived {
        node_id: Uuid,
        health: peat_schema::NodeHealth,
    },
    /// A detection from a SAPIENT sensor
    DetectionReceived {
        node_id: Uuid,
        detection_id: Uuid,
        track: peat_schema::Track,
    },
    /// An alert from a SAPIENT node
    AlertReceived {
        node_id: Uuid,
        alert: peat_schema::Alert,
    },
    /// A task acknowledgment received from a SAPIENT node
    TaskAcknowledged {
        node_id: Uuid,
        task_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    },
    /// A SAPIENT node disconnected
    NodeDisconnected {
        node_id: Uuid,
    },
}

/// Bridge error types
#[derive(Debug, thiserror::Error)]
pub enum SapientError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Protobuf decode error: {0}")]
    DecodeError(#[from] prost::DecodeError),
    #[error("Message mapping error: {kind} — {detail}")]
    MappingError { kind: &'static str, detail: String },
    #[error("Node {0} not registered")]
    NodeNotFound(Uuid),
    #[error("Task rejected by node {node_id}: {reason}")]
    TaskRejected { node_id: Uuid, reason: String },
    #[error("Unsupported SAPIENT version: {0:?}")]
    UnsupportedVersion(SapientVersion),
}
```

### Bridge API

```rust
pub struct SapientBridge {
    config: SapientBridgeConfig,
    node_registry: Arc<RwLock<NodeRegistry>>,
    update_tx: tokio::sync::broadcast::Sender<SapientUpdate>,
}

impl SapientBridge {
    /// Create a new bridge. Returns the bridge and a receiver for updates.
    /// The integrator consumes the receiver and writes updates into peat-mesh.
    pub async fn new(
        config: SapientBridgeConfig,
    ) -> Result<(Self, tokio::sync::broadcast::Receiver<SapientUpdate>), SapientError>;

    /// Start the bridge (spawns TCP listener / connector tasks)
    pub async fn start(&mut self) -> Result<(), SapientError>;

    /// Stop the bridge gracefully
    pub async fn stop(&mut self) -> Result<(), SapientError>;

    /// Send a task to a registered SAPIENT node (HLDMM role only)
    pub async fn send_task(
        &self,
        node_id: Uuid,
        task: SapientTask,
    ) -> Result<(), SapientError>;

    /// List all currently registered SAPIENT nodes
    pub fn registered_nodes(&self) -> Vec<RegisteredNode>;

    /// Get the last-known capability snapshot for a node
    pub fn node_capability(&self, node_id: Uuid) -> Option<peat_schema::Capability>;
}
```

### Message Flow

**Inbound (SAPIENT → Peat)**:

```
SAPIENT DLMM / ASM (TCP)
    │
    ▼
SapientConnection::recv()          ← Framed protobuf message
    │
    ▼
prost::Message::decode()           ← sapient_v8::SapientMessage
    │
    ▼
Bridge::handle_message()           ← Route by oneof variant
    │
    ├── RegisterNode        → mapping::registration::to_capability()
    ├── NodeDescription     → mapping::registration::to_capability_update()
    ├── StatusReport        → mapping::status::to_node_health()
    ├── DetectionReport     → mapping::detection::to_track()
    ├── Alert               → mapping::alert::to_peat_alert()
    ├── TaskAck             → SapientUpdate::TaskAcknowledged
    └── Error               → mapping::status::to_error_health()
    │
    ▼
update_tx.send(SapientUpdate)      ← Mission app receives via broadcast channel
```

**Outbound (Peat → SAPIENT)**:

```
Mission app calls bridge.send_task()
    │
    ▼
mapping::task::from_peat_command()  ← peat tasking → sapient_v8::Task
    │
    ▼
SapientConnection::send()           ← Framed protobuf to DLMM
    │
    ▼
Wait for TaskAck                     ← Async, with configurable timeout
```

### Detection Report → Track Mapping

SAPIENT `DetectionReport` is the richest message type. The mapping to `peat_schema::Track` preserves all fields that have a canonical Peat equivalent and stores SAPIENT-specific extensions in the track's opaque `extension` field.

| SAPIENT `DetectionReport` field | Peat `Track` field | Notes |
|--------------------------------|-------------------|-------|
| `detection_id` | `track_id` | UUID direct map |
| `node_id` | `source_node_id` | Which sensor produced this |
| `timestamp` | `timestamp` | |
| `location.x` / `y` / `z` | `position.{lat,lon,altitude}` | Coordinate conversion if SAPIENT uses local frame |
| `location.coordinate_system` | (informs conversion) | WGS84 → direct; local frame → transform |
| `object_report.object_class` | `classification` | SAPIENT class vocabulary → Peat classification |
| `object_report.confidence` | `confidence` | 0.0–1.0 direct |
| `object_report.colour` | (opaque extension) | |
| `track_object_info.track_id` | `track_id` (if present) | SAPIENT may supply a persistent track ID |
| `track_object_info.velocity` | `velocity` | Speed + heading |
| `range_bearing` | `position` (if primary) | Bearing/range converted to lat/lon when sensor position known |

Detection fields with no Peat equivalent (signal characteristics, detailed phenotype data) are preserved verbatim as a serialised JSON blob in `Track.extension["sapient_v8"]`.

### Status Report → NodeHealth + Capability Mapping

| SAPIENT `StatusReport` field | Peat type | Target field |
|------------------------------|-----------|-------------|
| `node_id` | `NodeHealth` | `node_id` |
| `timestamp` | `NodeHealth` | `timestamp` |
| `system` (OK / WARNING / ERROR / OFFLINE) | `NodeHealth` | `status` |
| `info` (free text) | `NodeHealth` | `status_message` |
| `mode` (ACTIVE / STANDBY / etc.) | `Capability` | `operational_mode` |
| `field_of_view` | `Capability` | `field_of_view` |
| `coverage.region` | `Capability` | `coverage_region` |
| `power.source` / `power.battery` | `NodeHealth` | `power_source` / `battery_percent` |

### RegisterNode + NodeDescription → Capability Mapping

| SAPIENT field | Peat `Capability` field | Notes |
|--------------|------------------------|-------|
| `node_id` | `node_id` | |
| `node_type` (CAMERA, RADAR, ACOUSTIC, UGV, UAV, …) | `node_type` | SAPIENT node type vocabulary → Peat `node_type` enum |
| `node_sub_type` | `node_subtype` | Vendor / model string |
| `node_description.capabilities` | `capabilities[]` | List of supported task types |
| `node_description.field_of_view` | `field_of_view` | |
| `node_description.coverage_regions` | `coverage_regions[]` | |
| `node_definition.region_definition` | `region_definition` | Configurable detection regions |

### Coordinate System Handling

`peat-sapient` normalises all positions to WGS84 (the Peat and CoT convention) at the bridge boundary. BSI Flex 335 v2.0 defines the following `LocationCoordinateSystem` and `RangeBearingCoordinateSystem` variants:

**`LocationCoordinateSystem` (BSI Flex 335 v2.0)**

| Proto value | Name | Bridge action |
|-------------|------|---------------|
| 1 | `LatLngDegM` | WGS84 lat/lon degrees, altitude metres — pass through |
| 2 | `LatLngRadM` | WGS84 lat/lon radians, altitude metres — angles converted to degrees |
| 3 | *(deprecated SAPIENT v7: degrees/feet)* | Angles pass through; altitude converted to metres (× 0.3048) |
| 4 | *(deprecated SAPIENT v7: radians/feet)* | Angles converted to degrees; altitude converted to metres |
| 5 | `UtmM` | UTM metres — Snyder series inverse Transverse Mercator projection to WGS84 |

MGRS is **not** a `LocationCoordinateSystem` variant in BSI Flex 335 v2.0. It was anticipated in an earlier draft of this ADR based on SAPIENT v7 semantics; the vendored proto schema does not define it.

**`RangeBearingCoordinateSystem` (BSI Flex 335 v2.0)**

| Proto value | Name | Bridge action |
|-------------|------|---------------|
| 1 | `DegreesM` | Azimuth/elevation degrees, range metres — pass through |
| 2 | `RadiansM` | Azimuth/elevation radians — converted to degrees |
| 3 | `DegreesKm` | Azimuth/elevation degrees, range km — range converted to metres |
| 4 | `RadiansKm` | Azimuth/elevation radians, range km — angles to degrees, range to metres |
| 5 | *(deprecated SAPIENT v7: degrees/feet)* | Range converted to metres |
| 6 | *(deprecated SAPIENT v7: radians/feet)* | Angles converted to degrees; range converted to metres |

Range-bearing detections require the sensor's last-known position from the node registry. If no position is available, `route_message` returns `Err(UnsupportedCoordinateSystem)`.

---

## DIL Resilience

Following the pattern from ADR-029 and ADR-058:

- **Outbound task queue**: tasks are queued when the SAPIENT link is down; replayed on reconnect in order.
- **Stale node detection**: `StatusReport` heartbeat interval is advertised in `NodeDescription`; the bridge marks a node as `STALE` after `2 × heartbeat_interval` without a status update.
- **Detection rate limiting**: configurable per-node cap on how many detections per second are relayed to the mesh (prevents flooding constrained mesh links with dense surveillance data).
- **Pending detection queue**: detections from unregistered sensors (or sensors whose position is not yet known for local-frame conversion) are held for up to a configurable window and released once registration arrives.

---

## Dependencies

| Crate | Version | Role |
|-------|---------|------|
| `peat-schema` | workspace path override | Peat document types |
| `prost` | 0.13 | Protobuf encode/decode |
| `prost-types` | 0.13 | Well-known types (Timestamp, etc.) |
| `tokio` | 1 | Async runtime, TCP, channels |
| `tokio-util` | 0.7 | Framed codec for length-prefixed TCP |
| `uuid` | 1 | SAPIENT node and detection IDs |
| `serde` / `serde_json` | 1 | Opaque extension serialisation |
| `tracing` | 0.1 | Structured logging |
| `thiserror` | 2 | Error types |

### Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `peat` | yes | Peat bridge layer (`peat-schema` dep, transforms, `SapientBridge`) |
| `v7` | no | JSON-based SAPIENT v7 support |
| `integration-tests` | no | Integration tests requiring the Dstl Apex middleware |

---

## External Crate Pattern

Following the pattern from ADR-058:

```
peat (main workspace)
├── peat-schema/      ← document types consumed by peat-sapient
└── ...

peat-mavlink (external)   ← MAVLink protocol bridge (ADR-058)
peat-sapient (external)   ← SAPIENT protocol bridge [THIS ADR]
```

---

## Alternatives Considered

### A. Integrate SAPIENT into `peat-transport` as a Transport Adapter

Implement `MeshTransport` for SAPIENT, carrying Peat sync bytes over SAPIENT's TCP channel.

**Rejected:** SAPIENT messages are semantically typed. The goal is to translate SAPIENT detections and tasks into Peat documents, not to tunnel Peat sync frames through SAPIENT. Same argument that rejected MAVLink-as-transport (ADR-058 §Why Not a Transport Adapter?).

### B. Extend `peat-tak-bridge` to Also Handle SAPIENT

Add SAPIENT ingest alongside CoT ingest in the existing TAK bridge crate.

**Rejected:** CoT and SAPIENT are structurally different protocols serving different purposes (SA/PLI vs. sensor/autonomous-platform management). Combining them in one crate creates a maintenance burden, dilutes the crate's identity, and violates single-responsibility. The bridge pattern scales horizontally, not vertically.

### C. Generate SAPIENT Protobuf Alongside `peat-schema`

Compile the SAPIENT proto definitions inside the `peat` workspace and expose the generated types from `peat-schema`.

**Rejected:** `peat-schema` is Peat's canonical schema layer; embedding an external standard's generated types would couple `peat-schema`'s versioning to Dstl's SAPIENT release cadence. The bridge crate is the correct isolation boundary.

---

## Consequences

### Positive

- **UK/NATO programme interoperability.** SAPIENT-compliant sensors and autonomous platforms can participate in the Peat mesh without modification to their firmware or ASM software.
- **Distributed HLDMM.** Multiple Peat nodes can co-operatively task SAPIENT sensors; CRDT-backed task state prevents conflicting commands.
- **Detection propagation.** Sensor detections propagate across the full mesh — any node can subscribe to `Track` updates from any SAPIENT source.
- **Clean dependency graph.** `peat-sapient` depends only on `peat-schema`; `peat-mesh` and `peat-sapient` are peers in the mission application.

### Negative

- **New crate to maintain.** SAPIENT v8 is an active standard; the proto definitions and message mappings will need updates as the spec evolves.
- **Coordinate conversion complexity.** Local-frame sensors require bridge-side conversion logic and a pending-queue for detections that arrive before registration.
- **SAPIENT v7 legacy burden.** If enabled via feature flag, v7 JSON parsing adds code surface without a protobuf codec.

### Risks

- **SAPIENT spec evolution.** Dstl releases point updates; breaking changes in the proto schema would require a bridge update. Mitigation: pin to a known-good proto release; track Dstl's public changelog.
- **Coordinate frame edge cases.** Sensors using local Cartesian frames with unusual reference point definitions may produce incorrect lat/lon after conversion. Mitigation: log the raw SAPIENT location alongside the converted value at DEBUG level; surface a `MappingError` if no anchor position is known.

---

## Future Work

1. **ASM relay mode.** A full pass-through mode where `peat-sapient` acts as an ASM broker, federating multiple SAPIENT networks under a single Peat mesh management plane.
2. **Fusion output.** Aggregating multiple `DetectionReport` streams from overlapping sensors into a single fused `Track` using a configurable Kalman-filter step inside `peat-sapient`.
3. **SAPIENT v9 / NATO STANAG alignment.** As SAPIENT progresses through NATO standardisation, the bridge will track the emerging STANAG number.
4. **TaskRegion CRDT.** Represent SAPIENT task regions as Peat CRDT documents so region changes are conflict-resolved across multiple Peat-side operators.

---

## References

- [SAPIENT Interface Control Document (Dstl public release)](https://github.com/dstl/SAPIENT-ICD)
- [SAPIENT v8 Proto definitions (Dstl)](https://github.com/dstl/SAPIENT-ICD)
- ADR-029 (TAK Transport Adapter)
- ADR-038 (Protocol-Level Format Transformation Primitives)
- ADR-058 (MAVLink Protocol Bridge)
- ADR-012 (Schema Definition & Protocol Extensibility)
