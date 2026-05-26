//! Network layer for Peat Protocol
//!
//! This module provides both:
//! - Network simulation (bandwidth, latency, loss constraints)
//! - Real P2P transport via Iroh (for AutomergeIrohBackend)

// Network simulation modules
pub mod constraints;
pub mod metrics;
pub mod partition;
pub mod transport;

// Real P2P transport (Phase 3: Iroh integration)
#[cfg(feature = "automerge-backend")]
pub mod formation_handshake;
#[cfg(feature = "automerge-backend")]
pub mod iroh_transport;
#[cfg(feature = "automerge-backend")]
pub mod peer_config;

// Re-exports
#[cfg(feature = "automerge-backend")]
pub use formation_handshake::{perform_initiator_handshake, perform_responder_handshake};
#[cfg(feature = "automerge-backend")]
pub use iroh_transport::{
    IrohTransport, TransportEventReceiver, TransportEventSender, TransportPeerEvent,
    TRANSPORT_EVENT_CHANNEL_CAPACITY,
};
#[cfg(feature = "automerge-backend")]
pub use peer_config::{FormationConfig, LocalConfig, PeerConfig, PeerInfo};

// Re-export iroh primitives that surface in our public API. Downstream consumers
// should reach for these via `peat_protocol::network::*` rather than a direct
// `iroh = "..."` dep, so they don't have to track which iroh major peat-mesh
// transitively resolves to. ADR-062 Phase 2: peat-protocol no longer carries
// `iroh` as a direct dep; these come transitively via peat-mesh's
// `peat_mesh::network` re-exports (added in rc.21 / rc.22).
#[cfg(feature = "automerge-backend")]
pub use peat_mesh::network::{Connection, DiscoveryEvent, EndpointId};
