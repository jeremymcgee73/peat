//! Re-export shim for the moved Iroh QUIC transport.
//!
//! The canonical implementation lives at
//! [`peat_mesh::network::iroh_transport`] as of peat-mesh 0.9.0-rc.18
//! and peat#918 (ADR-062 Phase 1). This file is a thin re-export so
//! peat-protocol consumers that historically imported
//! `peat_protocol::network::iroh_transport::IrohTransport` (and the
//! companion `TransportPeerEvent`, channel typedefs, `CAP_AUTOMERGE_ALPN`,
//! `connection_recycle_interval_secs`, etc.) keep compiling.
//!
//! **Wire-visible NodeId break at the rc.13 → rc.14 boundary.** The
//! deterministic-seed `IrohTransport::from_seed` / `from_seed_at_addr`
//! constructors now derive the iroh `SecretKey` via HKDF-SHA-256 (v2
//! salt/info) instead of the legacy `SHA-256("peat-iroh-key-v1:" || seed)`
//! prefix construction. Static peer-config TOML referencing pre-rc.14
//! NodeIds will need regeneration; see
//! [peat-mesh CHANGELOG 0.9.0-rc.18](https://github.com/defenseunicorns/peat-mesh/blob/main/CHANGELOG.md)
//! for the operator-facing upgrade steps.
//!
//! A future cleanup pass may delete this file entirely and migrate
//! consumers to the `peat_mesh::network::iroh_transport` path directly.
//! Keeping the shim now keeps the consumer-side diff in this PR
//! bounded.

#[cfg(feature = "automerge-backend")]
pub use peat_mesh::network::iroh_transport::*;
