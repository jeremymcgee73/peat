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
//
// peat-mesh 0.9.0-rc.24 (peat-mesh#173 / peat#932): `Connection` re-export
// removed in favor of the narrow `QuicMeshConnection` trait. The trait
// has four methods (`open_bi`, `accept_bi`, `close_reason`,
// `remote_endpoint_id`). peat-protocol's `formation_handshake.rs`
// uses **`open_bi` and `accept_bi`** through the trait object
// (`&dyn QuicMeshConnection`) — those two are the load-bearing calls
// at the abstracted handshake-side surface. `close_reason` and
// `remote_endpoint_id` are part of the trait for symmetry with the
// upstream `iroh::endpoint::Connection` method shape: peat-mesh
// chose to keep them in the trait so a future peat-protocol consumer
// holding `&dyn QuicMeshConnection` (rather than a concrete
// `Connection`) can probe close state or remote identity without
// reaching back through the iroh-typed handle. Today's `close_reason`
// callsites in `sync/automerge.rs` are on the concrete `Connection`
// (inherent impl), not through the trait — so dropping
// `close_reason`/`remote_endpoint_id` from the trait wouldn't
// regress any current call site, but would close off the abstracted
// path that the next consumer naturally reaches for. Keeping them is
// the deliberate forward-compat choice.
//
// A future iroh `Connection` method addition still doesn't widen
// peat-protocol's reachable surface by default — widening the trait
// requires a peat-mesh PR.
#[cfg(feature = "automerge-backend")]
pub use peat_mesh::network::{DiscoveryEvent, EndpointId, QuicMeshConnection};
