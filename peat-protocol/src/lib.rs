//! # Peat Protocol - Hierarchical Intelligence for Versatile Entities
//!
//! A hierarchical capability composition protocol using CRDTs for autonomous systems.
//!
//! ## Overview
//!
//! The Peat protocol enables scalable coordination of autonomous nodes through:
//! - **Three-phase protocol**: Discovery, Cell Formation, Hierarchical Operations
//! - **CRDT-based state**: Eventual consistency via Automerge over Iroh QUIC
//! - **Capability composition**: Additive, emergent, redundant, and constraint-based patterns
//!
//! ## Facade
//!
//! `peat-protocol` is the public entry point to the Peat stack. It re-exports
//! `peat_schema` (wire types) and `peat_mesh` (P2P plumbing) so downstream
//! consumers can depend on `peat-protocol` alone:
//!
//! ```toml
//! # During an rc window, Cargo does not pick pre-release versions by default
//! # — use the exact pin:
//! peat-protocol = "=0.9.0-rc.1"
//!
//! # Once 0.9.0 stable is published, the normal caret selector is fine:
//! # peat-protocol = "0.9"
//! ```
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
//! │   Phase 1:   │→ │   Phase 2:   │→ │   Phase 3:   │
//! │  Discovery   │  │    Cell      │  │ Hierarchical │
//! │              │  │  Formation   │  │  Operations  │
//! └──────────────┘  └──────────────┘  └──────────────┘
//! ```
//!
//! ## Cargo features
//!
//! | Feature | Default | Purpose |
//! |---------|---------|---------|
//! | `automerge-backend` | yes | Automerge CRDT over Iroh QUIC — the standard backend. |
//! | `lite-transport` | no | UDP-based `peat-lite` bridge for microcontrollers. |
//! | `bluetooth` | no | `peat-btle` BLE mesh transport. |
//! | `relay-n0-hosted` | **no** | Opt-in to n0's hosted iroh relay pool and DNS pkarr discovery. **See the security note below.** |
//!
//! ### Relay policy (`relay-n0-hosted`)
//!
//! By default, `peat-protocol`'s [`IrohTransport`](crate::network::iroh_transport::IrohTransport)
//! constructors build endpoints with `iroh::Endpoint::builder(presets::Empty)`
//! plus an explicit `.crypto_provider(rustls::crypto::aws_lc_rs::default_provider())`
//! — **no third-party relay servers, no DNS pkarr discovery via n0-hosted
//! infrastructure**. NAT traversal relies on direct addresses or LAN
//! discovery (mDNS via `address_lookup`). This is the correct posture for
//! tactical, edge, and air-gapped deployments where peer traffic must not
//! transit a third-party CDN. Iroh 0.98 retired the older `empty_builder()`
//! convenience; the `presets::Empty` shape is the equivalent (n0-computer/iroh#3978).
//!
//! Enabling the `relay-n0-hosted` feature flips every `IrohTransport`
//! constructor (and any code that calls them) to
//! `iroh::Endpoint::builder(iroh::endpoint::presets::N0)`, which restores
//! n0's hosted relay pool (`*.iroh.network`, `*.iroh-canary.iroh.link`) and
//! DNS pkarr discovery. Use this only when you have explicit authorization
//! to route peer traffic through n0's infrastructure.
//!
//! ```toml
//! # Default — no n0 phone-home:
//! peat-protocol = "=0.9.0-rc.1"
//!
//! # Opt-in — restores n0 hosted relay/DNS:
//! peat-protocol = { version = "=0.9.0-rc.1", features = ["relay-n0-hosted"] }
//! ```
//!
//! See issue [#833](https://github.com/defenseunicorns/peat/issues/833)
//! for context.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]

pub mod cell;
pub mod command; // Bidirectional command coordination
pub mod composition;
pub mod cot; // Cursor-on-Target translation layer (ADR-020, ADR-028)
pub mod credentials; // Backend-agnostic credential management
pub mod discovery;
pub mod distribution; // AI model distribution (manifests, updates, requirements)
pub mod error;
pub mod event; // Event routing and aggregation (ADR-027)
pub mod ffi; // FFI bindings for native consumers (Issue #258)
pub mod geohash; // Vendored geohash algorithm (supply chain audit)
pub mod hierarchy;
pub mod mesh_integration;
pub mod models;
pub mod network;
pub mod policy; // Generic policy engine for conflict resolution
pub mod qos; // Quality of Service framework (ADR-019)
pub mod security; // Device authentication and PKI (ADR-006)
pub mod storage;
pub mod sync; // Data synchronization abstraction layer
pub mod testing;
pub mod traits;
pub mod transport; // Backend-agnostic transport abstraction for mesh topology // Peat-specific adapters for peat-mesh

pub use error::{Error, Result};

// Facade re-exports: downstream consumers depend on peat-protocol alone.
pub use peat_mesh;
pub use peat_schema;

/// Protocol version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default cell size (nodes per cell)
pub const DEFAULT_CELL_SIZE: usize = 5;

/// Default discovery timeout in seconds
pub const DEFAULT_DISCOVERY_TIMEOUT_SECS: u64 = 60;

/// Default hierarchy depth (node -> cell -> zone -> network)
pub const DEFAULT_HIERARCHY_DEPTH: usize = 4;
