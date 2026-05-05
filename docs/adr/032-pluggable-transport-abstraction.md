# ADR-032: Pluggable Transport Abstraction for Multi-Network Operations

**Status**: Proposed
**Date**: 2025-12-07
**Updated**: 2026-05-05
**Authors**: Kit Plummer, Codex
**Relates to**: ADR-011 (Automerge + Iroh), ADR-017 (P2P Mesh Management), ADR-030 (Multi-Interface Transport), ADR-019 (QoS), ADR-046 (Targeted Delivery), ADR-039 (Peat-BTLE Mesh Transport), ADR-041 (Multi-Transport + Embedded Integration), ADR-052 (Peat-LoRa)
**Implements**: Issue #548
**Amends**: Issue #822 (Per-Peer Link State Query API)

---

## Context

### The Multi-Transport Reality

Tactical edge environments operate across diverse communication channels simultaneously:

| Transport | Bandwidth | Latency | Range | Power | Use Case |
|-----------|-----------|---------|-------|-------|----------|
| QUIC/Iroh | 1-100 Mbps | 1-50ms | Unlimited (IP) | Low | Primary mesh |
| WiFi Direct | 250 Mbps | 5-20ms | 200m | Medium | Peer-to-peer clusters |
| Bluetooth LE | 2 Mbps | 10-100ms | 100m | Very Low | Device pairing, beacons |
| LoRa | 0.3-50 kbps | 100-2000ms | 15km+ | Minimal | Long-range telemetry |
| Tactical Radio | 9.6-2000 kbps | 50-500ms | Variable | High | Secure MANET |
| Starlink | 50-200 Mbps | 20-40ms | Global | High | Backhaul to C2 |

### Three Dimensions of Multi-Transport

This ADR addresses three distinct but related requirements:

#### 1. Multiple Transport Types (Heterogeneous)
Different transport technologies with different characteristics:
- QUIC for IP networks
- BLE for close-range device mesh
- LoRa for long-range low-bandwidth
- Tactical radio for secure MANET

#### 2. Multiple Instances of Same Type (Homogeneous)
Multiple physical interfaces using the same transport protocol:
- Multiple NICs (eth0, wlan0, starlink0) all running QUIC/Iroh
- Multiple LoRa radios on different frequencies
- Multiple BLE adapters for different peer groups

#### 3. Simultaneous vs Failover Use
How multiple transports are used together:
- **PACE Failover**: Primary → Alternate → Contingency → Emergency
- **Simultaneous Redundant**: Send on multiple transports for reliability
- **Bonded/Aggregated**: Combine bandwidth across transports
- **Load Balanced**: Distribute traffic across transports

### Current Implementation Gap

**What exists today:**
- `MeshTransport` trait in `peat-protocol/src/transport/mod.rs`
- `IrohMeshTransport` implementing QUIC-based transport
- `DittoMeshTransport` delegating to Ditto's transport
- `HealthMonitor` for connection quality tracking

**What's missing:**
- Transport instance identification (multiple of same type)
- Transport capability declaration (bandwidth, latency, range, power)
- PACE-style transport policies
- Simultaneous transport modes (redundant, bonded)
- Collection ↔ transport binding
- Multi-transport coordination (TransportManager)

### ADR-030 vs ADR-032

**ADR-030** answered: "Does Iroh support multiple NICs?"
- Yes, Iroh automatically binds to all interfaces

**ADR-032** answers: "How does Peat manage multiple transports holistically?"
- Transport registration with unique IDs
- PACE-style failover policies
- Simultaneous use modes
- Collection-level transport preferences

---

## Decision Drivers

### Requirements

1. **Transport Independence**: Mesh logic must not depend on specific transport implementation
2. **Capability Awareness**: System must know what each transport can do (bandwidth, latency, range)
3. **Dynamic Selection**: Choose transport based on message size, urgency, and current conditions
4. **Graceful Degradation**: Fallback to alternative transport when primary fails
5. **Battery Efficiency**: Consider power consumption for mobile/tactical platforms
6. **Range Optimization**: Use appropriate transport for peer distance
7. **Security Parity**: All transports must support authentication/encryption

### Constraints

1. **Existing Trait**: Must extend or wrap existing `MeshTransport` trait
2. **Feature Flags**: Transport implementations behind cargo features
3. **Platform Support**: Some transports only available on specific platforms (BLE on mobile)
4. **No Breaking Changes**: Current `IrohMeshTransport` users unaffected

---

## Decision

### 1. Transport Instance Identification

Each transport instance has a unique string ID, allowing multiple instances of the same type:

```rust
/// Unique identifier for a transport instance
/// Format: "{type}-{interface}" or custom string
/// Examples: "iroh-eth0", "iroh-wlan0", "iroh-starlink0", "lora-915mhz", "ble-hci0"
pub type TransportId = String;

/// Transport instance registration
#[derive(Debug, Clone)]
pub struct TransportInstance {
    /// Unique instance identifier
    pub id: TransportId,

    /// Transport type (for capability grouping)
    pub transport_type: TransportType,

    /// Human-readable description
    pub description: String,

    /// Physical interface name (if applicable)
    pub interface: Option<String>,

    /// Current capabilities (may change with range mode)
    pub capabilities: TransportCapabilities,

    /// Is this transport currently available?
    pub available: bool,
}
```

**Example configuration:**
```yaml
transports:
  - id: "iroh-eth0"
    type: quic
    interface: eth0
    description: "Wired LAN"

  - id: "iroh-wlan0"
    type: quic
    interface: wlan0
    description: "WiFi mesh"

  - id: "iroh-starlink"
    type: quic
    interface: starlink0
    description: "Starlink backhaul"

  - id: "lora-primary"
    type: lora
    interface: /dev/ttyUSB0
    description: "LoRa 915MHz primary"

  - id: "lora-backup"
    type: lora
    interface: /dev/ttyUSB1
    description: "LoRa 868MHz backup"

  - id: "ble-mesh"
    type: bluetooth_le
    interface: hci0
    description: "BLE peripheral mesh"
```

### 2. PACE Transport Policy

Military PACE planning (Primary, Alternate, Contingency, Emergency) applied to transport selection:

```rust
/// PACE-style transport policy
#[derive(Debug, Clone)]
pub struct TransportPolicy {
    /// Policy name for reference
    pub name: String,

    /// Primary transports - use when available
    /// Multiple entries = load balance or redundant (based on mode)
    pub primary: Vec<TransportId>,

    /// Alternate - if all primary unavailable
    pub alternate: Vec<TransportId>,

    /// Contingency - degraded but functional
    pub contingency: Vec<TransportId>,

    /// Emergency - last resort, may have significant limitations
    pub emergency: Vec<TransportId>,
}

impl TransportPolicy {
    /// Standard PACE policy for tactical operations
    pub fn tactical_standard() -> Self {
        Self {
            name: "tactical-standard".into(),
            primary: vec!["iroh-eth0".into(), "iroh-wlan0".into()],
            alternate: vec!["iroh-starlink".into()],
            contingency: vec!["lora-primary".into()],
            emergency: vec!["ble-mesh".into()],
        }
    }

    /// Get transports in PACE order
    pub fn ordered(&self) -> impl Iterator<Item = &TransportId> {
        self.primary.iter()
            .chain(self.alternate.iter())
            .chain(self.contingency.iter())
            .chain(self.emergency.iter())
    }

    /// Get current PACE level based on what's available
    pub fn current_level(&self, available: &HashSet<TransportId>) -> PaceLevel {
        if self.primary.iter().any(|t| available.contains(t)) {
            PaceLevel::Primary
        } else if self.alternate.iter().any(|t| available.contains(t)) {
            PaceLevel::Alternate
        } else if self.contingency.iter().any(|t| available.contains(t)) {
            PaceLevel::Contingency
        } else if self.emergency.iter().any(|t| available.contains(t)) {
            PaceLevel::Emergency
        } else {
            PaceLevel::None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaceLevel {
    Primary = 0,
    Alternate = 1,
    Contingency = 2,
    Emergency = 3,
    None = 4,
}
```

### 3. Simultaneous Transport Modes

How multiple transports are used together:

```rust
/// How to use multiple available transports
#[derive(Debug, Clone, Copy, Default)]
pub enum TransportMode {
    /// Use single best transport from policy (PACE failover)
    #[default]
    Single,

    /// Send on multiple transports simultaneously for reliability
    /// Receiver deduplicates by message ID
    Redundant {
        /// Minimum transports to send on (default: 2)
        min_paths: u8,
        /// Maximum transports to send on (default: all available)
        max_paths: Option<u8>,
    },

    /// Aggregate bandwidth across transports (for large transfers)
    /// Splits message across transports, receiver reassembles
    Bonded,

    /// Distribute messages across transports (round-robin or weighted)
    LoadBalanced {
        /// Weight per transport (higher = more traffic)
        weights: Option<HashMap<TransportId, u8>>,
    },
}

impl TransportMode {
    pub fn redundant(min: u8) -> Self {
        Self::Redundant { min_paths: min, max_paths: None }
    }
}
```

### 4. Collection ↔ Transport Binding

Collections can specify transport preferences:

```rust
/// Collection configuration with transport preferences
pub struct CollectionConfig {
    /// Collection name
    pub name: String,

    /// Default delivery mode (from ADR-046)
    pub delivery_mode: DeliveryMode,

    /// Default TTL for documents
    pub default_ttl: Option<Duration>,

    // === NEW: Transport preferences ===

    /// Transport policy for this collection
    /// None = use node default policy
    pub transport_policy: Option<TransportPolicy>,

    /// How to use multiple transports
    pub transport_mode: TransportMode,

    /// Minimum PACE level required
    /// Documents won't send if below this level
    pub min_pace_level: Option<PaceLevel>,
}
```

**Example configurations:**

```yaml
collections:
  # Critical commands - redundant delivery
  commands:
    delivery_mode: targeted
    transport_mode:
      redundant:
        min_paths: 2
    min_pace_level: alternate  # Don't send if only contingency/emergency

  # Position updates - single best transport, tolerant of degradation
  positions:
    delivery_mode: broadcast
    transport_mode: single
    min_pace_level: emergency  # Send even on emergency transport

  # Large file transfers - bond transports for bandwidth
  blob_transfers:
    delivery_mode: targeted
    transport_mode: bonded
    transport_policy:
      name: "high-bandwidth-only"
      primary: ["iroh-eth0", "iroh-starlink"]
      alternate: []
      contingency: []
      emergency: []

  # Telemetry - load balance across available
  telemetry:
    delivery_mode: broadcast
    transport_mode:
      load_balanced:
        weights:
          iroh-eth0: 3
          iroh-wlan0: 2
          lora-primary: 1
```

### 5. Enhanced Transport Trait Hierarchy

```rust
/// Transport capability declaration
#[derive(Debug, Clone)]
pub struct TransportCapabilities {
    /// Transport type identifier
    pub transport_type: TransportType,

    /// Maximum bandwidth in bytes/second (0 = unknown)
    pub max_bandwidth_bps: u64,

    /// Typical latency in milliseconds
    pub typical_latency_ms: u32,

    /// Maximum practical range in meters (0 = unlimited/IP)
    pub max_range_meters: u32,

    /// Supports bidirectional streams
    pub bidirectional: bool,

    /// Supports reliable delivery (vs best-effort)
    pub reliable: bool,

    /// Battery impact score (0-100, 100 = high drain)
    pub battery_impact: u8,

    /// Supports broadcast/multicast
    pub supports_broadcast: bool,

    /// Requires pairing/bonding before use
    pub requires_pairing: bool,

    /// Maximum message size in bytes (0 = unlimited)
    pub max_message_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportType {
    /// QUIC over IP (Iroh)
    Quic,
    /// Classic Bluetooth (RFCOMM)
    BluetoothClassic,
    /// Bluetooth Low Energy (GATT)
    BluetoothLE,
    /// WiFi Direct (P2P)
    WifiDirect,
    /// LoRa (long range, low power)
    LoRa,
    /// Tactical radio (MANET)
    TacticalRadio,
    /// Satellite (Starlink, Iridium)
    Satellite,
    /// Custom/Vendor-specific
    Custom(u32),
}

/// Extended transport trait with capability advertisement
#[async_trait]
pub trait Transport: MeshTransport {
    /// Get transport capabilities
    fn capabilities(&self) -> &TransportCapabilities;

    /// Check if transport is currently available/enabled
    fn is_available(&self) -> bool;

    /// Get current signal quality (0-100, for wireless transports)
    fn signal_quality(&self) -> Option<u8> {
        None  // Default for wired/IP transports
    }

    /// Estimate if peer is reachable via this transport
    fn can_reach(&self, peer_id: &NodeId) -> bool;

    /// Get estimated delivery time for message of given size
    fn estimate_delivery_ms(&self, message_size: usize) -> u32 {
        let caps = self.capabilities();
        let transfer_time = if caps.max_bandwidth_bps > 0 {
            (message_size as u64 * 1000 / caps.max_bandwidth_bps) as u32
        } else {
            0
        };
        caps.typical_latency_ms + transfer_time
    }
}
```

### Amendment A — Per-Peer Link State Query (2026-05-05)

> **Note on placement.** This amendment is intentionally headed as "Amendment A" rather than continuing the existing numbered sequence (§§1–5, 2, 7). The numbered sequence in the original document has a pre-existing duplicate ("§2 PACE Transport Policy" and "§2 Message Requirements" both use the number 2) that is out of scope for this amendment to renumber. The amendment heading sits between §5 and the duplicate §2 in the source order; readers using a generated TOC will see it as a top-level amendment, not as part of the numbered Decision sequence.

