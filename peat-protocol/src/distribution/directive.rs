//! Deployment Directives - ADR-012 / ADR-026
//!
//! This module provides the `DeploymentDirective` type for pushing software
//! artifacts (models, containers, binaries) from C2 to edge nodes.
//!
//! ## Flow
//!
//! ```text
//! ┌─────────────┐                    ┌─────────────┐
//! │     C2      │                    │  Edge Node  │
//! │             │─DeploymentDirective│             │
//! │             │───────────────────▶│             │
//! │             │                    │ fetch blob  │
//! │             │                    │ activate    │
//! │             │◀───────────────────│             │
//! │             │  DeploymentStatus  │ advertise   │
//! └─────────────┘                    └─────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust
//! use peat_protocol::distribution::{
//!     DeploymentDirective, DeploymentScope, ArtifactSpec, DeploymentPriority,
//! };
//!
//! // Create directive for ONNX model deployment
//! let directive = DeploymentDirective::new("yolov8n-deploy-001")
//!     .with_artifact(ArtifactSpec::onnx_model(
//!         "sha256:abc123...",
//!         500_000_000,
//!         vec!["CUDAExecutionProvider".into()],
//!     ))
//!     .with_scope(DeploymentScope::formation("formation-alpha"))
//!     .with_capabilities(vec!["object_detection".into()])
//!     .with_priority(DeploymentPriority::High);
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Deployment directive - command to deploy software to nodes
///
/// This is the primary message type for C2 → Edge software deployment.
/// Nodes matching the scope will fetch the artifact and activate it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentDirective {
    /// Unique directive identifier
    pub directive_id: String,
    /// When the directive was issued
    pub issued_at: DateTime<Utc>,
    /// Node ID of the issuer
    pub issuer_node_id: String,
    /// Formation ID of the issuer (for hierarchy routing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer_formation_id: Option<String>,
    /// Target scope for this deployment
    pub scope: DeploymentScope,
    /// Artifact specification
    pub artifact: ArtifactSpec,
    /// Capabilities this deployment provides
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Runtime-specific configuration
    #[serde(default)]
    pub config: serde_json::Value,
    /// Deployment options
    #[serde(default)]
    pub options: DeploymentOptions,
}

impl DeploymentDirective {
    /// Create a new deployment directive
    pub fn new(directive_id: impl Into<String>) -> Self {
        Self {
            directive_id: directive_id.into(),
            issued_at: Utc::now(),
            issuer_node_id: String::new(),
            issuer_formation_id: None,
            scope: DeploymentScope::Broadcast,
            artifact: ArtifactSpec::default(),
            capabilities: Vec::new(),
            config: serde_json::Value::Null,
            options: DeploymentOptions::default(),
        }
    }

    /// Generate a unique directive ID
    pub fn generate() -> Self {
        Self::new(uuid::Uuid::new_v4().to_string())
    }

    /// Set the issuer node
    pub fn with_issuer(mut self, node_id: impl Into<String>) -> Self {
        self.issuer_node_id = node_id.into();
        self
    }

    /// Set the issuer formation
    pub fn with_formation(mut self, formation_id: impl Into<String>) -> Self {
        self.issuer_formation_id = Some(formation_id.into());
        self
    }

    /// Set the deployment scope
    pub fn with_scope(mut self, scope: DeploymentScope) -> Self {
        self.scope = scope;
        self
    }

    /// Set the artifact
    pub fn with_artifact(mut self, artifact: ArtifactSpec) -> Self {
        self.artifact = artifact;
        self
    }

