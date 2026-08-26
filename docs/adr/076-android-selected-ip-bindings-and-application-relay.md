# ADR-076: Android-selected IP bindings and bounded application relay

**Status**: Accepted
**Date**: 2026-08-25
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

PEAT FFI accepts a bounded JSON list of concrete numeric IP addresses, CIDR
prefixes, nonzero platform network handles, and one optional default route per
address family. `peat-mesh` converts those into exact Iroh `BindOpts`; it does
not enumerate or add host interfaces in this mode.

The Android FFI installs one exclusive, process-local `netwatch` UDP pre-bind
hook for the exact native node lifetime. The hook maps the socket's concrete
local address to the declared handle and calls `android_setsocknetwork` before
initial bind and rebind. Missing mappings, duplicate ownership, invalid
addresses/prefixes/defaults, competing hook owners, and non-Android use fail
closed. Node free drops the hook guard. Network-change notification asks Iroh to
re-evaluate paths; a changed declaration is handled by consumer stop/free/create.

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
