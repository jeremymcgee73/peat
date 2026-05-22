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

> Faster builds are opt-in. CI uses `mold` + `clang` via env vars; if you want the same locally, see [DEVELOPMENT.md → Faster local builds](../../DEVELOPMENT.md). The default toolchain works without it.

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
INFO node 'alpha' ready (id=21e809d6978bd737...8f90 bind=127.0.0.1:39001)
INFO     reach me with: --peer 21e80...8f90@127.0.0.1:39001
INFO [peers=0] me:alpha=100
INFO [peers=0] me:alpha=99
```

Three event prefixes are worth knowing:

- `discovery: ...` — a peer was *configured* (`--peer`) or *found* (`--mdns`). Doesn't yet mean we've reached them.
- `connection: peer X connected` / `... reconnected` / `... lost` — transport-level QUIC connection state.
- `sync: NAME (new)` or `sync: NAME (updated) M → N` — a remote document arrived or changed. **This is the proof state is actually replicating.**

The periodic `[peers=N] me:alpha=99 | bravo=98 | ...` line is a heartbeat snapshot — own state to the left of `me:`, remote nodes after. It fires every 2 seconds regardless of activity.

> The binary defaults to a quiet log filter so the protocol-level noise from the transport and sync coordinator doesn't drown out these events. Set `RUST_LOG=debug` (or any other explicit value) to opt back into the full firehose.

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
INFO connection: peer 71e2acca38df97d6 connected
INFO sync: bravo (new) fuel_minutes=99
INFO [peers=1] me:alpha=98 | bravo=99
INFO sync: bravo (updated) fuel_minutes 99 → 97
INFO [peers=1] me:alpha=97 | bravo=97

# bravo:
INFO discovery: static peer configured (21e809d6978bd737@127.0.0.1:39001)
INFO connection: peer 21e809d6978bd737 connected
INFO sync: alpha (new) fuel_minutes=98
INFO [peers=1] alpha=98 | me:bravo=99
INFO sync: alpha (updated) fuel_minutes 98 → 96
INFO [peers=1] alpha=96 | me:bravo=97
```

That's it — two CRDT-replicated documents propagating over a QUIC mesh. The `sync:` lines are the convergence proof; the `[peers=1]` snapshot shows the merged view of the `nodes` collection.

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

Charlie's log will explicitly show it receiving bravo's state even though bravo isn't a direct peer — that's transitive gossip via alpha:

```
# charlie:
INFO connection: peer 21e809d6978bd737 connected     ← only direct peer is alpha
INFO sync: alpha (new) fuel_minutes=98
INFO sync: bravo (new) fuel_minutes=98               ← arrived via alpha's gossip
INFO [peers=1] alpha=98 | bravo=98 | me:charlie=99
INFO sync: alpha (updated) fuel_minutes 98 → 93
INFO sync: bravo (updated) fuel_minutes 98 → 93
INFO [peers=1] alpha=93 | bravo=93 | me:charlie=96
```

Meanwhile **alpha shows `peers=2`** because it has direct connections to both bravo and charlie:

```
# alpha:
INFO connection: peer 71e2acca38df97d6 connected     ← bravo
INFO sync: bravo (new) fuel_minutes=98
INFO connection: peer 5e69189372ec1316 connected     ← charlie
INFO sync: charlie (new) fuel_minutes=99
INFO [peers=2] me:alpha=93 | bravo=91 | charlie=94
```

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
INFO discovery: mDNS enabled (service _peat-node._tcp.local)
INFO discovery: mDNS found 71e2acca38df97d6
INFO connection: peer 71e2acca38df97d6 connected
INFO discovery: mDNS found 5e69189372ec1316
INFO connection: peer 5e69189372ec1316 connected
INFO sync: bravo (new) fuel_minutes=98
INFO sync: charlie (new) fuel_minutes=99
INFO [peers=2] me:alpha=98 | bravo=98 | charlie=99
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

Same as Scenario 1, but bind to `0.0.0.0` so the Pis can reach you. Set `PEAT_CONNECTION_RECYCLE_SECS=0` to disable the iroh memory-leak workaround that otherwise recycles every QUIC connection at the 60-second mark — for the quickstart's 1-document-per-peer workload that workaround is unnecessary and turns continuous sync into a 4–6 s outage every minute (see [#892](https://github.com/defenseunicorns/peat/issues/892)):

```bash
PEAT_CONNECTION_RECYCLE_SECS=0 ./target/release/peat-quickstart --name alpha --bind 0.0.0.0:39001
```

Copy alpha's NodeId from the log.

### 4.5 Start bravo and charlie on the Pis

**Preflight — kill any leftover quickstart processes on each Pi.** When peers are launched over SSH with `nohup ... &`, they survive the SSH disconnect and don't shut down when you Ctrl-C the local shell. If you've run Scenario 4 before, a `pkill -f peat-quickstart` against your *local* shell does not reach them. Verify before relaunching:

```bash
ssh pi-a 'pgrep -af peat-quickstart || echo CLEAN'
ssh pi-b 'pgrep -af peat-quickstart || echo CLEAN'
```

If you see processes listed, kill them on the Pi: `ssh pi-a 'pkill -9 -f peat-quickstart'`. Confirm `pgrep` returns nothing. **Skipping this step produces a confusing failure mode**: the new SSH launch silently exits with `error: cannot bind 0.0.0.0:39001: port 39001 is already in use.`, alpha connects to the stale peer instead, and sync shows `fuel_minutes=0` from the first heartbeat (no countdown). Symptom and remedy are also in the troubleshooting table at the bottom of this page.