The trait hierarchy above gives consumers transport-level capability and per-transport `signal_quality()`. What it does not yet give consumers is **per-peer link detail** — for a specific peer, which transport(s) is it currently reachable via, and at what link quality? That question is asked by visualization layers (the ATAK plugin's platform list, future desktop UIs, headless dashboards) and by transport-selection policies that want richer signals than `can_reach()`'s boolean.

The amendment adds a single transport-agnostic accessor on the `Transport` trait, plus the unified `LinkState` type that consumers walk over `TransportManager` to render the multi-transport view. This is the natural extension of the dual-mode pattern in ADR-039 §dual-mode and the multi-transport architecture in ADR-041: hosts that render multi-transport state use peat-mesh + transports as plugins (so they get the unified view); pure transport peripherals (M5Stack on xtensa-esp32, watch) may continue to use transport crates standalone — they are sources of state, not consumers of it.

```rust
/// Transport-agnostic per-peer link state.
///
/// Returned by `Transport::peer_link_state` for any transport that knows
/// about the given peer. Optional fields are populated where meaningful
/// for the transport and `None` otherwise — adding a new transport (LoRa,
/// satellite, future radios) requires no struct change.
#[derive(Debug, Clone)]
pub struct LinkState {
    /// Identifies which transport instance this state describes
    /// (e.g., "iroh-wlan0", "ble-hci0", "lora-915mhz"). Matches the
    /// `TransportInstance.id` registered in `TransportManager`.
    pub transport_id: TransportId,

    /// Transport family — for UI category grouping and per-transport
    /// rendering decisions.
    pub transport_type: TransportType,

    /// Physical interface name where applicable (eth0, wlan0,
    /// p2p-wlan0). None for transports that don't expose a NIC concept
    /// (e.g., BLE, LoRa).
    pub interface: Option<String>,

    /// Bucketed quality for UI tier indicators. Each transport
    /// implementation maps its raw signal/path-quality measure into
    /// this enum; consumers don't need to know transport-specific
    /// thresholds.
    pub quality: LinkQuality,

    /// Round-trip-time estimate in milliseconds, where the transport
    /// can measure or estimate it.
    pub rtt_ms: Option<u32>,

    /// Received signal strength in dBm, populated by transports that
    /// expose it (BLE, LoRa, tactical radio). None for IP transports.
    pub rssi_dbm: Option<i8>,

    /// Path classification for transports with a notion of direct vs
    /// relayed connectivity (iroh, future tactical radio with relay
    /// nodes). None where the concept doesn't apply.
    pub path_kind: Option<PathKind>,
}

/// Bucketed link quality. Each transport defines its own RSSI / RTT /
/// loss thresholds for the bucket boundaries; consumers only see the
/// bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkQuality {
    Excellent,
    Good,
    Fair,
    Weak,
    Unknown,
}

/// Connection path classification. Populated by IP-style transports
/// that expose direct-vs-relayed paths (iroh's `PathInfo::is_relay()`).
///
/// `Mixed` (multiple paths actively transmitting at once) was considered
/// during this amendment and intentionally deferred — no transport
/// currently has a multi-path-concurrent consumer that needs the
/// distinction, and adding the variant before a real emitter exists
/// would force consumers to write rendering code with no test path.
/// File as a follow-up to ADR-032 when iroh (or another transport) gains
/// a concurrent-multi-path mode that consumers must distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Direct peer-to-peer connection
    Direct,
    /// Connection routed through a relay
    Relay,
}

#[async_trait]
pub trait Transport: MeshTransport {
    // ... existing methods (capabilities, is_available, signal_quality,
    //     can_reach, estimate_delivery_ms) unchanged ...

    /// Get per-peer link state for visualization and selection.
    ///
    /// Returns `None` if this transport has no record of this peer
    /// (never seen, no connection history, or the transport simply
    /// doesn't track per-peer state). A peer with a known-disconnected
    /// state record SHOULD return `Some` with `LinkQuality::Weak` (or
    /// the transport's analogous degraded bucket), not `None` —
    /// "disconnected but known" is meaningful information for the
    /// visualization layer and should not be conflated with "no record
    /// at all". Returns `Some(LinkState)` with as much detail as the
    /// transport can provide — fields the transport doesn't measure
    /// stay `None`.
    ///
    /// Default impl returns `None`, so existing transport
    /// implementations continue to compile; visualization simply shows
    /// "unknown" until each transport opts in.
    ///
    /// `peer_id` is the unified peat-mesh `NodeId` (see "Identity
    /// bridging" below). Per-transport implementations are responsible
    /// for converting it to whatever native handle the underlying stack
    /// uses.
    fn peer_link_state(&self, peer_id: &peat_mesh::NodeId) -> Option<LinkState> {
        None
    }
}
```

#### Identity bridging

`peat_mesh::NodeId` is the unified peer identifier across all transports — the type already used by `MeshTransport::connect`, `can_reach`, and the rest of the peat-mesh `Transport` surface. Each `Transport` implementation is responsible for mapping it to whatever native peer handle the underlying stack uses, the same way per-transport `connect` already does:

- **iroh transport.** `peat_mesh::NodeId` ↔ iroh `NodeId` (both Ed25519-public-key-derived). The mapping is the identity function in this case.
- **`PeatBleTransport`.** `peat_mesh::NodeId` ↔ `peat_btle::NodeId` (a u32). The conversion already exists in peat-mesh today (`peat-mesh/src/transport/btle.rs::btle_to_peat_node_id` / `peat_to_btle_node_id`); `peer_link_state` reuses it before reaching into peat-btle's per-peer record.
- **Future `PeatLoraTransport`.** Whatever radio-layer addressing the LoRa stack uses (per ADR-052) gets bridged to `peat_mesh::NodeId` the same way, in the transport's own conversion functions.

If a transport cannot resolve the requested `NodeId` to one of its own peers, it returns `None` — same contract as `can_reach` returning `false`.

#### Per-transport implementation expectations

| Transport | Source of `LinkState` |
|---|---|
| `PeatBleTransport` (peat-mesh wrapping peat-btle) | Reads from peat-btle's per-peer connection record — `state` → `LinkQuality` (`Connected` → Good/Excellent depending on RSSI, `Degraded` → Fair, `Disconnected/Lost` → Weak), `last_rssi` → `rssi_dbm`. `LinkQuality` uses existing peat-btle thresholds (≥ −50 Excellent, ≥ −70 Good, ≥ −85 Fair, < −85 Weak). `path_kind` is `None` (BLE has no relay concept at this layer). `interface` is the local BLE adapter identifier where the platform exposes one. |
| iroh `Transport` (peat-mesh wrapping `MeshSyncTransport`) | Reads from `MeshSyncTransport::peer_paths(peer_id)` — selected path's `is_relay()` → `PathKind`, `peer_rtt(peer_id)` → `rtt_ms`. `interface` from `TransportInstance.interface` (e.g., "wlan0"). `rssi_dbm` is `None`. `quality` is bucketed from RTT + path kind per the table below. |

##### iroh quality bucketing

Concrete thresholds, applied in priority order (first match wins):

| Condition | `LinkQuality` |
|---|---|
| `path_kind == Relay` | `Fair` |
| `path_kind == Direct` and `rtt_ms < 30` | `Excellent` |
| `path_kind == Direct` and `rtt_ms < 100` | `Good` |
| `path_kind == Direct` and `rtt_ms < 300` | `Fair` |
| `path_kind == Direct` and `rtt_ms >= 300` | `Weak` |
| `rtt_ms` unavailable (e.g., new connection, no measurement yet) | `Unknown` |

These are starting-point thresholds, expected to be empirically retuned once the implementation has soak data. The point of locking them in the ADR is to keep peat-mesh and any future transport that reuses the bucketing pattern from drifting against each other in their first releases. Future transport implementations that follow this pattern (LoRa, satellite, tactical radio) define their own per-row thresholds in their own ADRs but reuse the same `LinkQuality` enum.
| Future `PeatLoraTransport` (per ADR-052) | Reads link metrics from the LoRa modem (RSSI, SNR, SF). `path_kind` = `None` initially; if/when LoRa relay lands, populates `Direct`/`Relay`. `interface` is the radio identifier. |

#### TransportManager surface for the unified loop

Three `TransportManager` accessors compose into the consumer-side loop. All three already exist in peat-mesh today (`peat-mesh/src/transport/manager.rs`); this section documents them for ADR cover, since the host-rendering rule below relies on them being part of the public surface:

```rust
impl TransportManager {
    /// Transports a given peer is currently reachable through, in
    /// implementation-defined preference order. Returns an empty Vec if
    /// the peer is not known to any registered transport.
    pub fn available_instances_for_peer(
        &self,
        peer_id: &peat_mesh::NodeId,
    ) -> Vec<TransportInstance> { /* ... */ }

    /// Transport types (deduplicated) a given peer is currently
    /// reachable through. Convenience wrapper for callers that only
    /// need the type-level view (e.g., "is this peer on BLE at all?").
    pub fn available_transports(
        &self,
        peer_id: &peat_mesh::NodeId,
    ) -> Vec<TransportType> { /* ... */ }

    /// Resolve a `TransportId` (from a `TransportInstance`) back to the
    /// concrete `Arc<dyn Transport>` so callers can invoke trait
    /// methods like `peer_link_state` against the right transport
    /// instance. Returns `None` if no transport is registered under
    /// that ID.
    pub fn get_transport(
        &self,
        transport_id: &TransportId,
    ) -> Option<Arc<dyn Transport>> { /* ... */ }
}
```

These are the three `TransportManager` methods the consumer-side rule depends on. Other manager surface (PACE policy, message-requirement matching) is unaffected by this amendment.

#### Host-rendering rule

Hosts that render or reason about multi-transport state MUST consume `LinkState` through peat-mesh's `TransportManager`. The unified loop is:

```rust
fn render_peer_links(manager: &TransportManager, peer_id: &peat_mesh::NodeId)
    -> Vec<LinkState>
{
    manager.available_instances_for_peer(peer_id)
        .into_iter()
        .filter_map(|instance| {
            let transport = manager.get_transport(&instance.id)?;
            transport.peer_link_state(peer_id)
        })
        .collect()
}
```

The same loop works regardless of how many transport types are registered, including transports added in the future.

Hosts MUST NOT reach into individual transport crates (peat-btle, peat-lora, etc.) to query per-peer state for visualization purposes; that path duplicates plumbing and ossifies the consumer against transport-specific facades. The consumer-side surface is `peat-mesh::Transport::peer_link_state` and nothing else.

Hosts that are **pure transport peripherals** — environments where peat-mesh genuinely cannot run (M5Stack on xtensa-esp32, watch where peat-mesh's deps are not earned) — may continue to use transport crates standalone (peat-btle's `translator-codec` Cargo feature, the equivalent in future peat-lora). These hosts produce state for the network but are not expected to render multi-transport views; the consumer-side rule above does not apply to them. ADR-039 §dual-mode and ADR-041 spell out which hosts fall into which category.

This amendment does not promote ADR-032 from Proposed to Accepted — that promotion belongs with the implementation that proves the trait extension out across at least two transports (peat-btle plugin mode + iroh), not with this doc change.

### 2. Message Requirements for Transport Selection

```rust
/// Requirements for message delivery
#[derive(Debug, Clone, Default)]
pub struct MessageRequirements {
    /// Minimum required bandwidth (bytes/second)
    pub min_bandwidth_bps: u64,

    /// Maximum acceptable latency (ms)
    pub max_latency_ms: Option<u32>,

    /// Message size in bytes (for capacity checking)
    pub message_size: usize,

    /// Requires reliable delivery
    pub reliable: bool,

    /// Priority level (higher = more important)
    pub priority: MessagePriority,

    /// Prefer low power consumption
    pub power_sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum MessagePriority {
    /// Background sync, can use any available transport
    Background = 0,
    /// Normal operational messages
    #[default]
    Normal = 1,
    /// Time-sensitive, prefer low-latency transports
    High = 2,
    /// Emergency/critical, use fastest available path
    Critical = 3,
}
```

### 7. Transport Manager (Updated)

```rust
/// Manages multiple transports with PACE policies and simultaneous modes
pub struct TransportManager {
    /// Registered transports by unique ID
    transports: HashMap<TransportId, Arc<dyn Transport>>,

    /// Transport instances metadata
    instances: HashMap<TransportId, TransportInstance>,

    /// Default PACE policy for the node
    default_policy: TransportPolicy,

    /// Per-collection policy overrides
    collection_policies: HashMap<String, TransportPolicy>,

    /// Active transports per peer (learned from successful deliveries)
    peer_transports: RwLock<HashMap<NodeId, Vec<TransportId>>>,

    /// Health monitor for transport quality tracking
    health_monitor: Arc<HealthMonitor>,

    /// Message deduplication for redundant mode
    seen_messages: RwLock<LruCache<MessageId, Instant>>,
}

impl TransportManager {
    /// Register a transport instance
    pub fn register(&mut self, id: TransportId, transport: Arc<dyn Transport>) {
        let instance = TransportInstance {
            id: id.clone(),
            transport_type: transport.capabilities().transport_type,
            description: String::new(),
            interface: None,
            capabilities: transport.capabilities().clone(),
            available: transport.is_available(),
        };
        self.instances.insert(id.clone(), instance);
        self.transports.insert(id, transport);
    }

    /// Get available transport IDs for a peer
    pub fn available_for_peer(&self, peer_id: &NodeId) -> HashSet<TransportId> {
        self.transports
            .iter()
            .filter(|(_, t)| t.is_available() && t.can_reach(peer_id))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get current PACE level for a peer
    pub fn pace_level_for_peer(
        &self,
        peer_id: &NodeId,
        policy: &TransportPolicy,
    ) -> PaceLevel {
        let available = self.available_for_peer(peer_id);
        policy.current_level(&available)
    }

    /// Select transports based on policy and mode
    pub fn select_transports(
        &self,
        peer_id: &NodeId,
        policy: &TransportPolicy,
        mode: TransportMode,
        requirements: &MessageRequirements,
    ) -> Vec<TransportId> {
        let available = self.available_for_peer(peer_id);

        // Filter by requirements first
        let candidates: Vec<_> = policy.ordered()
            .filter(|id| available.contains(*id))
            .filter(|id| {
                if let Some(transport) = self.transports.get(*id) {
                    self.meets_requirements(transport.as_ref(), requirements)
                } else {
                    false
                }
            })
            .cloned()
            .collect();

        match mode {
            TransportMode::Single => {
                // Return first available in PACE order
                candidates.into_iter().take(1).collect()
            }
            TransportMode::Redundant { min_paths, max_paths } => {
                let max = max_paths.unwrap_or(u8::MAX) as usize;
                let selected: Vec<_> = candidates.into_iter().take(max).collect();
                if selected.len() >= min_paths as usize {
                    selected
                } else {
                    vec![] // Can't meet minimum redundancy
                }
            }
            TransportMode::LoadBalanced { .. } => {
                // Return all candidates for load balancing
                candidates
            }
            TransportMode::Bonded => {
                // Return all high-bandwidth candidates
                candidates.into_iter()
                    .filter(|id| {
                        self.instances.get(id)
                            .map(|i| i.capabilities.max_bandwidth_bps > 1_000_000)
                            .unwrap_or(false)
                    })
                    .collect()
            }
        }
    }

    /// Send message with policy and mode
    pub async fn send(
        &self,
        peer_id: &NodeId,
        message_id: MessageId,
        data: &[u8],
        collection: &str,
        requirements: MessageRequirements,
    ) -> Result<SendResult, TransportError> {
        // Get policy (collection override or default)
        let policy = self.collection_policies
            .get(collection)
            .unwrap_or(&self.default_policy);

        // Get mode from collection config (simplified here)
        let mode = TransportMode::Single; // Would come from CollectionConfig

        let transports = self.select_transports(peer_id, policy, mode, &requirements);

        if transports.is_empty() {
            return Err(TransportError::NoTransportAvailable);
        }

        let mut results = Vec::new();
        for transport_id in &transports {
            if let Some(transport) = self.transports.get(transport_id) {
                match transport.connect(peer_id).await {
                    Ok(conn) => {
                        // Send via this transport
                        results.push((transport_id.clone(), Ok(())));
                    }
                    Err(e) => {
                        results.push((transport_id.clone(), Err(e)));
                    }
                }
            }
        }

        Ok(SendResult {
            message_id,
            transports_used: results,
            pace_level: policy.current_level(&self.available_for_peer(peer_id)),
        })
    }

    fn meets_requirements(&self, transport: &dyn Transport, req: &MessageRequirements) -> bool {
        let caps = transport.capabilities();

        if req.reliable && !caps.reliable {
            return false;
        }
        if caps.max_bandwidth_bps > 0 && caps.max_bandwidth_bps < req.min_bandwidth_bps {
            return false;
        }
        if caps.max_message_size > 0 && caps.max_message_size < req.message_size {
            return false;
        }
        if let Some(max_latency) = req.max_latency_ms {
            let est = transport.estimate_delivery_ms(req.message_size);
            if est > max_latency {
                return false;
            }
        }
        true
    }
}

#[derive(Debug)]
pub struct SendResult {
    pub message_id: MessageId,
    pub transports_used: Vec<(TransportId, Result<(), TransportError>)>,
    pub pace_level: PaceLevel,
}
```

### 8. Transport Implementation Examples

#### Bluetooth LE Transport (Conceptual)

```rust
#[cfg(feature = "bluetooth")]
pub struct BluetoothLETransport {
    capabilities: TransportCapabilities,
    adapter: BluetoothAdapter,
    connections: RwLock<HashMap<NodeId, GattConnection>>,
    service_uuid: Uuid,
}

impl BluetoothLETransport {
    pub fn new(adapter: BluetoothAdapter) -> Self {
        Self {
            capabilities: TransportCapabilities {
                transport_type: TransportType::BluetoothLE,
                max_bandwidth_bps: 250_000,  // ~2 Mbps theoretical
                typical_latency_ms: 30,
                max_range_meters: 100,
                bidirectional: true,
                reliable: true,
                battery_impact: 15,  // BLE is very efficient
                supports_broadcast: true,  // Advertising
                requires_pairing: false,   // Can use just-works or no pairing
                max_message_size: 512,     // MTU limit per characteristic
            },
            adapter,
            connections: RwLock::new(HashMap::new()),
            service_uuid: Uuid::parse_str("Peat-SERVICE-UUID").unwrap(),
        }
    }
}

#[async_trait]
impl Transport for BluetoothLETransport {
    fn capabilities(&self) -> &TransportCapabilities {
        &self.capabilities
    }

    fn is_available(&self) -> bool {
        self.adapter.is_powered_on()
    }

    fn signal_quality(&self) -> Option<u8> {
        // Return average RSSI across connections
        Some(75) // Placeholder
    }

    fn can_reach(&self, peer_id: &NodeId) -> bool {
        // Check if peer is in BLE range (via scan or cached info)
        self.connections.read().unwrap().contains_key(peer_id)
    }
}
```

#### LoRa Transport (Conceptual)

```rust
#[cfg(feature = "lora")]
pub struct LoRaTransport {
    capabilities: TransportCapabilities,
    radio: LoRaRadio,
    spreading_factor: u8,
}

impl LoRaTransport {
    pub fn new(radio: LoRaRadio, spreading_factor: u8) -> Self {
        // Higher SF = longer range, lower bandwidth
        let bandwidth = match spreading_factor {
            7 => 21_900,   // ~21.9 kbps
            8 => 12_500,   // ~12.5 kbps
            9 => 7_000,    // ~7 kbps
            10 => 3_900,   // ~3.9 kbps
            11 => 2_100,   // ~2.1 kbps
            12 => 1_100,   // ~1.1 kbps
            _ => 5_000,    // Default
        };

        Self {
            capabilities: TransportCapabilities {
                transport_type: TransportType::LoRa,
                max_bandwidth_bps: bandwidth,
                typical_latency_ms: 500,
                max_range_meters: 15_000,  // 15km typical
                bidirectional: true,
                reliable: false,  // Best-effort (can add ACKs)
                battery_impact: 10,
                supports_broadcast: true,
                requires_pairing: false,
                max_message_size: 255,  // LoRa packet size limit
            },
            radio,
            spreading_factor,
        }
    }
}
```

---

## Dynamic Range/Bandwidth Tradeoffs

### The Range-Bandwidth Tradeoff

Many radio technologies allow dynamically trading bandwidth for range. This is a critical capability for tactical operations where peer distance varies significantly.

| Technology | Mechanism | Range Boost | Bandwidth Cost |
|------------|-----------|-------------|----------------|
| **Bluetooth LE** | Coded PHY (S=2) | 2x | 2x slower |
| **Bluetooth LE** | Coded PHY (S=8) | 4x | 4x slower |
| **LoRa** | Spreading Factor 7→12 | ~4x | ~20x slower |
| **WiFi** | 802.11b vs 802.11ax | ~2x | ~100x slower |
| **Tactical Radio** | Modulation scheme | Varies | Varies |

### Range Mode Abstraction

```rust
/// Available range modes for a transport
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeMode {
    /// Default/balanced mode
    Standard,
    /// Extended range at cost of bandwidth
    Extended,
    /// Maximum range (lowest bandwidth)
    Maximum,
    /// Custom configuration
    Custom(u8),  // Transport-specific value
}

/// Range mode capabilities for a transport
#[derive(Debug, Clone)]
pub struct RangeModeConfig {
    /// Available modes for this transport
    pub available_modes: Vec<RangeMode>,
    /// Current active mode
    pub current_mode: RangeMode,
    /// Capabilities per mode
    pub mode_capabilities: HashMap<RangeMode, TransportCapabilities>,
}

/// Extended transport trait with range mode support
#[async_trait]
pub trait ConfigurableTransport: Transport {
    /// Get available range modes
    fn range_modes(&self) -> Option<&RangeModeConfig> {
        None  // Default: not configurable
    }

    /// Set range mode (returns new capabilities)
    async fn set_range_mode(&self, mode: RangeMode) -> Result<TransportCapabilities, TransportError> {
        Err(TransportError::Other("Range mode not supported".into()))
    }

    /// Get recommended mode for target distance
    fn recommend_mode_for_distance(&self, distance_meters: u32) -> Option<RangeMode> {
        let config = self.range_modes()?;

        // Find mode with sufficient range and best bandwidth
        config.mode_capabilities
            .iter()
            .filter(|(_, caps)| caps.max_range_meters >= distance_meters)
            .max_by_key(|(_, caps)| caps.max_bandwidth_bps)
            .map(|(mode, _)| *mode)
    }
}
```

### Bluetooth LE Coded PHY Example

```rust
#[cfg(feature = "bluetooth")]
impl ConfigurableTransport for BluetoothLETransport {
    fn range_modes(&self) -> Option<&RangeModeConfig> {
        Some(&self.range_config)
    }

    async fn set_range_mode(&self, mode: RangeMode) -> Result<TransportCapabilities, TransportError> {
        let phy = match mode {
            RangeMode::Standard => BlePhy::Le1M,      // Standard 1 Mbps
            RangeMode::Extended => BlePhy::LeCoded2,  // Coded S=2, 500 kbps, 2x range
            RangeMode::Maximum => BlePhy::LeCoded8,   // Coded S=8, 125 kbps, 4x range
            _ => return Err(TransportError::Other("Unsupported mode".into())),
        };

        self.adapter.set_phy(phy).await?;

        // Return new capabilities
        Ok(match mode {
            RangeMode::Standard => TransportCapabilities {
                max_bandwidth_bps: 1_000_000,
                max_range_meters: 100,
                ..self.capabilities.clone()
            },
            RangeMode::Extended => TransportCapabilities {
                max_bandwidth_bps: 500_000,
                max_range_meters: 200,
                battery_impact: 20,  // Slightly higher power
                ..self.capabilities.clone()
            },
            RangeMode::Maximum => TransportCapabilities {
                max_bandwidth_bps: 125_000,
                max_range_meters: 400,
                battery_impact: 25,
                ..self.capabilities.clone()
            },
            _ => self.capabilities.clone(),
        })
    }
}
```

### LoRa Adaptive Spreading Factor

```rust
#[cfg(feature = "lora")]
impl ConfigurableTransport for LoRaTransport {
    fn range_modes(&self) -> Option<&RangeModeConfig> {
        Some(&self.range_config)
    }

    async fn set_range_mode(&self, mode: RangeMode) -> Result<TransportCapabilities, TransportError> {
        let sf = match mode {
            RangeMode::Standard => 7,   // SF7: ~6km, 21.9 kbps
            RangeMode::Extended => 10,  // SF10: ~12km, 3.9 kbps
            RangeMode::Maximum => 12,   // SF12: ~15km+, 1.1 kbps
            RangeMode::Custom(sf) if (7..=12).contains(&sf) => sf,
            _ => return Err(TransportError::Other("Invalid SF".into())),
        };

        self.radio.set_spreading_factor(sf).await?;

        // Capabilities scale with spreading factor
        let (bandwidth, range) = match sf {
            7 => (21_900, 6_000),
            8 => (12_500, 8_000),
            9 => (7_000, 10_000),
            10 => (3_900, 12_000),
            11 => (2_100, 14_000),
            12 => (1_100, 15_000),
            _ => (5_000, 10_000),
        };

        Ok(TransportCapabilities {
            max_bandwidth_bps: bandwidth,
            max_range_meters: range,
            typical_latency_ms: 100 + (sf as u32 - 7) * 100,  // Higher SF = more airtime
            ..self.capabilities.clone()
        })
    }
}
```

### TransportManager Integration

The `TransportManager` should consider range requirements when selecting transports:

```rust
impl TransportManager {
    /// Select transport with range mode adaptation
    pub async fn select_transport_for_distance(
        &self,
        peer_id: &NodeId,
        distance_meters: Option<u32>,
        requirements: &MessageRequirements,
    ) -> Result<(TransportType, Option<RangeMode>), TransportError> {
        let transport_type = self.select_transport(peer_id, requirements)
            .ok_or_else(|| TransportError::PeerNotFound(peer_id.to_string()))?;

        let transport = self.transports.get(&transport_type).unwrap();

        // Check if we need to adjust range mode
        let range_mode = if let Some(dist) = distance_meters {
            if let Some(configurable) = transport.as_configurable() {
                configurable.recommend_mode_for_distance(dist)
            } else {
                None
            }
        } else {
            None
        };

        Ok((transport_type, range_mode))
    }

    /// Send with automatic range adaptation
    pub async fn send_adaptive(
        &self,
        peer_id: &NodeId,
        data: &[u8],
        requirements: MessageRequirements,
        estimated_distance: Option<u32>,
    ) -> Result<(), TransportError> {
        let (transport_type, range_mode) = self
            .select_transport_for_distance(peer_id, estimated_distance, &requirements)
            .await?;

        let transport = self.transports.get(&transport_type).unwrap();

        // Adjust range mode if needed
        if let Some(mode) = range_mode {
            if let Some(configurable) = transport.as_configurable() {
                configurable.set_range_mode(mode).await?;
            }
        }

        // Send via transport
        let conn = transport.connect(peer_id).await?;
        // ... send data

        Ok(())
    }
}
```

### Distance Estimation

For automatic range mode selection, the system needs distance estimates:

```rust
/// How peer distance was determined
#[derive(Debug, Clone)]
pub enum DistanceSource {
    /// GPS coordinates from both peers
    Gps { confidence_meters: u32 },
    /// Signal strength (RSSI) estimation
    Rssi { estimated_meters: u32, variance: u32 },
    /// Time-of-flight measurement
    Tof { precision_ns: u32 },
    /// Manual/configured
    Configured,
    /// Unknown distance
    Unknown,
}

/// Peer distance information
#[derive(Debug, Clone)]
pub struct PeerDistance {
    pub peer_id: NodeId,
    pub distance_meters: u32,
    pub source: DistanceSource,
    pub last_updated: Instant,
}
```

This integrates with the geographic beacon system (ADR-024) to provide distance estimates for range mode selection.

---

## Implementation Plan

### Phase 1: Core Abstractions (Issue #255)

**Goal**: Define traits and refactor IrohMeshTransport

- [ ] Define `TransportCapabilities` struct
- [ ] Define `TransportType` enum
- [ ] Define `Transport` trait extending `MeshTransport`
- [ ] Define `MessageRequirements` and `MessagePriority`
- [ ] Define `RangeMode` and `ConfigurableTransport` trait
- [ ] Implement `Transport` for `IrohMeshTransport`
- [ ] Add capability constants for QUIC transport
- [ ] Unit tests for transport selection logic

**Estimated scope**: ~600 lines new code, ~100 lines modifications

### Phase 2: Transport Manager

**Goal**: Multi-transport coordination

- [ ] Implement `TransportManager` struct
- [ ] Implement transport registration/unregistration
- [ ] Implement transport selection algorithm
- [ ] Add peer-transport affinity tracking
- [ ] Integrate with existing `MeshRouter`
- [ ] Add fallback logic when primary transport fails

**Estimated scope**: ~800 lines

### Phase 3: Bluetooth Transport (Separate Issue)

**Goal**: Android/iOS peer discovery and messaging

- [ ] Create `peat-bluetooth` crate (behind feature flag)
- [ ] Implement `BluetoothLETransport`
- [ ] Implement BLE advertising for peer discovery
- [ ] Implement GATT service for data exchange
- [ ] Handle pairing and bonding
- [ ] Platform-specific implementations (Android NDK, iOS)

**Estimated scope**: ~2000 lines + platform-specific code

### Phase 4: LoRa Transport (Separate Issue)

**Goal**: Long-range, low-power telemetry

- [ ] Create `peat-lora` crate (behind feature flag)
- [ ] Implement `LoRaTransport`
- [ ] Handle spreading factor selection
- [ ] Implement duty cycle management
- [ ] Add CAD (channel activity detection)
- [ ] Define compact message format for low bandwidth

**Estimated scope**: ~1500 lines

### Phase 5: WiFi Direct Transport (Separate Issue)

**Goal**: High-bandwidth peer-to-peer clusters

- [ ] Create `peat-wifi-direct` crate
- [ ] Implement `WifiDirectTransport`
- [ ] Handle group formation (GO negotiation)
- [ ] Integrate with IP-based QUIC after connection

**Estimated scope**: ~1200 lines

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Application Layer                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                      │
│  │ CRDT Sync   │  │ Telemetry   │  │ Commands    │                      │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                      │
│         │                │                │                              │
│         └────────────────┼────────────────┘                              │
│                          ▼                                               │
│         ┌────────────────────────────────┐                               │
│         │        Transport Manager        │ ◄── Transport Selection      │
│         │   (Multi-Transport Coordinator) │     Message Requirements     │
│         └──────────────┬─────────────────┘                               │
│                        │                                                 │
├────────────────────────┼────────────────────────────────────────────────┤
│                Transport Abstraction Layer                               │
│                        │                                                 │
│         ┌──────────────┴──────────────┐                                  │
│         ▼              ▼              ▼              ▼                   │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐            │
│  │   QUIC     │ │ Bluetooth  │ │   LoRa     │ │ WiFi Direct│            │
│  │  (Iroh)    │ │    LE      │ │            │ │            │            │
│  └─────┬──────┘ └─────┬──────┘ └─────┬──────┘ └─────┬──────┘            │
│        │              │              │              │                    │
├────────┼──────────────┼──────────────┼──────────────┼───────────────────┤
│        ▼              ▼              ▼              ▼                    │
│  ┌──────────────────────────────────────────────────────────┐           │
│  │                    Physical Layer                          │           │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐       │           │
│  │  │ Ethernet│  │  BLE    │  │ LoRa    │  │  WiFi   │       │           │
│  │  │  NIC    │  │ Adapter │  │ Radio   │  │ Adapter │       │           │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘       │           │
│  └──────────────────────────────────────────────────────────┘           │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Transport Selection Algorithm

```
FUNCTION select_transport(peer_id, requirements):
    available = []

    FOR each transport in registered_transports:
        IF transport.is_available() AND transport.can_reach(peer_id):
            IF meets_requirements(transport, requirements):
                score = calculate_score(transport, requirements)
                available.append((transport, score))

    IF available.is_empty():
        RETURN None

    RETURN transport with highest score

FUNCTION meets_requirements(transport, requirements):
    caps = transport.capabilities()

    # Hard requirements - must be met
    IF requirements.reliable AND NOT caps.reliable:
        RETURN False
    IF caps.max_bandwidth < requirements.min_bandwidth:
        RETURN False
    IF caps.max_message_size > 0 AND caps.max_message_size < requirements.message_size:
        RETURN False
    IF requirements.max_latency IS SET:
        est_delivery = transport.estimate_delivery_ms(requirements.message_size)
        IF est_delivery > requirements.max_latency:
            RETURN False

    RETURN True

FUNCTION calculate_score(transport, requirements):
    caps = transport.capabilities()
    score = 100

    # Latency bonus for high-priority messages
    IF requirements.priority >= High:
        score += 50 - min(caps.typical_latency_ms, 50)

    # Power penalty if power-sensitive
    IF requirements.power_sensitive:
        score -= caps.battery_impact

    # Preference bonus
    IF transport.type in preference_order:
        idx = preference_order.index(transport.type)
        score += 20 - (idx * 5)

    # Signal quality bonus for wireless
    IF transport.signal_quality() IS SET:
        score += transport.signal_quality() / 10

    RETURN score
```

---

## Success Criteria

### Functional Requirements

- [ ] Single node can register multiple transports
- [ ] Transport selection respects message requirements
- [ ] Fallback works when primary transport fails
- [ ] Peer-transport affinity reduces selection overhead
- [ ] All transports share same `NodeId` namespace

### Performance Requirements

- [ ] Transport selection < 1ms
- [ ] No memory overhead when transport not in use
- [ ] Graceful degradation under load

### Testing

- [ ] Unit tests for TransportManager selection logic
- [ ] Integration tests with mock transports
- [ ] E2E test with QUIC + simulated LoRa

---

## Security Considerations

### Per-Transport Security

| Transport | Authentication | Encryption | Notes |
|-----------|---------------|------------|-------|
| QUIC/Iroh | TLS 1.3 | Built-in | Strongest |
| Bluetooth LE | BLE pairing | AES-CCM | Good |
| WiFi Direct | WPS/WPA2 | AES | Good |
| LoRa | Custom | Optional | Add app-layer encryption |
| Tactical Radio | Built-in | MIL-STD | Platform-dependent |

### Recommendations

1. **Enforce encryption at transport manager level** - Reject transports without encryption for sensitive messages
2. **Node identity verification** - Use same keypair across all transports
3. **Message authentication** - Sign messages at application layer (independent of transport)

---

## Open Questions

### Critical: Policy Scope (Team Discussion Required)

**⚠️ This decision has significant architectural implications and should be discussed by the full team before implementation.**

Where does PACE transport policy live, and how do the layers interact?

```
┌─────────────────────────────────────────────────────────────────┐
│                    Policy Scope Options                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Option A: Node Default Only                                    │
│   ┌─────────────┐                                               │
│   │ Node Policy │ ──────────────────────────────► All traffic   │
│   └─────────────┘                                               │
│   Simple, but no flexibility for different data types            │
│                                                                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Option B: Collection Override                                  │
│   ┌─────────────┐     ┌───────────────────┐                     │
│   │ Node Policy │ ◄── │ Collection Policy │ (optional override) │
│   └─────────────┘     └───────────────────┘                     │
│   Collections like "commands" can require redundant delivery     │
│   while "telemetry" uses single/load-balanced                   │
│                                                                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Option C: Full Cascade (Node → Collection → Message)           │
│   ┌─────────────┐     ┌───────────────────┐     ┌─────────────┐ │
│   │ Node Policy │ ◄── │ Collection Policy │ ◄── │ WriteOptions│ │
│   └─────────────┘     └───────────────────┘     └─────────────┘ │
│   Maximum flexibility, but complex precedence rules              │
│   WriteOptions could specify: "use redundant for this message"   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Considerations:**

| Aspect | Node Only (A) | + Collection (B) | + Message (C) |
|--------|---------------|------------------|---------------|
| Simplicity | ✅ Simple | 🔶 Moderate | ❌ Complex |
| Flexibility | ❌ None | ✅ Good | ✅✅ Maximum |
| Predictability | ✅ Easy to reason | ✅ Per-collection | ❌ Per-message variance |
| Config burden | ✅ One place | 🔶 Per collection | ❌ Throughout code |
| Runtime overhead | ✅ Minimal | 🔶 Lookup per collection | ❌ Evaluate per message |

**Recommendation for discussion**: Option B (Node + Collection) provides good balance. Per-message override (Option C) adds complexity that may not be worth it.

**Questions to resolve:**
1. If collection policy is stricter than node policy, which wins?
2. Should policy be validated at config time or runtime?
3. How do subscriptions interact? (subscriber specifies preference vs publisher decides)

---

### Other Open Questions

1. **Message Fragmentation**: How to handle large messages on limited transports (LoRa 255-byte limit)?
   - Option A: Transport-layer fragmentation
   - Option B: Application-layer chunking (recommended)

2. **Discovery Protocol**: How do peers advertise which transports they support?
   - Option A: Extended beacon with transport capabilities
   - Option B: Separate discovery protocol per transport
   - **Note**: Beacon should include available `TransportId` list

3. **Connection Handoff**: How to migrate active streams between transports?
   - Option A: Don't migrate - just use best for new messages
   - Option B: QUIC connection migration (only works for IP-based)

4. **Redundant Mode Deduplication**: When sending on multiple transports:
   - Who deduplicates? Receiver must track message IDs
   - How long to keep dedup cache? (TTL-based)
   - Does this interact with CRDT deduplication?

5. **Bonded Mode Reassembly**: For bandwidth aggregation:
   - How are chunks distributed across transports?
   - What if one transport fails mid-transfer?
   - Is this even needed given CRDT sync handles large docs?

6. **Range Mode Coordination**: When should range mode changes be synchronized across peers?
   - Option A: Sender-driven - sender sets mode based on estimated distance
   - Option B: Negotiated - both peers agree on mode before communication
   - Option C: Asymmetric - each direction can use different modes

7. **Subscription Transport Preference**: Can a subscriber specify transport preferences?
   ```rust
   // Does this make sense?
   store.subscribe_with_options(
       "positions",
       SubscribeOptions {
           preferred_transports: vec!["lora-primary"], // Only receive via LoRa
           ..Default::default()
       }
   )
   ```
   - Or is transport purely a sender/publisher concern?

8. **PACE Level Notifications**: Should the application be notified when PACE level degrades?
   ```rust
   // Event when dropping from Primary to Alternate?
   transport_manager.on_pace_change(|old, new| {
       if new > PaceLevel::Alternate {
           warn!("Operating on contingency/emergency comms");
       }
   });
   ```

---

## References

1. [Iroh Multipath](https://iroh.computer/docs) - Native multi-interface support
2. [QUIC Multipath RFC](https://datatracker.ietf.org/doc/draft-ietf-quic-multipath/)
3. [Bluetooth Core Spec 5.4](https://www.bluetooth.com/specifications/specs/core-specification-5-4/)
4. [LoRa Alliance Specifications](https://lora-alliance.org/resource_hub/lorawan-specification-v1-0-4/)
5. [WiFi Direct P2P Specification](https://www.wi-fi.org/discover-wi-fi/wi-fi-direct)
6. ADR-011: CRDT + Networking Stack Selection
7. ADR-017: P2P Mesh Management and Discovery
8. ADR-030: Multi-Interface Transport

---

**Last Updated**: 2025-12-07
**Status**: PROPOSED - Awaiting discussion
