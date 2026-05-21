//! Peer Discovery Strategies for the Automerge+Iroh Backend
//!
//! This module hosts the transport-agnostic pieces of peer discovery: the
//! [`DiscoveryStrategy`] trait, the [`StaticDiscovery`] TOML-loader strategy,
//! and the [`DiscoveryManager`] aggregator. Each consumer composes whichever
//! strategies are appropriate for its environment.
//!
//! The mDNS and Iroh-relay-based strategies that used to live here were
//! IP-stack-specific and duplicated implementations in `peat-mesh` and
//! `peat-discovery`. They were retired in peat#898 (mDNS consolidation) per
//! `peat/SKILL.md`'s transport-agnosticism rule — IP-mesh-specific
//! discovery code now lives in the `peat-mesh` sibling crate as
//! `peat_mesh::discovery::MdnsDiscovery` / `HybridDiscovery` /
//! `KubernetesDiscovery`.
//!
//! ## Usage
//!
//! ```text
//! use peat_protocol::discovery::peer::*;
//!
//! // Create discovery manager
//! let mut manager = DiscoveryManager::new();
//!
//! // Add static config strategy
//! let static_disc = StaticDiscovery::from_file("peers.toml")?;
//! manager.add_strategy(Box::new(static_disc));
//!
//! // Start discovery
//! manager.start().await?;
//!
//! // Get discovered peers
//! let peers = manager.get_peers().await;
//! ```

use async_trait::async_trait;
use iroh::EndpointId;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

// Re-export PeerInfo from network module
pub use crate::network::peer_config::{PeerConfig, PeerInfo};

/// Discovery event emitted when peers are found or lost
#[derive(Clone, Debug)]
pub enum DiscoveryEvent {
    /// New peer discovered
    PeerFound(PeerInfo),
    /// Peer lost/offline
    PeerLost(EndpointId),
}

/// Trait for discovery strategies
#[async_trait]
pub trait DiscoveryStrategy: Send + Sync {
    /// Start the discovery process
    async fn start(&mut self) -> anyhow::Result<()>;

    /// Get all currently discovered peers
    async fn discovered_peers(&self) -> Vec<PeerInfo>;

    /// Subscribe to discovery events
    fn event_stream(&self) -> mpsc::Receiver<DiscoveryEvent>;
}

/// Static peer configuration from TOML files
///
/// Loads pre-configured peer lists from `peers.toml` files. Useful for:
/// - EMCON (emission control) mode where broadcasting is disabled
/// - Known peer sets in tactical environments
/// - Fallback when mDNS is unavailable
pub struct StaticDiscovery {
    peers: Vec<PeerInfo>,
}

impl StaticDiscovery {
    /// Load peers from a TOML configuration file
    ///
    /// Expected format:
    /// ```toml
    /// [[peers]]
    /// name = "Node Alpha"
    /// node_id = "abc123..."
    /// addresses = ["192.168.100.10:5000"]
    /// relay_url = "https://relay.tactical.mil:3479"
    /// ```
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let config = PeerConfig::from_file(path)?;
        Ok(Self {
            peers: config.peers,
        })
    }

    /// Create from in-memory peer list
    pub fn from_peers(peers: Vec<PeerInfo>) -> Self {
        Self { peers }
    }
}

#[async_trait]
impl DiscoveryStrategy for StaticDiscovery {
    async fn start(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            "Static: Loaded {} peers from configuration",
            self.peers.len()
        );
        Ok(())
    }

    async fn discovered_peers(&self) -> Vec<PeerInfo> {
        self.peers.clone()
    }

    fn event_stream(&self) -> mpsc::Receiver<DiscoveryEvent> {
        // Static peers don't change, so return an empty channel
        let (_, rx) = mpsc::channel(1);
        rx
    }
}

/// Hybrid discovery manager that coordinates multiple strategies
///
/// Aggregates peers from all configured discovery strategies and deduplicates
/// based on EndpointId.
pub struct DiscoveryManager {
    strategies: Vec<Box<dyn DiscoveryStrategy>>,
    all_peers: Arc<RwLock<HashMap<EndpointId, PeerInfo>>>,
}

impl DiscoveryManager {
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
            all_peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a discovery strategy
    pub fn add_strategy(&mut self, strategy: Box<dyn DiscoveryStrategy>) {
        self.strategies.push(strategy);
    }

    /// Start all discovery strategies
    pub async fn start(&mut self) -> anyhow::Result<()> {
        for strategy in &mut self.strategies {
            strategy.start().await?;
        }

        // Start background task to periodically merge peer lists
        self.update_peers().await;

        Ok(())
    }

    /// Update aggregated peer list from all strategies
    ///
    /// This should be called before querying discovered peers to ensure
    /// the latest peers from all strategies are included.
    pub async fn update_peers(&self) {
        let mut all = self.all_peers.write().await;

        for strategy in &self.strategies {
            for peer in strategy.discovered_peers().await {
                // Parse EndpointId from hex string
                if let Ok(endpoint_id) = peer.endpoint_id() {
                    all.insert(endpoint_id, peer);
                }
            }
        }
    }

    /// Get all discovered peers by querying all strategies
    ///
    /// This queries each strategy's cache directly, avoiding redundant aggregation.
    /// Strategies maintain their caches asynchronously, so this is a fast read.
    pub async fn get_peers(&self) -> Vec<PeerInfo> {
        let mut all_peers = HashMap::new();

        for strategy in &self.strategies {
            for peer in strategy.discovered_peers().await {
                // Use EndpointId as key to deduplicate peers across strategies
                if let Ok(endpoint_id) = peer.endpoint_id() {
                    all_peers.insert(endpoint_id, peer);
                }
            }
        }

        all_peers.into_values().collect()
    }

    /// Get all discovered peers (alias for get_peers for backward compatibility)
    pub async fn discovered_peers(&self) -> anyhow::Result<Vec<PeerInfo>> {
        Ok(self.get_peers().await)
    }

    /// Get number of discovered peers
    pub async fn peer_count(&self) -> usize {
        self.get_peers().await.len()
    }
}

impl Default for DiscoveryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_static_discovery() {
        let peer = PeerInfo {
            name: "Test Node".to_string(),
            node_id: "a".repeat(64), // 32 bytes hex
            addresses: vec!["192.168.1.100:5000".to_string()],
            relay_url: None,
        };

        let mut discovery = StaticDiscovery::from_peers(vec![peer.clone()]);
        discovery.start().await.unwrap();

        let peers = discovery.discovered_peers().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "Test Node");
    }

    #[tokio::test]
    async fn test_discovery_manager() {
        let peer1 = PeerInfo {
            name: "Node 1".to_string(),
            node_id: "a".repeat(64),
            addresses: vec!["192.168.1.1:5000".to_string()],
            relay_url: None,
        };

        let peer2 = PeerInfo {
            name: "Node 2".to_string(),
            node_id: "b".repeat(64),
            addresses: vec!["192.168.1.2:5000".to_string()],
            relay_url: None,
        };

        let mut manager = DiscoveryManager::new();
        manager.add_strategy(Box::new(StaticDiscovery::from_peers(vec![peer1, peer2])));

        manager.start().await.unwrap();

        let peers = manager.get_peers().await;
        assert_eq!(peers.len(), 2);
    }

    // `test_mdns_service_registration` was deleted alongside MdnsDiscovery
    // and RelayDiscovery (peat#898). Equivalent mDNS coverage now lives in
    // `peat_mesh::discovery::mdns` tests in the peat-mesh sibling crate.
}
