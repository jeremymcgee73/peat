# Quickstart

This guide takes you from `git clone` to a 3-node Peat mesh syncing state across two Raspberry Pis and your laptop. Everything here uses one tiny example crate — [`examples/quickstart`](../../examples/quickstart) — that you can read in one sitting.

You will go through four scenarios in order:

1. **Two nodes, one host, static peers** — fastest way to see sync work.
2. **Three nodes, one host, static peers** — same pattern, demonstrates transitive gossip.
3. **Three nodes, mDNS discovery** — zero-config: drop `--peer` flags entirely.
4. **Three nodes across two Pis + a laptop** — cross-compile, scp, run.

Each scenario builds on the previous one. The same binary handles all four.

**Time:** ~20 minutes if you don't have a Pi; ~45 minutes including the cross-compile.

---

## What you'll need

- **Rust 1.70+** — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **`protoc`** — `apt install protobuf-compiler` (Debian/Ubuntu) or `brew install protobuf` (macOS)
- **For Pi deployment:** `cross` (`cargo install cross`), Docker, and SSH access to one or two Raspberry Pis running 64-bit Linux (Pi 4 or 5 recommended).

---

## Build the binary

```bash
git clone https://github.com/defenseunicorns/peat.git
cd peat
cargo build -p peat-quickstart --release
```

The binary lands at `target/release/peat-quickstart`. The first build pulls the workspace and takes several minutes; subsequent rebuilds are seconds.

Quick sanity check:

```bash
./target/release/peat-quickstart --help
```

You should see flags for `--name`, `--bind`, `--peer`, `--mdns`, and `--storage`.

---

## Scenario 1 — Two nodes, static peers

### Terminal 1: start the first node

```bash
./target/release/peat-quickstart --name alpha --bind 127.0.0.1:39001
```

The node prints something like:

```
INFO Node 'alpha' ready: node_id=21e809d6978bd73751984af8247c5ca0e4f6d660cd15fbd461b881accd428f90 bind=127.0.0.1:39001
INFO     Other nodes can reach me with: --peer 21e80...8f90@127.0.0.1:39001
INFO [peers=0] nodes: [alpha=100]
INFO [peers=0] nodes: [alpha=99]
```

`alpha` is writing one document per 2 seconds with a counter that decrements from 100. Nothing else is on the mesh yet, so it only sees its own state.

**Copy the `node_id` from the output** — you need it for the next step.

> The NodeId is deterministic from `--name`: the same `--name alpha` always yields the same NodeId, so once you've copied it once you don't need to re-copy it across restarts.

### Terminal 2: start the second node, point it at the first

```bash
./target/release/peat-quickstart \
    --name bravo \
    --bind 127.0.0.1:39002 \
    --peer <ALPHA_NODE_ID>@127.0.0.1:39001
```

Within a few seconds both terminals start showing each other:

```
# alpha:
INFO [peers=1] nodes: [alpha=92, bravo=100]
INFO [peers=1] nodes: [alpha=91, bravo=99]

# bravo:
INFO Static peer 21e809d6978bd737: (re)connected
INFO [peers=1] nodes: [alpha=92, bravo=100]
INFO [peers=1] nodes: [alpha=91, bravo=99]
```

That's it — two CRDT-replicated documents propagating over a QUIC mesh. `peers=1` is the count of live transport connections; `nodes:` is the merged view of the `nodes` collection.

**What just happened:**

- Each node opened an Automerge store (under a tempdir) and an Iroh QUIC transport bound to its address.
- Bravo's `--peer` arg told its transport to dial alpha. Once connected, both sides start sync automatically.
- Every 2 seconds each node writes a `NodeState` document keyed by its own name; every 2 seconds each node scans the whole `nodes` collection and prints what it sees.
- The quickstart binary also retries `connect_peer` every 5 seconds, so if you Ctrl-C alpha and restart it, bravo reconnects without intervention.

Stop both nodes with Ctrl-C before moving on.

---

## Scenario 2 — Three nodes, static peers

The 3-node case adds one piece of new behavior: **transitive sync via gossip**. Bravo and charlie never directly connect to each other, but they both see each other's state via alpha.

### Start alpha and bravo as before

Same two terminals as Scenario 1.

### Terminal 3: start charlie, also pointed at alpha

