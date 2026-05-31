//! Data models for the Peat protocol

pub mod capability;
pub mod cell;
pub mod domain;
pub mod node;
pub mod operator;
pub mod role;
pub mod zone;

// Re-export commonly used types at module level
pub use capability::{Capability, CapabilityExt, CapabilityType};
pub use cell::{CellConfig, CellConfigExt, CellState, CellStateExt};
pub use domain::{DetectionCheck, Domain, DomainSet, SensorType};
pub use node::{HealthStatus, NodeConfig, NodeConfigExt, NodeState, NodeStateExt};
pub use operator::{
    AuthorityLevel, AuthorityLevelExt, BindingType, HumanMachinePair, HumanMachinePairExt,
    Operator, OperatorExt, OperatorRank, OperatorRankExt,
};
pub use role::{CellRole, RoleAssignment, RoleScorer};
pub use zone::{ZoneConfig, ZoneState, ZoneStats};

// Legacy compatibility aliases for the `Platform*` naming kept during the
// pre-ADR-066 migration window. The earlier military-vocabulary aliases were
// removed alongside the Cell/Cohort/Federation/Coalition vocabulary refresh
// (ADR-066, peat#904).
pub use node::NodeConfig as PlatformConfig;
pub use node::NodeState as PlatformState;