    /// Add capabilities
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Add a capability
    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.capabilities.push(capability.into());
        self
    }

    /// Set runtime config
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }

    /// Set deployment options
    pub fn with_options(mut self, options: DeploymentOptions) -> Self {
        self.options = options;
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: DeploymentPriority) -> Self {
        self.options.priority = priority;
        self
    }

    /// Check if this directive targets a specific node by ID alone.
    ///
    /// Conservative for `Capability` scope: returns `true` only if the
    /// filter is fully empty (no required capabilities, no hardware
    /// bounds, no custom constraints). Callers that have a candidate
    /// node's [`CapabilityAdvertisement`] should use
    /// [`Self::targets`] for the full match, which evaluates the filter
    /// against the node's advertised capabilities and hardware via
    /// [`CapabilityMatcher`] (peat#773).
    ///
    /// [`CapabilityAdvertisement`]: crate::cot::CapabilityAdvertisement
    /// [`CapabilityMatcher`]: crate::distribution::CapabilityMatcher
    pub fn targets_node(&self, node_id: &str) -> bool {
        match &self.scope {
            DeploymentScope::Broadcast => true,
            DeploymentScope::Formation(fid) => {
                // Would need formation membership lookup
                // For now, return true if same formation
                self.issuer_formation_id.as_deref() == Some(fid)
            }
            DeploymentScope::Nodes(node_ids) => node_ids.iter().any(|n| n == node_id),
            DeploymentScope::Capability(filter) => {
                // Conservative: ID-only callers can't evaluate any real
                // capability constraint, so we admit only the unconstrained
                // filter and reject everything else. Pre-peat#773 this arm
                // returned `filter.required_capabilities.is_empty()`, which
                // wrongly admitted filters that only set hardware bounds or
                // custom k/v constraints. Callers that need to evaluate
                // those constraints should switch to `targets(adv)`.
                filter.is_unconstrained()
            }
        }
    }

    /// Check if this directive targets a specific node given its
    /// [`CapabilityAdvertisement`].
    ///
    /// Full evaluation: identity (`node_id`), formation membership
    /// (`formation_id` on both directive and advert), and capability
    /// matching via [`CapabilityMatcher::matches`]. Use this in place of
    /// [`Self::targets_node`] wherever an advertisement is available — it
    /// supersedes the ID-only fast path and is the entry point peat#773
    /// added.
    ///
    /// # Formation scope: fall-through for unassigned nodes
    ///
    /// For `DeploymentScope::Formation(fid)`, this method matches if
    /// **either**:
    ///
    /// 1. The advert explicitly self-identifies as a member of the
    ///    scoped formation (`adv.formation_id == Some(fid)`); **or**
    /// 2. The advert has no `formation_id` set AND the directive was
    ///    issued by a node in the scoped formation
    ///    (`self.issuer_formation_id == Some(fid)`).
    ///
    /// An advert that explicitly advertises membership in a **different**
    /// formation does NOT fall through — explicit non-membership wins.
    ///
    /// The fall-through exists to support transitional bootstrap flows:
    /// a freshly-deployed node with `formation_id = None` can
    /// receive formation-scoped directives from issuers in that
    /// formation (e.g., the very directive that sets its
    /// `formation_id`). This policy is **asymmetric with**
    /// [`crate::cot::HardwareSpec`]-bound semantics, which treat
    /// missing fields as non-matches. Hardware-claim absence vs.
    /// provisioning-state absence are distinct semantic axes; see
    /// **ADR-064** (`docs/adr/064-deployment-formation-fallthrough.md`)
    /// for the full rationale, alternatives considered, and operational
    /// implications.
    ///
    /// [`CapabilityAdvertisement`]: crate::cot::CapabilityAdvertisement
    /// [`CapabilityMatcher::matches`]: crate::distribution::CapabilityMatcher::matches
    pub fn targets(&self, adv: &crate::cot::CapabilityAdvertisement) -> bool {
        match &self.scope {
            DeploymentScope::Broadcast => true,
            DeploymentScope::Formation(fid) => {
                // ADR-064: explicit-match wins; otherwise fall through to
                // the directive's issuer formation iff the advert hasn't
                // self-identified. An advert that explicitly belongs to a
                // different formation does NOT fall through.
                adv.formation_id.as_deref() == Some(fid)
                    || (adv.formation_id.is_none()
                        && self.issuer_formation_id.as_deref() == Some(fid))
            }
            DeploymentScope::Nodes(node_ids) => node_ids.iter().any(|n| n == &adv.node_id),
            DeploymentScope::Capability(filter) => {
                crate::distribution::CapabilityMatcher::matches(adv, filter)
            }
        }
    }
}

