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
    let (id_hex, addr_str) = spec.split_once('@').ok_or_else(|| {
        anyhow!(
            "--peer needs NODE_ID@ADDR; missing `@` in {spec:?}. \
             Example: --peer 21e8…8f90@127.0.0.1:39001"
        )
    })?;
    let raw = hex::decode(id_hex).with_context(|| {
        format!("node id {id_hex:?} is not valid hex — expected 64 hex characters (32 bytes)")
    })?;
    if raw.len() != 32 {
        return Err(anyhow!(
            "node id has {} bytes after hex decode, need 32 (= 64 hex chars); got {} chars in {id_hex:?}",
            raw.len(),
            id_hex.len()
        ));
    }
    let _: SocketAddr = addr_str.parse().with_context(|| {
        format!("address {addr_str:?} is not a valid IP:PORT — e.g. 127.0.0.1:39001")
    })?;
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

/// Translate a transport-bind failure into a one-line, actionable message.
/// Walks the anyhow chain to catch both downcastable `io::Error`s and
/// any cause whose text mentions the underlying errno (e.g. when an
/// upstream crate wraps the kernel error in its own type).
fn explain_bind_error(bind: SocketAddr, err: &anyhow::Error) -> Option<String> {
    use std::io::ErrorKind;
    let port = bind.port();
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                ErrorKind::AddrInUse => {
                    return Some(format!(
                        "cannot bind {bind}: port {port} is already in use.\n       \
                         find the holder with `lsof -i :{port}` (or `ss -ltnup | grep :{port}`), \
                         or pass `--bind ADDR:PORT` with a free port."
                    ));
                }
                ErrorKind::PermissionDenied => {
                    return Some(format!(
                        "cannot bind {bind}: permission denied.\n       \
                         ports below 1024 require elevated privileges — pick a port ≥ 1024."
                    ));
                }
                ErrorKind::AddrNotAvailable => {
                    return Some(format!(
                        "cannot bind {bind}: that IP is not configured on this host.\n       \
                         list local interfaces with `ip -4 -o addr show`, or use 127.0.0.1 / 0.0.0.0."
                    ));
                }
                _ => {}
            }
        }
        let text = cause.to_string();
        if text.contains("Address already in use") || text.contains("(os error 98)") {
            return Some(format!(
                "cannot bind {bind}: port {port} is already in use.\n       \
                 find the holder with `lsof -i :{port}` (or `ss -ltnup | grep :{port}`), \
                 or pass `--bind ADDR:PORT` with a free port."
            ));
        }
        if text.contains("Permission denied") || text.contains("(os error 13)") {
            return Some(format!(
                "cannot bind {bind}: permission denied.\n       \
                 ports below 1024 require elevated privileges — pick a port ≥ 1024."
            ));
        }
        if text.contains("Cannot assign requested address") || text.contains("(os error 99)") {
            return Some(format!(
                "cannot bind {bind}: that IP is not configured on this host.\n       \
                 list local interfaces with `ip -4 -o addr show`, or use 127.0.0.1 / 0.0.0.0."
            ));
        }
    }
    None
}

/// Translate an AutomergeStore::open failure into an actionable message.
/// Common causes: parent directory missing, permission denied, read-only fs.
fn explain_storage_error(path: &std::path::Path, err: &anyhow::Error) -> Option<String> {
    use std::io::ErrorKind;
    let path_disp = path.display();
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                ErrorKind::NotFound => {
                    return Some(format!(
                        "storage path {path_disp} does not exist (and its parent isn't writable).\n       \
                         create the directory first, or omit `--storage` to use a tempdir."
                    ));
                }
                ErrorKind::PermissionDenied => {
                    return Some(format!(
                        "no permission to write to storage path {path_disp}.\n       \
                         check ownership/mode, or pick a path under your home directory."
                    ));
                }
                ErrorKind::ReadOnlyFilesystem => {
                    return Some(format!(
                        "storage path {path_disp} is on a read-only filesystem.\n       \
                         pick a writable path (e.g. /tmp/peat-quickstart or omit `--storage`)."
                    ));
                }
                _ => {}
            }
        }
        let text = cause.to_string();
        if text.contains("No such file or directory") || text.contains("(os error 2)") {
            return Some(format!(
                "storage path {path_disp} does not exist (and its parent isn't writable).\n       \
                 create the directory first, or omit `--storage` to use a tempdir."
            ));
        }
        if text.contains("Permission denied") || text.contains("(os error 13)") {
            return Some(format!(
                "no permission to write to storage path {path_disp}.\n       \
                 check ownership/mode, or pick a path under your home directory."
            ));
        }
        if text.contains("Read-only file system") || text.contains("(os error 30)") {
            return Some(format!(
                "storage path {path_disp} is on a read-only filesystem.\n       \
                 pick a writable path (e.g. /tmp/peat-quickstart or omit `--storage`)."
            ));
        }
    }
    None
}

