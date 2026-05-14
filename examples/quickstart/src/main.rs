//! peat-quickstart — the smallest runnable Peat node.
//!
//! Companion binary for `docs/guides/QUICKSTART.md`. One process = one node.
//! Run two or three copies in separate terminals (or on separate Pis) and watch
//! state replicate.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use peat_protocol::discovery::peer::{DiscoveryStrategy, MdnsDiscovery};
use peat_protocol::network::{EndpointId, IrohTransport, PeerInfo};
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

fn short_id(id: &EndpointId) -> String {
    let s = hex::encode(id.as_bytes());
    s[..16].to_string()
}

#[tokio::main]
async fn main() -> Result<()> {
    // Default filter prioritizes legibility over diagnostic detail. The transport
    // (Iroh QUIC) and the sync coordinator both emit chatty WARN/ERROR lines tied
    // to the tactical 5-second idle timeout — the streams close after every
    // successful exchange, the reconnect loop re-opens them, sync still converges,
    // but the noise drowns out the actual scan output.
    // Set `RUST_LOG=debug` (or any explicit value) to opt back into the firehose.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "warn,peat_quickstart=info,\
                 peat_mesh::storage::sync_channel=error,\
                 peat_mesh::storage::automerge_sync=error,\
                 iroh::socket::remote_map::remote_state=off"
                    .into()
            }),
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
        "node '{}' ready (id={} bind={})",
        args.name, node_id_hex, args.bind
    );
    info!("    reach me with: --peer {}@{}", node_id_hex, args.bind);

    let mut static_peers: Vec<PeerInfo> = Vec::new();
    for spec in &args.peer {
        match parse_peer(spec) {
            Ok(peer) => static_peers.push(peer),
            Err(e) => warn!("bad --peer {:?}: {}", spec, e),
        }
    }
    if !static_peers.is_empty() {
        for peer in &static_peers {
            info!(
                "discovery: static peer configured ({}@{})",
                &peer.node_id[..16],
                peer.addresses.first().map(String::as_str).unwrap_or("?")
            );
        }
        let transport_for_static = Arc::clone(&transport);
        let peers = static_peers.clone();
        tokio::spawn(async move {
            // Idempotent dial loop. The scan loop is responsible for emitting
            // connected/lost/reconnected — this task just keeps trying.
            loop {
                for peer in &peers {
                    if let Err(e) = transport_for_static.connect_peer(peer).await {
                        tracing::debug!("dial {} failed: {}", &peer.node_id[..16], e);
                    }
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }

    if args.mdns {
        let mut mdns = MdnsDiscovery::new(transport.endpoint().clone(), args.name.clone())?;
        mdns.start().await?;
        info!("discovery: mDNS enabled (service _peat-node._tcp.local)");

        let transport_for_mdns = Arc::clone(&transport);
        let self_id = node_id_hex.clone();
        tokio::spawn(async move {
            let mut discovered_logged: HashSet<String> = HashSet::new();
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                for peer in mdns.discovered_peers().await {
                    if peer.node_id == self_id {
                        continue;
                    }
                    let short = &peer.node_id[..16];
                    if discovered_logged.insert(peer.node_id.clone()) {
                        info!("discovery: mDNS found {}", short);
                    }
                    // Idempotent dial; connected/lost events come from scan loop.
                    if let Err(e) = transport_for_mdns.connect_peer(&peer).await {
                        tracing::debug!("mDNS dial {} failed: {}", short, e);
                    }
                }
            }
        });
    }

    let nodes: Arc<dyn TypedCollection<NodeState>> = backend.typed_collection("nodes");

    // Writer: decrement own counter every 2s and upsert.
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

    // Scan loop: diff peer set + remote doc state against previous tick, emit
    // explicit connection/sync events. Print a periodic state snapshot so the
    // user always sees "the loop is alive" between events.
    let self_name = args.name.clone();
    let mut last_connected: HashSet<EndpointId> = HashSet::new();
    let mut ever_seen: HashSet<EndpointId> = HashSet::new();
    let mut last_remote_state: HashMap<String, u32> = HashMap::new();

    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;

        // --- Connection events ---
        let current: HashSet<EndpointId> = transport.connected_peers().into_iter().collect();
        for peer in current.difference(&last_connected) {
            if ever_seen.insert(*peer) {
                info!("connection: peer {} connected", short_id(peer));
            } else {
                info!("connection: peer {} reconnected", short_id(peer));
            }
        }
        for peer in last_connected.difference(&current) {
            info!("connection: peer {} lost — will retry", short_id(peer));
        }
        last_connected = current;

        // --- Sync events (remote docs only — skip self) ---
        match nodes.scan() {
            Ok(all) => {
                let mut summary: Vec<String> = Vec::with_capacity(all.len());
                for (id, state) in &all {
                    let label = if id == &self_name {
                        format!("me:{}={}", id, state.fuel_minutes)
                    } else {
                        match last_remote_state.get(id) {
                            None => info!("sync: {} (new) fuel_minutes={}", id, state.fuel_minutes),
                            Some(prev) if *prev != state.fuel_minutes => info!(
                                "sync: {} (updated) fuel_minutes {} → {}",
                                id, prev, state.fuel_minutes
                            ),
                            Some(_) => {}
                        }
                        last_remote_state.insert(id.clone(), state.fuel_minutes);
                        format!("{}={}", id, state.fuel_minutes)
                    };
                    summary.push(label);
                }
                info!("[peers={}] {}", last_connected.len(), summary.join(" | "));
            }
            Err(e) => warn!("scan failed: {}", e),
        }
    }
}
