//! peat-quickstart — the smallest runnable Peat node.
//!
//! Companion binary for `docs/guides/QUICKSTART.md`. One process = one node.
//! Run two or three copies in separate terminals (or on separate Pis) and watch
//! state replicate.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use peat_protocol::discovery::peer::{DiscoveryStrategy, MdnsDiscovery};
use peat_protocol::network::{IrohTransport, PeerInfo};
use peat_protocol::storage::capabilities::{CrdtCapable, SyncCapable, TypedCollection};
use peat_protocol::storage::{AutomergeBackend, AutomergeStore};
use peat_schema::node::v1::NodeState;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "peat-quickstart",
    about = "Minimal Peat node — see docs/guides/QUICKSTART.md"
)]
struct Args {
    /// Friendly name. Used as the doc id, the mDNS instance name, and as the
    /// NodeId seed — so the same `--name` always yields the same NodeId.
    #[arg(long)]
    name: String,

    /// Address to bind. Use 127.0.0.1:900X on a single host, 0.0.0.0:9000 to
    /// expose on a LAN.
    #[arg(long, default_value = "127.0.0.1:9001")]
    bind: SocketAddr,

    /// Static peer in the form NODE_ID_HEX@ADDR. Repeatable.
    ///
    /// Example: --peer abc123...@127.0.0.1:9001
    #[arg(long)]
    peer: Vec<String>,

    /// Enable mDNS discovery on the local network. Combine with `--peer` for
    /// belt-and-suspenders.
    #[arg(long)]
    mdns: bool,

    /// Persistence directory. Defaults to a tempdir cleaned up at exit.
    #[arg(long)]
    storage: Option<PathBuf>,
}

fn parse_peer(spec: &str) -> Result<PeerInfo> {
    let (id_hex, addr_str) = spec
        .split_once('@')
        .ok_or_else(|| anyhow!("--peer must look like NODE_ID@ADDR (got {spec:?})"))?;
    let raw = hex::decode(id_hex).context("NODE_ID is not valid hex")?;
    if raw.len() != 32 {
        return Err(anyhow!(
            "NODE_ID is {} bytes after hex decode, expected 32",
            raw.len()
        ));
    }
    let _: SocketAddr = addr_str.parse().context("invalid socket address")?;
    Ok(PeerInfo {
        name: format!("peer-{}", &id_hex[..8]),
        node_id: id_hex.to_string(),
        addresses: vec![addr_str.to_string()],
        relay_url: None,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,peat_quickstart=info".into()),
        )
        .init();

    let args = Args::parse();

    let (storage_path, _tempdir_guard) = match &args.storage {
        Some(p) => (p.clone(), None),
        None => {
            let td = tempfile::tempdir()?;
            (td.path().to_path_buf(), Some(td))
        }
    };

    let store = Arc::new(AutomergeStore::open(&storage_path).context("opening AutomergeStore")?);
    let transport = Arc::new(IrohTransport::from_seed_at_addr(&args.name, args.bind).await?);
    let backend = AutomergeBackend::with_transport(Arc::clone(&store), Arc::clone(&transport));
    backend.start_sync().context("start_sync")?;

    let node_id_hex = hex::encode(transport.endpoint_id().as_bytes());
    info!(
        "Node '{}' ready: node_id={} bind={}",
        args.name, node_id_hex, args.bind
    );
    info!(
        "    Other nodes can reach me with: --peer {}@{}",
        node_id_hex, args.bind
    );

    let mut static_peers: Vec<PeerInfo> = Vec::new();
    for spec in &args.peer {
        match parse_peer(spec) {
            Ok(peer) => static_peers.push(peer),
            Err(e) => warn!("Bad --peer {:?}: {}", spec, e),
        }
    }
    if !static_peers.is_empty() {
        let transport_for_static = Arc::clone(&transport);
        let peers = static_peers.clone();
        tokio::spawn(async move {
            loop {
                for peer in &peers {
                    let short = &peer.node_id[..16];
                    match transport_for_static.connect_peer(peer).await {
                        Ok(Some(_)) => info!("Static peer {}: (re)connected", short),
                        Ok(None) => {
                            tracing::debug!("Static peer {}: connection already live", short)
                        }
                        Err(e) => warn!("Static peer {} unreachable: {}", short, e),
                    }
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }

    if args.mdns {
        let mut mdns = MdnsDiscovery::new(transport.endpoint().clone(), args.name.clone())?;
        mdns.start().await?;
        info!("mDNS enabled (service _peat-node._tcp.local)");

        let transport_for_mdns = Arc::clone(&transport);
        let self_id = node_id_hex.clone();
        tokio::spawn(async move {
            // Track which peers we've announced (so we don't spam the log on every
            // reconnect attempt) but still call connect_peer each tick — the transport
            // returns Ok(None) for already-live connections, so the call is idempotent.
            let mut announced: HashSet<String> = HashSet::new();
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                for peer in mdns.discovered_peers().await {
                    if peer.node_id == self_id {
                        continue;
                    }
                    let short = &peer.node_id[..16];
                    match transport_for_mdns.connect_peer(&peer).await {
                        Ok(Some(_)) => {
                            if announced.insert(peer.node_id.clone()) {
                                info!("mDNS: discovered + connected to {}", short);
                            } else {
                                info!("mDNS: reconnected to {}", short);
                            }
                        }
                        Ok(None) => {
                            tracing::debug!("mDNS: peer {} already connected", short)
                        }
                        Err(e) => warn!("mDNS: connect to {} failed: {}", short, e),
                    }
                }
            }
        });
    }

    let nodes: Arc<dyn TypedCollection<NodeState>> = backend.typed_collection("nodes");

    {
        let nodes = Arc::clone(&nodes);
        let name = args.name.clone();
        tokio::spawn(async move {
            let mut counter: u32 = 100;
            loop {
                let state = NodeState {
                    fuel_minutes: counter,
                    cell_id: Some(name.clone()),
                    ..Default::default()
                };
                if let Err(e) = nodes.upsert(&name, &state) {
                    warn!("upsert failed: {}", e);
                }
                counter = counter.saturating_sub(1);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let peer_count = transport.peer_count();
        match nodes.scan() {
            Ok(all) => {
                let summary: Vec<String> = all
                    .iter()
                    .map(|(id, s)| format!("{}={}", id, s.fuel_minutes))
                    .collect();
                info!("[peers={}] nodes: [{}]", peer_count, summary.join(", "));
            }
            Err(e) => warn!("scan failed: {}", e),
        }
    }
}
