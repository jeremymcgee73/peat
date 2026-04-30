//! # TAK Transport Adapter
//!
//! Provides bidirectional CoT message transport between Peat and TAK ecosystem.
//! Supports TAK Server (TCP/SSL) and Mesh SA (UDP multicast) modes.
//!
//! ## Architecture (ADR-029)
//!
//! ```text
//! ┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
//! │  Peat Protocol  │───▶│  TakTransport   │───▶│  TAK Server/    │
//! │                 │    │                 │    │  Mesh SA        │
//! │  CotEncoder     │    │  DIL Queue      │    │                 │
//! │  CotEvent       │    │  Reconnection   │    │  ATAK/WinTAK    │
//! └─────────────────┘    └─────────────────┘    └─────────────────┘
//! ```
//!
//! ## Features
//!
//! - **TAK Server Mode**: TCP/SSL connection to TAK Server (ports 8087/8089)
//! - **Mesh SA Mode**: UDP multicast for local SA sharing
//! - **DIL Resilience**: Priority-aware message queuing during disconnections
//! - **Protobuf Support**: TAK Protocol v1 for 3-5x bandwidth reduction
//! - **Certificate Auth**: Client certificate authentication for TAK Server

mod config;
mod error;
mod metrics;
mod queue;
mod reconnect;
mod traits;
// ADR-059 Slice 1.5: Cursor-on-Target [`Translator`] impl wired against
// the peat-mesh Slice 1 trait. Codec-only — does not own the radio
// (that stays in the `server` / `mesh` transports). Gated behind the
// `mesh-translator` Cargo feature so consumers without peat-mesh in
// their dep graph (radio-only TAK clients) can still use this crate.
// See `cot_translator.rs` module docs for trait-stability findings.
//
// [`Translator`]: peat_mesh::transport::Translator
#[cfg(feature = "mesh-translator")]
mod cot_translator;

pub mod bridge;
pub mod mesh;
pub mod server;

// Re-export main types
pub use config::{
    PriorityQueueLimits, ProtocolConfig, QueueConfig, ReconnectPolicy, TakCredentials, TakIdentity,
    TakProtocolVersion, TakTransportConfig, TakTransportMode, XmlEncodingOptions,
};
pub use error::TakError;
pub use metrics::{QueueDepthMetrics, TakMetrics};
pub use queue::TakMessageQueue;
pub use reconnect::ReconnectionManager;
pub use traits::{CotEventStream, CotFilter, TakTransport};

#[cfg(feature = "mesh-translator")]
pub use cot_translator::{CotTranslator, CotTranslatorConfig};
pub use mesh::MeshSaTransport;
pub use server::TakServerTransport;