/// Scope for deployment targeting
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeploymentScope {
    /// Broadcast to all capable nodes
    Broadcast,
    /// Target a specific formation
    Formation(String),
    /// Target specific nodes by ID
    Nodes(Vec<String>),
    /// Target nodes matching capability filter
    Capability(CapabilityFilter),
}

impl DeploymentScope {
    /// Create scope for a specific formation
    pub fn formation(formation_id: impl Into<String>) -> Self {
        Self::Formation(formation_id.into())
    }

    /// Create scope for specific nodes
    pub fn nodes(node_ids: Vec<String>) -> Self {
        Self::Nodes(node_ids)
    }

    /// Create scope for capability-based targeting
    pub fn with_capabilities(capabilities: Vec<String>) -> Self {
        Self::Capability(CapabilityFilter {
            required_capabilities: capabilities,
            ..Default::default()
        })
    }
}

/// Filter for capability-based deployment targeting
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityFilter {
    /// Minimum GPU memory in MB
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_gpu_memory_mb: Option<u64>,
    /// Minimum system memory in MB
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_memory_mb: Option<u64>,
    /// Minimum storage in MB
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_storage_mb: Option<u64>,
    /// Required capabilities (e.g., ["cuda", "tensorrt"])
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    /// Custom filters
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, String>,
}

impl CapabilityFilter {
    /// True iff no constraint is set on this filter. Used by
    /// [`DeploymentDirective::targets_node`] to admit the trivial
    /// no-constraint case under ID-only evaluation; any non-empty
    /// constraint requires a full [`CapabilityAdvertisement`]-aware
    /// match via [`DeploymentDirective::targets`] (peat#773).
    ///
    /// [`CapabilityAdvertisement`]: crate::cot::CapabilityAdvertisement
    pub fn is_unconstrained(&self) -> bool {
        self.min_gpu_memory_mb.is_none()
            && self.min_memory_mb.is_none()
            && self.min_storage_mb.is_none()
            && self.required_capabilities.is_empty()
            && self.custom.is_empty()
    }
}

/// Artifact specification for deployment
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtifactSpec {
    /// Blob hash (content-addressed)
    pub blob_hash: String,
    /// Size in bytes
    pub size_bytes: u64,
    /// Artifact type
    pub artifact_type: ArtifactType,
    /// SHA256 hash for verification (if different from blob hash)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Human-readable name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Version string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ArtifactSpec {
    /// Create ONNX model artifact spec
    pub fn onnx_model(
        blob_hash: impl Into<String>,
        size_bytes: u64,
        execution_providers: Vec<String>,
    ) -> Self {
        Self {
            blob_hash: blob_hash.into(),
            size_bytes,
            artifact_type: ArtifactType::OnnxModel {
                execution_providers,
            },
            sha256: None,
            name: None,
            version: None,
        }
    }

    /// Create container artifact spec
    pub fn container(
        blob_hash: impl Into<String>,
        size_bytes: u64,
        runtime: ContainerRuntime,
    ) -> Self {
        Self {
            blob_hash: blob_hash.into(),
            size_bytes,
            artifact_type: ArtifactType::Container {
                runtime,
                ports: Vec::new(),
                env: HashMap::new(),
            },
            sha256: None,
            name: None,
            version: None,
        }
    }

    /// Create native binary artifact spec
    pub fn native_binary(
        blob_hash: impl Into<String>,
        size_bytes: u64,
        arch: impl Into<String>,
    ) -> Self {
        Self {
            blob_hash: blob_hash.into(),
            size_bytes,
            artifact_type: ArtifactType::NativeBinary {
                arch: arch.into(),
                args: Vec::new(),
            },
            sha256: None,
            name: None,
            version: None,
        }
    }

    /// Set name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set version
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set SHA256 hash
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }
}