```bash
ssh pi-a 'PEAT_CONNECTION_RECYCLE_SECS=0 /tmp/peat-quickstart --name bravo \
    --bind 0.0.0.0:39001 \
    --peer <ALPHA_NODE_ID>@192.168.1.10:39001'
```

```bash
ssh pi-b 'PEAT_CONNECTION_RECYCLE_SECS=0 /tmp/peat-quickstart --name charlie \
    --bind 0.0.0.0:39001 \
    --peer <ALPHA_NODE_ID>@192.168.1.10:39001'
```

(If your firewall blocks UDP on `39001`, open it on each host.)

Within a few seconds all three terminals show the full 3-node view, and sync stays continuous instead of cycling every minute.

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
| `error: cannot bind <addr>: port N is already in use.` (the binary's own friendly message) | Another process owns that port — most often a previous quickstart that didn't shut down cleanly. Especially common on **Scenario 4** Pis where peers were launched over SSH (`nohup ... &`) and a `pkill -f peat-quickstart` from the local shell didn't reach them, or where the SSH session that launched them was torn down without first stopping the remote process. | The error message itself lists the recovery commands: `lsof -i :39001` (or `ss -ltnup \| grep :39001`) to find the holder, then kill it. If `pkill -f peat-quickstart` doesn't take effect, find the explicit PID via `pgrep -af peat-quickstart` and `kill -9 <PID>`. Or pass `--bind ADDR:PORT` with a free port. **Sanity-check both Pis** with `ssh pi-a 'pgrep -af peat-quickstart'` / `ssh pi-b 'pgrep -af peat-quickstart'` *before* launching a new Scenario 4 run — see §4.5 below. |
| Scenario 4 starts but alpha sees `sync: bravo (new) fuel_minutes=0` (already at zero, no progression) | A **stale `peat-quickstart` process** on bravo / charlie from an earlier run is still alive and its counter ran out hours ago. Alpha connected to that stale peer instead of the fresh one your new SSH launch tried to start — the new launch silently failed with port-in-use. The symptom is sync that looks "stuck" rather than the natural 100 → 0 countdown. | On each Pi: `ssh pi-a 'pkill -9 -f peat-quickstart'` then `ssh pi-a 'pgrep -af peat-quickstart \|\| echo CLEAN'` to confirm. Then relaunch. The §4.5 preflight covers this. |
| Logs show `[peers=0]` indefinitely | Static peer's `node_id` or address is wrong; or a firewall is blocking UDP. | Re-copy the `node_id` from the peer's startup log; verify the address; check UDP is open. |
| `connection: peer X lost — will retry` followed by `connection: peer X reconnected` repeating on a ~60–70 s cadence | The Issue [#435](https://github.com/defenseunicorns/peat/issues/435) connection-recycling workaround disconnects every peer once it's been up for 60 s, to bound an iroh memory-growth pattern observed in early 2025. Set `PEAT_CONNECTION_RECYCLE_SECS=0` to disable for low-traffic workloads (like this quickstart), or a larger value (e.g. `600`) for ~10 min sessions. | The default is `60` s. For demos and any deployment where the workload doesn't churn enough documents to provoke the leak, `0` is safe and gives continuous sync. |
| Single `WARN noq_udp: sendmsg error: ... destination: <stale ip>` shortly after a peer attaches | An advertised candidate address — e.g. a stale DHCP lease — isn't reachable from your host. Loopback, docker bridges, podman/CNI bridges, tailscale CGNAT, and link-local interfaces are filtered automatically (see the `interface filter applied (peat#890)` INFO line at startup); a stray warning here means an interface slipped past the default heuristics. Closed in [#890](https://github.com/defenseunicorns/peat/issues/890). | Cosmetic. Sync still converges. To restrict further, set `PEAT_ADVERTISE_INTERFACES=eno1` (or whatever your LAN-facing iface is) to allowlist exactly the interfaces you want published. To bypass filtering entirely (diagnostics only), set `PEAT_ADVERTISE_ALL_INTERFACES=1`. |
| `mDNS enabled` but never `discovered + connected` | LAN is blocking multicast (enterprise Wi-Fi, some VPNs). | Use `--peer` instead. |
| `connection: peer ... connected` fires but no `sync:` event for several seconds | Sync messages take 1–2 round trips after the QUIC handshake completes. | Normal up to ~5 seconds. If `sync: ... (new)` still hasn't fired after that, run with `RUST_LOG=info,peat_mesh::storage::automerge_sync=debug` to watch batches arrive. |
| `error: linker 'mold' not found` during build | You set the fast-linker env vars but don't have `mold` installed. | Either `apt install mold clang`, or unset `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` / `_RUSTFLAGS` to fall back to the default linker. |
| With `RUST_LOG=debug`, repeating `Stream read error: stream finished early` and `Failed to write doc_key length` | The 5-second QUIC idle timeout closes the sync stream after each successful exchange; the channel reopens on next sync. | Cosmetic. The quickstart's default filter suppresses these — you only see them if you opted into a verbose `RUST_LOG`. |
| Occasional `WARN ... Circuit breaker open for peer` | Transient — happens during connection storms (e.g. a third node joining). | Ignore unless persistent. |
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