```bash
./target/release/peat-quickstart \
    --name charlie \
    --bind 127.0.0.1:39003 \
    --peer <ALPHA_NODE_ID>@127.0.0.1:39001
```

After ~10 seconds you should see all three names in every terminal:

```
# alpha:
INFO [peers=2] nodes: [alpha=80, bravo=85, charlie=97]

# bravo:
INFO [peers=1] nodes: [alpha=80, bravo=85, charlie=97]

# charlie:
INFO [peers=1] nodes: [alpha=80, bravo=85, charlie=97]
```

Notice that **alpha has `peers=2`** (one connection to bravo, one to charlie) but **bravo and charlie each have `peers=1`** — they're connected only to alpha. Yet both still see each other's documents. Alpha is gossiping the merged CRDT state.

You can make the mesh fully-connected by giving charlie both alpha and bravo as static peers:

```bash
./target/release/peat-quickstart \
    --name charlie --bind 127.0.0.1:39003 \
    --peer <ALPHA_ID>@127.0.0.1:39001 \
    --peer <BRAVO_ID>@127.0.0.1:39002
```

In the 3-node case this is mostly redundant, but it's how you'd add a redundant path in a 4+ node deployment.

---

## Scenario 3 — Three nodes, mDNS discovery (no static config)

Static peers are fine when you know everyone's address up front. On a normal LAN you can skip the bookkeeping with mDNS — every node advertises itself as `_peat-node._tcp.local` and discovers everyone else automatically.

Open three terminals and run, in any order:

```bash
./target/release/peat-quickstart --name alpha   --bind 127.0.0.1:39001 --mdns
./target/release/peat-quickstart --name bravo   --bind 127.0.0.1:39002 --mdns
./target/release/peat-quickstart --name charlie --bind 127.0.0.1:39003 --mdns
```

Within ~5 seconds each terminal logs:

```
INFO mDNS enabled (service _peat-node._tcp.local)
INFO mDNS: discovered + connected to <other-node-id>
INFO [peers=2] nodes: [alpha=..., bravo=..., charlie=...]
```

No `--peer` flags, no NodeId copy-paste — the nodes find each other.

**When mDNS won't work:**

- Enterprise Wi-Fi networks that block multicast (very common). Fall back to static `--peer` flags.
- Across subnets / VPNs. mDNS is link-local.
- Air-gapped tactical networks where you've turned multicast off by policy.

In all of those, static peer config is the right answer — and it's the same binary either way.

You can also combine the two: `--mdns --peer NODE_ID@ADDR` uses mDNS where it works and a fixed seed peer as a fallback.

---

## Scenario 4 — Three nodes across two Raspberry Pis + a laptop

This is where Peat earns its keep: same code, real hardware, real network.

Setup we'll build:

```
   laptop (alpha)            pi-a (bravo)          pi-b (charlie)
     192.168.1.10   ◀──────▶   192.168.1.20  ◀──▶  192.168.1.21
       :39001                    :39001                :39001
```

All three on the same LAN. Replace IP addresses with your actual ones.

### 4.1 Install `cross`

```bash
cargo install cross
docker info   # cross needs Docker (or Podman with docker CLI compat)
```

This repo's [`Cross.toml`](../../Cross.toml) already configures `aarch64-unknown-linux-gnu` — the target triple for 64-bit Raspberry Pi OS (Pi 4 / Pi 5). No edits required.

### 4.2 Cross-compile for the Pi

```bash
cross build -p peat-quickstart --release --target aarch64-unknown-linux-gnu
```

The first run pulls and provisions a Docker container with `protoc`, `libdbus-1-dev`, and the aarch64 toolchain — slow. After that it's a normal `cargo build`.

Binary lands at `target/aarch64-unknown-linux-gnu/release/peat-quickstart`.

### 4.3 Deploy to each Pi

```bash
scp target/aarch64-unknown-linux-gnu/release/peat-quickstart  pi-a:/tmp/peat-quickstart
scp target/aarch64-unknown-linux-gnu/release/peat-quickstart  pi-b:/tmp/peat-quickstart
```

The binary is ~29 MB and dynamically links to `libc`/`libgcc_s` only — no additional system packages required on a standard Raspberry Pi OS or Ubuntu 24.04 aarch64 image.

### 4.4 Start alpha on the laptop

Same as Scenario 1, but bind to `0.0.0.0` so the Pis can reach you:

```bash
./target/release/peat-quickstart --name alpha --bind 0.0.0.0:39001
```

Copy alpha's NodeId from the log.

### 4.5 Start bravo and charlie on the Pis

```bash
ssh pi-a '/tmp/peat-quickstart --name bravo \
    --bind 0.0.0.0:39001 \
    --peer <ALPHA_NODE_ID>@192.168.1.10:39001'
```

```bash
ssh pi-b '/tmp/peat-quickstart --name charlie \
    --bind 0.0.0.0:39001 \
    --peer <ALPHA_NODE_ID>@192.168.1.10:39001'
```

(If your firewall blocks UDP on `39001`, open it on each host.)

Within a few seconds all three terminals show the full 3-node view.

### 4.6 Try mDNS across the Pis

If your LAN allows multicast, you can drop the `--peer` flags entirely:

```bash
# laptop
./target/release/peat-quickstart --name alpha --bind 0.0.0.0:39001 --mdns

# pi-a
ssh pi-a '/tmp/peat-quickstart --name bravo --bind 0.0.0.0:39001 --mdns'

# pi-b
ssh pi-b '/tmp/peat-quickstart --name charlie --bind 0.0.0.0:39001 --mdns'
```

Same result, zero static configuration. Home/office Wi-Fi usually works; enterprise often does not.

---

## What to read next

You now have a working mesh and a runnable starting point — copy `examples/quickstart/src/main.rs` into your own project and replace the periodic `NodeState` writer with whatever your app actually produces.

- [Developer Guide §4 — Core Concepts](developer/DEVELOPER_GUIDE.md#4-core-concepts) for cells, capabilities, hierarchy.
- [Developer Guide §5 — Crate Reference](developer/DEVELOPER_GUIDE.md#5-crate-reference) for the public API map.
- [README → Integrate Peat](../../README.md#integrate-peat) for the shallow/medium/deep integration depths.
- [ADR-011 — Automerge + Iroh backend](../adr/011-ditto-vs-automerge-iroh.md) for the design rationale behind the transport and CRDT choice.

---

## Troubleshooting

| Symptom | Likely cause | What to do |
|---------|--------------|------------|
| `Address already in use` | Another process owns that port (often a previous quickstart that didn't shut down). | `lsof -i :39001` and kill it, or pick a different port. |
| Logs show `[peers=0]` indefinitely | Static peer's `node_id` or address is wrong; or a firewall is blocking UDP. | Re-copy the `node_id` from the peer's startup log; verify the address; check UDP is open. |
| Logs alternate `[peers=1]` ↔ `[peers=0]` with `Static peer ...: (re)connected` lines | Iroh's QUIC idle timeout dropped the connection; the 5-second reconnect loop restored it. | Normal under low-traffic conditions — values still converge. |
| `mDNS enabled` but never `discovered + connected` | LAN is blocking multicast (enterprise Wi-Fi, some VPNs). | Use `--peer` instead. |
| Occasional `WARN Failed to sync ... doc_key length`, `Sync cooldown active`, or `Circuit breaker open for peer` | Transient — happens during connection storms (e.g. a third node joining) or when the reconnect retries during a sync. | Ignore unless persistent. Convergence still happens. |
| `cross build` fails with `protoc` errors | Old Docker image cached. | `cross clean` then rebuild. |
| Pi binary won't start: `Exec format error` | Wrong target triple (you built for the laptop, not the Pi). | Re-run `cross build --target aarch64-unknown-linux-gnu`. |
| Pi binary starts but `[peers=0]` from the LAN | Pi firewall (ufw) is blocking UDP on the bind port. | `sudo ufw allow 39001/udp` (or your chosen port). |

---

## A note on this quickstart's scope

This binary is deliberately minimal:

- One collection (`nodes`) with one document per node.
- Periodic counter as the only state, just to make sync visible.
- Static peers + mDNS — no formation key, no MLS group keys, no enrollment.

That's enough to see the protocol's core property — multi-transport eventual consistency — without the surrounding security machinery. For production deployments you'll want formation keys (cell admission), MLS for forward secrecy, and persistent on-disk storage. Those are layered on top via `peat_protocol::sync::automerge::AutomergeIrohBackend`; see the Developer Guide for the typed-collection and discovery-strategy APIs at the higher tier.