/// Artifact type (mirrors peat-inference ArtifactType for protocol layer)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactType {
    /// ONNX model for inference
    OnnxModel {
        /// Execution providers in preference order
        #[serde(default)]
        execution_providers: Vec<String>,
    },
    /// Container image
    Container {
        /// Container runtime
        runtime: ContainerRuntime,
        /// Port mappings
        #[serde(default)]
        ports: Vec<PortMapping>,
        /// Environment variables
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// Native executable
    NativeBinary {
        /// Target architecture
        arch: String,
        /// Command-line arguments
        #[serde(default)]
        args: Vec<String>,
    },
    /// Configuration package
    ConfigPackage {
        /// Target extraction path
        target_path: String,
    },
    /// WebAssembly module
    WasmModule {
        /// WASI capabilities
        #[serde(default)]
        wasi_capabilities: Vec<String>,
    },
}

impl Default for ArtifactType {
    fn default() -> Self {
        Self::OnnxModel {
            execution_providers: Vec::new(),
        }
    }
}

/// Container runtime
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    #[default]
    Docker,
    Podman,
    Containerd,
}

/// Port mapping for containers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    /// Container port
    pub container_port: u16,
    /// Host port
    pub host_port: u16,
    /// Protocol (tcp/udp)
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "tcp".to_string()
}

/// Deployment options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentOptions {
    /// Priority level
    #[serde(default)]
    pub priority: DeploymentPriority,
    /// Timeout in seconds (0 = no timeout)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    /// Replace existing deployment with same capabilities
    #[serde(default)]
    pub replace_existing: bool,
    /// Rollback threshold (percentage of nodes that must succeed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_threshold_percent: Option<u32>,
    /// Auto-activate after download
    #[serde(default = "default_true")]
    pub auto_activate: bool,
}

fn default_timeout() -> u32 {
    300 // 5 minutes
}

fn default_true() -> bool {
    true
}

impl Default for DeploymentOptions {
    fn default() -> Self {
        Self {
            priority: DeploymentPriority::Normal,
            timeout_seconds: default_timeout(),
            replace_existing: false,
            rollback_threshold_percent: None,
            auto_activate: true,
        }
    }
}

/// Deployment priority
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentPriority {
    /// Critical - interrupt other operations
    Critical,
    /// High - process soon
    High,
    /// Normal - standard processing
    #[default]
    Normal,
    /// Low - process when idle
    Low,
}

/// Deployment status report from a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStatus {
    /// Directive ID this status is for
    pub directive_id: String,
    /// Reporting node ID
    pub node_id: String,
    /// When this status was reported
    pub reported_at: DateTime<Utc>,
    /// Current state
    pub state: DeploymentState,
    /// Progress percentage (0-100)
    pub progress_percent: u8,
    /// Error message (if state is Failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Instance ID (if state is Active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

impl DeploymentStatus {
    /// Create a new status report
    pub fn new(directive_id: impl Into<String>, node_id: impl Into<String>) -> Self {
        Self {
            directive_id: directive_id.into(),
            node_id: node_id.into(),
            reported_at: Utc::now(),
            state: DeploymentState::Pending,
            progress_percent: 0,
            error_message: None,
            instance_id: None,
        }
    }

    /// Set state to downloading
    pub fn downloading(mut self, progress: u8) -> Self {
        self.state = DeploymentState::Downloading;
        self.progress_percent = progress.min(99);
        self
    }

    /// Set state to activating
    pub fn activating(mut self) -> Self {
        self.state = DeploymentState::Activating;
        self.progress_percent = 100;
        self
    }

