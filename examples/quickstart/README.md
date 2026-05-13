# peat-quickstart

The smallest runnable Peat node — companion binary for [`docs/guides/QUICKSTART.md`](../../docs/guides/QUICKSTART.md).

One binary, one file (`src/main.rs`), ~150 lines. Reads as a copy-paste starting point for your own integration. Don't run it from this README — follow the [walkthrough](../../docs/guides/QUICKSTART.md), which steps you through 2-node → 3-node → mDNS → cross-compile to Raspberry Pi.

### What it does

- Opens an `AutomergeStore` (CRDT) and an `IrohTransport` (QUIC).
- Wires them via `AutomergeBackend::with_transport` and calls `start_sync()`.
- Connects to static `--peer NODE_ID@ADDR` arguments (with a 5-second reconnect loop).
- Optionally enables `--mdns` for zero-config LAN discovery.
- Writes a `NodeState` document with a decrementing counter every 2 seconds.
- Prints the merged view of the `nodes` collection every 2 seconds.

### Flags

| Flag | Default | Purpose |
|------|---------|---------|
| `--name` | *(required)* | Friendly name; also the doc id, the mDNS instance, and the NodeId seed. |
| `--bind` | `127.0.0.1:9001` | Address to bind. Use `0.0.0.0:PORT` for cross-host. |
| `--peer NODE_ID@ADDR` | — | Static peer. Repeatable. |
| `--mdns` | off | Enable zero-config mDNS on the local LAN. |
| `--storage PATH` | tempdir | Persistence directory. |

Set `RUST_LOG=debug` for verbose transport logs.

### What it leaves out

No formation key (cell admission), no MLS group keys, no enrollment, no typed collections beyond the `nodes` proto. Those layer on top via `peat_protocol::sync::automerge::AutomergeIrohBackend` — see the Developer Guide.
