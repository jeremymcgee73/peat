//! Transport abstraction for mesh topology connections
//!
//! All transport types, traits, and iroh-backed implementations live in
//! [`peat_mesh::transport`] as of ADR-062 Phase 2 (peat#926). This
//! module re-exports the full surface for backwards compatibility with
//! the pre-Phase-2 `peat_protocol::transport::*` path.
//!
//! The `iroh` submodule (`peat_protocol::transport::iroh::IrohMeshTransport`)
//! was deleted in Phase 2 — its canonical home is
//! [`peat_mesh::transport::iroh_mesh::IrohMeshTransport`]. peat-ffi and
//! other consumers should switch their `use` lines to the new path
//! rather than continuing to reach for the deleted location.

// Re-export everything from peat-mesh's transport module, including the
// iroh-backed MeshTransport impl (added in peat-mesh 0.9.0-rc.21).
pub use peat_mesh::transport::*;
