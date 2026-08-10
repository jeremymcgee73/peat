//! Consumer-neutral durable application delivery contract.
//!
//! The transport, persistence, retry, expiry, and authenticated acknowledgement
//! state machine is owned by `peat-mesh`; this module is intentionally a thin
//! public re-export.

pub use peat_mesh::storage::application_delivery::*;