/// Translate an mDNS startup failure into an actionable message. The mDNS
/// daemon needs to open multicast sockets, which can fail when multicast is
/// disabled (containers, restricted networks), when IPv6 is hard-disabled,
/// or when no usable interface is up.
fn explain_mdns_error(err: &anyhow::Error) -> Option<String> {
    for cause in err.chain() {
        let text = cause.to_string();
        if text.contains("multicast") || text.contains("IP_ADD_MEMBERSHIP") {
            return Some(
                "cannot start mDNS: multicast appears blocked on this host.\n       \
                 enterprise/locked-down networks frequently disable it. drop `--mdns` \
                 and pass `--peer NODE_ID@ADDR` for static peers."
                    .to_string(),
            );
        }
        if text.contains("No such device") || text.contains("(os error 19)") {
            return Some(
                "cannot start mDNS: no usable network interface is up.\n       \
                 check `ip link show` — at least one interface must be UP and configured."
                    .to_string(),
            );
        }
    }
    None
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

    let store = match AutomergeStore::open(&storage_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            if let Some(msg) = explain_storage_error(&storage_path, &e) {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
            return Err(e.context("opening AutomergeStore"));
        }
    };
    let transport = match IrohTransport::from_seed_at_addr(&args.name, args.bind).await {
        Ok(t) => Arc::new(t),
        Err(e) => {
            if let Some(msg) = explain_bind_error(args.bind, &e) {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
            return Err(e);
        }
    };
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
        let mut mdns = match MdnsDiscovery::new(transport.endpoint().clone(), args.name.clone()) {
            Ok(m) => m,
            Err(e) => {
                if let Some(msg) = explain_mdns_error(&e) {
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
                return Err(e.context("creating mDNS discovery"));
            }
        };
        if let Err(e) = mdns.start().await {
            if let Some(msg) = explain_mdns_error(&e) {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
            return Err(e.context("starting mDNS discovery"));
        }
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

#[cfg(test)]
mod explain_error_tests {
    //! Regression tests for the friendly-error helpers. Each variant of each
    //! helper has two cases — one driven by `std::io::Error::kind()` reaching
    //! the chain via `downcast_ref` (the "rich" path) and one driven solely by
    //! the cause's `Display` text containing the errno phrase (the
    //! "text-only" path used when an upstream crate wraps the io::Error in
    //! its own type). Both paths must produce the same actionable message.
    use super::*;
    use std::io::{Error as IoError, ErrorKind};

    fn anyhow_with_io(kind: ErrorKind, msg: &'static str) -> anyhow::Error {
        anyhow::Error::from(IoError::new(kind, msg)).context("simulated upstream context")
    }

    fn anyhow_text_only(s: &str) -> anyhow::Error {
        // String-typed Error so downcast_ref::<io::Error>() misses and the
        // helper has to fall back to text matching.
        anyhow!("simulated upstream context: {s}")
    }

    fn addr(s: &str) -> SocketAddr {
        s.parse().expect("test addr must parse")
    }

    // --- explain_bind_error ---

    #[test]
    fn bind_addr_in_use_via_io_error() {
        let e = anyhow_with_io(ErrorKind::AddrInUse, "Address already in use");
        let msg = explain_bind_error(addr("127.0.0.1:39001"), &e).expect("must explain");
        assert!(
            msg.contains("127.0.0.1:39001"),
            "msg includes bind addr: {msg}"
        );
        assert!(
            msg.contains("39001 is already in use"),
            "msg names the port: {msg}"
        );
        assert!(
            msg.contains("lsof -i :39001"),
            "msg gives the lookup command: {msg}"
        );
    }

    #[test]
    fn bind_addr_in_use_via_text_only() {
        let e = anyhow_text_only("Address already in use (os error 98)");
        let msg = explain_bind_error(addr("127.0.0.1:39001"), &e).expect("must explain");
        assert!(
            msg.contains("39001 is already in use"),
            "text path catches errno 98: {msg}"
        );
    }

    #[test]
    fn bind_permission_denied_via_io_error() {
        let e = anyhow_with_io(ErrorKind::PermissionDenied, "Permission denied");
        let msg = explain_bind_error(addr("127.0.0.1:80"), &e).expect("must explain");
        assert!(msg.contains("permission denied"));
        assert!(
            msg.contains("1024"),
            "msg names the privileged-port threshold: {msg}"
        );
    }

    #[test]
    fn bind_permission_denied_via_text_only() {
        let e = anyhow_text_only("Permission denied (os error 13)");
        let msg = explain_bind_error(addr("127.0.0.1:80"), &e).expect("must explain");
        assert!(msg.contains("permission denied"));
    }

    #[test]
    fn bind_addr_not_available_via_io_error() {
        let e = anyhow_with_io(
            ErrorKind::AddrNotAvailable,
            "Cannot assign requested address",
        );
        let msg = explain_bind_error(addr("10.255.255.1:39001"), &e).expect("must explain");
        assert!(msg.contains("not configured on this host"));
        assert!(
            msg.contains("ip -4 -o addr show"),
            "msg gives the diagnostic command: {msg}"
        );
    }

    #[test]
    fn bind_addr_not_available_via_text_only() {
        let e = anyhow_text_only("Cannot assign requested address (os error 99)");
        let msg = explain_bind_error(addr("10.255.255.1:39001"), &e).expect("must explain");
        assert!(msg.contains("not configured on this host"));
    }

    #[test]
    fn bind_unknown_error_returns_none() {
        let e = anyhow!("something completely unrelated happened");
        assert!(explain_bind_error(addr("127.0.0.1:39001"), &e).is_none());
    }

    // --- explain_storage_error ---

    #[test]
    fn storage_not_found_via_io_error() {
        let e = anyhow_with_io(ErrorKind::NotFound, "No such file or directory");
        let msg = explain_storage_error(std::path::Path::new("/nonexistent/peat"), &e)
            .expect("must explain");
        assert!(msg.contains("/nonexistent/peat"));
        assert!(msg.contains("does not exist"));
        assert!(
            msg.contains("omit `--storage`"),
            "msg points at the tempdir escape: {msg}"
        );
    }

    #[test]
    fn storage_not_found_via_text_only() {
        let e = anyhow_text_only("No such file or directory (os error 2)");
        let msg = explain_storage_error(std::path::Path::new("/nonexistent/peat"), &e)
            .expect("must explain");
        assert!(msg.contains("does not exist"));
    }

    #[test]
    fn storage_permission_denied_via_io_error() {
        let e = anyhow_with_io(ErrorKind::PermissionDenied, "Permission denied");
        let msg =
            explain_storage_error(std::path::Path::new("/root/peat"), &e).expect("must explain");
        assert!(msg.contains("no permission to write"));
        assert!(msg.contains("/root/peat"));
    }

    #[test]
    fn storage_read_only_filesystem_via_text_only() {
        // ReadOnlyFilesystem isn't stable in std on all toolchains we target,
        // so the text-only path is the dominant code path here.
        let e = anyhow_text_only("Read-only file system (os error 30)");
        let msg =
            explain_storage_error(std::path::Path::new("/cdrom/peat"), &e).expect("must explain");
        assert!(msg.contains("read-only filesystem"));
    }

    #[test]
    fn storage_unknown_error_returns_none() {
        let e = anyhow!("disk quota exceeded but we don't handle that yet");
        assert!(explain_storage_error(std::path::Path::new("/tmp/peat"), &e).is_none());
    }

    // --- explain_mdns_error ---

    #[test]
    fn mdns_multicast_blocked() {
        let e = anyhow!("Failed to join multicast group on eth0");
        let msg = explain_mdns_error(&e).expect("must explain");
        assert!(msg.contains("multicast"));
        assert!(
            msg.contains("--peer"),
            "msg recommends the static-peer fallback: {msg}"
        );
    }

    #[test]
    fn mdns_ip_add_membership() {
        let e = anyhow!("setsockopt IP_ADD_MEMBERSHIP failed");
        let msg = explain_mdns_error(&e).expect("must explain");
        assert!(msg.contains("multicast"));
    }

    #[test]
    fn mdns_no_interface() {
        let e = anyhow!("daemon error: No such device (os error 19)");
        let msg = explain_mdns_error(&e).expect("must explain");
        assert!(msg.contains("no usable network interface"));
        assert!(
            msg.contains("ip link show"),
            "msg gives the diagnostic command: {msg}"
        );
    }

    #[test]
    fn mdns_unknown_error_returns_none() {
        let e = anyhow!("the mDNS daemon spontaneously combusted");
        assert!(explain_mdns_error(&e).is_none());
    }

    // --- parse_peer (existing fn; cover its error branches too) ---

    #[test]
    fn parse_peer_missing_at_separator() {
        let err = parse_peer("DEADBEEF.NotASeparator").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("missing `@`"), "msg names the split: {msg}");
        assert!(msg.contains("Example"), "msg shows an example: {msg}");
    }

    #[test]
    fn parse_peer_bad_hex() {
        let err = parse_peer("NOTHEX@127.0.0.1:39001").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not valid hex"),
            "msg names the failure: {msg}"
        );
    }

    #[test]
    fn parse_peer_wrong_hex_length() {
        // 8 hex chars = 4 bytes, not 32
        let err = parse_peer("deadbeef@127.0.0.1:39001").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("64 hex chars") || msg.contains("32"),
            "msg names the expected size: {msg}"
        );
    }

    #[test]
    fn parse_peer_bad_addr() {
        let id = "a".repeat(64);
        let err = parse_peer(&format!("{id}@not-an-addr")).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not a valid IP:PORT") || msg.contains("not-an-addr"),
            "msg names the bad addr: {msg}"
        );
    }
}
