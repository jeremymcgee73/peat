# ADR-076: Android-selected IP bindings and bounded application relay

**Status**: Accepted
**Date**: 2026-08-25
**Authors**: Jeremy McGee
**Relates to**: ADR-030, ADR-046, ADR-059, ADR-062

## Context

Android may keep an isolated Wi-Fi/Ethernet/USB network attached while choosing
another network for Internet traffic. A source address alone does not select an
Android `Network`: each native socket must be associated with that network
before bind. Process-wide binding is unsafe for consumers embedded in a shared
host process. Iroh owns the UDP sockets, and its public API does not accept
consumer-created sockets.

Consumers also need a narrow gateway behavior for application documents that
arrive over local authenticated sync and must be re-originated to one upstream
PEAT peer. This must not become generic IP routing or an open proxy.

## Decision

PEAT FFI accepts a bounded JSON list of concrete numeric IP addresses and CIDR
prefixes. Each entry identifies either a nonzero platform network handle or a
concrete local interface. Handle-backed entries retain one optional default
route per address family. A handle-less local-interface entry is always
non-default, must name a live interface that currently owns the address, and
must carry the matching nonzero scope for IPv6 link-local addresses.
`peat-mesh` converts the validated declarations into exact Iroh `BindOpts`; it
does not enumerate or add host interfaces in this mode.

The Android FFI installs one exclusive, process-local `netwatch` UDP pre-bind
hook for the exact native node lifetime. The hook maps each socket's concrete
local address and IPv6 scope to its declared owner. It calls
`android_setsocknetwork` for a handle-backed owner and skips that call only for
an exact local-interface declaration already validated against the live Android
interface table. Missing mappings, stale interfaces, mismatched scopes,
duplicate ownership, invalid addresses/prefixes/defaults, competing hook
owners, and non-Android use fail closed. Node free drops the hook guard.
Network-change notification asks Iroh to re-evaluate paths; a changed or lost
declaration is handled by consumer stop/free/create.

Selected-binding mode disables n0 hosted relay. Its TCP and DNS sockets are not
inside this UDP hook and therefore cannot truthfully claim selected-network
ownership. This can change only after those socket owners expose an equivalent
pre-connect boundary.

PEAT core also registers the transport-neutral `peat.application.relay.v1`
schema in collection `application-relay`. It contains message/origin/destination
identity, timestamps, maximum/current hops, an ordered unique route, payload
kind, SHA-256 digest, and base64 payload. The schema bounds route length, TTL,
and encoded payload size. FFI submits the document through existing durable,
formation-authenticated direct application delivery; core does not choose
consumer destinations or payload policy.

## Consequences

- Consumers can keep local mesh traffic on one or more explicitly selected
  Wi-Fi, Ethernet, or USB-backed Android networks without changing the host
  process default or naming a particular radio product.
- Local-only bearer owners that are not surfaced as Android `Network` objects
  can contribute a concrete non-default interface binding without weakening
  the ownership requirement for any other socket.
- A consumer must stop, free, and recreate the node when its selected address,
  prefix, Android network handle, local interface, or IPv6 scope changes.
  Reachability-only changes can use the notification entrypoint without
  recreating the node.
- Hosted relay remains unavailable on this selected-binding path until every
  relay-owned TCP and DNS socket has an equivalent pre-connect network hook.
  Direct local discovery and application delivery remain available.
- Application relay is deliberately document-scoped and addressed to an
  authenticated PEAT endpoint. Consumers retain destination, payload, metered
  network, queue, loop, and operator-consent policy.
- The small vendored `netwatch` hook remains an explicit maintenance and
  license-notice obligation until the capability is available from a released
  upstream dependency.

## Security and routing properties

- No process binding, VPN, root, kernel forwarding, packet interception, or
  arbitrary socket proxy is introduced.
- The platform adapter may influence only sockets created by the selected PEAT
  node while its exclusive hook guard is alive.
- Local CRDT convergence remains transitive. Direct application relay is one
  explicitly addressed delivery, not a new general mesh router.
- Delivery authenticates the relay node to the destination under existing
  formation trust. Preserved origin identity in the body is hop-by-hop
  attestation unless a consumer adds an origin signature.
- Consumer adapters must enforce allowed payloads, destinations, metered policy,
  TTL/hops, loop prevention, queue bounds, and operator opt-in.

## Compatibility and licensing

The existing JNI create path remains unchanged. New JNI methods are additive.
The vendored `netwatch` 0.19.1 source is `MIT OR Apache-2.0`; this repository
uses the Apache-2.0 option and retains its license in `vendor/netwatch`.
No Meshrabiya code, LGPL dependency, or radio-vendor assumption is present.

The vendored delta should be replaced by a released upstream hook when one is
available. Until then, it is deliberately small and isolated from protocol
semantics.

## Qualification boundary

Host checks can validate parsing, schema, lifecycle ownership, and compilation.
An Android AAR built with the matching Rust/NDK target and a device test are
required before claiming that socket association ran on hardware.
