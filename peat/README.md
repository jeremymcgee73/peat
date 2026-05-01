<p align="center">
  <img src="https://raw.githubusercontent.com/defenseunicorns/peat/main/assets/peat-wordmark.png" alt="PEAT" width="420">
</p>

# peat

[![Crates.io](https://img.shields.io/crates/v/peat.svg)](https://crates.io/crates/peat)
[![Docs.rs](https://docs.rs/peat/badge.svg)](https://docs.rs/peat)

> **Reserved placeholder.** This crate name is reserved for the future
> top-level facade crate of the Peat Protocol ecosystem. It does not yet
> expose any functionality.

## What this is

The Peat Protocol ships today as a set of focused per-component crates.
This `peat` crate exists as a reserved name on crates.io so that, when the
project is ready to publish a curated facade, the canonical name is
already in the right hands.

## What to depend on today

Depend on the per-component crates directly:

| Crate                                                     | Purpose                                       |
| --------------------------------------------------------- | --------------------------------------------- |
| [`peat-schema`](https://crates.io/crates/peat-schema)     | Wire-format (Protobuf) message definitions    |
| [`peat-protocol`](https://crates.io/crates/peat-protocol) | Coordination protocol semantics               |
| [`peat-mesh`](https://crates.io/crates/peat-mesh)         | Automerge + Iroh mesh transport               |
| [`peat-btle`](https://crates.io/crates/peat-btle)         | Bluetooth LE transport for tactical devices   |

`peat-schema` and `peat-protocol` are released from the
[defenseunicorns/peat](https://github.com/defenseunicorns/peat) workspace.
`peat-mesh` and `peat-btle` are released independently from
[defenseunicorns/peat-mesh](https://github.com/defenseunicorns/peat-mesh)
and [defenseunicorns/peat-btle](https://github.com/defenseunicorns/peat-btle)
on their own cadences.

## License

Apache-2.0. See [LICENSE](https://github.com/defenseunicorns/peat/blob/main/LICENSE).