    /// Set state to active
    pub fn active(mut self, instance_id: impl Into<String>) -> Self {
        self.state = DeploymentState::Active;
        self.progress_percent = 100;
        self.instance_id = Some(instance_id.into());
        self
    }

    /// Set state to failed
    pub fn failed(mut self, error: impl Into<String>) -> Self {
        self.state = DeploymentState::Failed;
        self.error_message = Some(error.into());
        self
    }

    /// Set state to rolled back
    pub fn rolled_back(mut self) -> Self {
        self.state = DeploymentState::RolledBack;
        self
    }
}

/// Deployment state
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    /// Directive received, waiting to process
    Pending,
    /// Downloading artifact from blob store
    Downloading,
    /// Activating artifact via runtime adapter
    Activating,
    /// Artifact is active and running
    Active,
    /// Deployment failed
    Failed,
    /// Deployment was rolled back
    RolledBack,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directive_creation() {
        let directive = DeploymentDirective::generate()
            .with_issuer("c2-node-1")
            .with_formation("formation-alpha")
            .with_artifact(ArtifactSpec::onnx_model(
                "sha256:abc123",
                500_000_000,
                vec!["CUDAExecutionProvider".into()],
            ))
            .with_capability("object_detection")
            .with_priority(DeploymentPriority::High);

        assert!(!directive.directive_id.is_empty());
        assert_eq!(directive.issuer_node_id, "c2-node-1");
        assert_eq!(directive.capabilities, vec!["object_detection"]);
        assert_eq!(directive.options.priority, DeploymentPriority::High);
    }

    #[test]
    fn test_scope_targeting() {
        // Broadcast targets everyone
        let directive = DeploymentDirective::generate();
        assert!(directive.targets_node("any-node"));

        // Node list targets specific nodes
        let directive = DeploymentDirective::generate().with_scope(DeploymentScope::nodes(vec![
            "node-1".into(),
            "node-2".into(),
        ]));
        assert!(directive.targets_node("node-1"));
        assert!(!directive.targets_node("node-3"));
    }

    /// peat#773 regression pin: the `targets_node(node_id)` fast path
    /// must NOT vacuously accept Capability scope when the filter has
    /// hardware bounds or custom k/v constraints set. Pre-#773 the arm
    /// only checked `required_capabilities.is_empty()` and would have
    /// returned `true` for the filter below. The new
    /// `is_unconstrained()` check rejects every non-trivial filter
    /// from the ID-only fast path.
    #[test]
    fn targets_node_rejects_hardware_only_filter_under_capability_scope() {
        use crate::distribution::CapabilityFilter;
        let directive = DeploymentDirective::generate().with_scope(DeploymentScope::Capability(
            CapabilityFilter {
                min_gpu_memory_mb: Some(8_192),
                ..Default::default()
            },
        ));
        assert!(!directive.targets_node("any-node"));

        // Truly unconstrained filter still passes (back-compat for the
        // pre-#773 callers that built empty Capability filters).
        let unconstrained = DeploymentDirective::generate()
            .with_scope(DeploymentScope::Capability(CapabilityFilter::default()));
        assert!(unconstrained.targets_node("any-node"));
    }

    /// peat#773 regression pin: `targets(adv)` evaluates the full
    /// Capability filter via `CapabilityMatcher`. Covers the integration
    /// surface beyond the unit tests in
    /// `distribution::capability_matcher::tests`.
    #[test]
    fn targets_capability_scope_uses_capability_matcher() {
        use crate::cot::{
            CapabilityAdvertisement, CapabilityInfo, HardwareSpec, OperationalStatus, Position,
        };
        use crate::distribution::CapabilityFilter;

        let directive = DeploymentDirective::generate().with_scope(DeploymentScope::Capability(
            CapabilityFilter {
                required_capabilities: vec!["ELECTRO_OPTICAL".into()],
                min_gpu_memory_mb: Some(4_096),
                ..Default::default()
            },
        ));

        let base = CapabilityAdvertisement::new(
            "node-7".into(),
            "UAV".into(),
            Position::new(35.0, -120.0),
            OperationalStatus::Active,
            1.0,
        );

        // No capabilities + no hardware → fails.
        assert!(!directive.targets(&base));

        // Has EO sensor but no GPU advert → fails on the hardware bound.
        let with_sensor_no_gpu = base.clone().with_capability(CapabilityInfo {
            capability_type: "EO".into(),
            model_name: "Mk1".into(),
            version: "1.0".into(),
            precision: 0.9,
            status: OperationalStatus::Active,
        });
        assert!(!directive.targets(&with_sensor_no_gpu));

        // Has EO sensor + sufficient GPU → matches.
        let fully_fit = with_sensor_no_gpu.with_hardware(HardwareSpec {
            gpu_memory_mb: Some(8_192),
            ..Default::default()
        });
        assert!(directive.targets(&fully_fit));
    }

    /// `targets(adv)` for `Formation` scope prefers the advert's
    /// `formation_id` when set, with a fall-through to the directive's
    /// issuer formation for advertisements that haven't yet been
    /// assigned a formation.
    #[test]
    fn targets_formation_scope_uses_advert_formation_id() {
        use crate::cot::{CapabilityAdvertisement, OperationalStatus, Position};

        let directive = DeploymentDirective::generate()
            .with_formation("formation-alpha")
            .with_scope(DeploymentScope::Formation("formation-alpha".into()));

        // Advert in matching formation → matches.
        let mut adv_in_formation = CapabilityAdvertisement::new(
            "p-1".into(),
            "UAV".into(),
            Position::new(0.0, 0.0),
            OperationalStatus::Active,
            1.0,
        );
        adv_in_formation.formation_id = Some("formation-alpha".into());
        assert!(directive.targets(&adv_in_formation));

        // Advert in different formation → does not match.
        let mut adv_other_formation = adv_in_formation.clone();
        adv_other_formation.formation_id = Some("formation-bravo".into());
        assert!(!directive.targets(&adv_other_formation));

        // Advert with no formation_id → falls through to issuer's formation
        // (which matches in this directive).
        let mut adv_unassigned = adv_in_formation.clone();
        adv_unassigned.formation_id = None;
        assert!(directive.targets(&adv_unassigned));
    }

    #[test]
    fn test_artifact_spec() {
        let spec = ArtifactSpec::onnx_model("sha256:abc", 1000, vec!["CUDA".into()])
            .with_name("YOLOv8n")
            .with_version("1.0.0");

        assert_eq!(spec.blob_hash, "sha256:abc");
        assert_eq!(spec.name, Some("YOLOv8n".to_string()));
        assert!(matches!(spec.artifact_type, ArtifactType::OnnxModel { .. }));
    }

    #[test]
    fn test_deployment_status_transitions() {
        let status = DeploymentStatus::new("directive-1", "node-1");
        assert_eq!(status.state, DeploymentState::Pending);

        let status = status.downloading(50);
        assert_eq!(status.state, DeploymentState::Downloading);
        assert_eq!(status.progress_percent, 50);

        let status = status.activating();
        assert_eq!(status.state, DeploymentState::Activating);

        let status = status.active("instance-123");
        assert_eq!(status.state, DeploymentState::Active);
        assert_eq!(status.instance_id, Some("instance-123".to_string()));
    }

    #[test]
    fn test_serialization() {
        let directive = DeploymentDirective::generate().with_artifact(ArtifactSpec::container(
            "sha256:def456",
            100_000_000,
            ContainerRuntime::Docker,
        ));

        let json = serde_json::to_string_pretty(&directive).unwrap();
        let parsed: DeploymentDirective = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.directive_id, directive.directive_id);
        assert!(matches!(
            parsed.artifact.artifact_type,
            ArtifactType::Container { .. }
        ));
    }
}
