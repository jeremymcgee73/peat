# Peat Ecosystem SKILL.md

This file is the top-level context document for Claude Code sessions workingacross the Peat ecosystem. Always read this file before touching any Peat repo.Then read the per-repo SKILL.md for domain-specific rules.

-----

## What is Peat

Peat is an interoperability-first mesh registry sync platform built for
heterogeneous autonomous systems in defense and edge environments. Peat’s
core value proposition is **interoperability that enables scale** — not
software delivery, not a data sync layer. Peat connects systems that don’t
speak the same language across transport and protocol boundaries.

Peat is developed under the Defense Unicorns GitHub org:
https://github.com/defenseunicorns

TODO: Add one sentence on relationship to UDS (Defense Unicorns’ flagshipproduct) — Peat is a peer/complement, not a subset.

-----

## Repo Map

All repos live at https://github.com/defenseunicorns/peat-*

|Repo                |Purpose                                                                                    |Status           |
|--------------------|-------------------------------------------------------------------------------------------|-----------------|
|**peat**            |Top-level crate. Shared types, traits, error handling. Dependency anchor for the ecosystem.|WIP / placeholder|
|**peat-registry**   |Registry sync engine. Kubernetes integration. First public-facing capability.              |Active           |
|**peat-mesh**       |Mesh networking layer. Multi-hop routing between nodes.                                    |Active           |
|**peat-btle**       |Bluetooth Low Energy transport bridge. M5Stack/ESP32 hardware integration.                 |Active           |
|**peat-node**       |Individual node implementation. Runs on edge hardware.                                     |Active           |
|**peat-gateway**    |Gateway and bridge layer. Connects mesh segments and external systems.                     |Active           |
|**peat-lite**       |Lightweight implementation for constrained environments.                                   |Active           |
|**peat-atak-plugin**|Android/Kotlin ATAK plugin. TAK ecosystem integration via FFI from Rust.                   |Active           |
|**peat-rmw**        |ROS2 RMW (Robot Middleware) integration.                                                   |Active           |
|**peat-mavlink**    |MavLink protocol integration.                                                              |Active           |



TODO: Add WearTAK integration location — inside peat-atak-plugin or separate?TODO: Add any private/internal repos not listed here.TODO: Confirm status of each repo (active / experimental / deprecated).

-----

## Architecture Invariants

These rules apply across every repo in the ecosystem. Claude Code sessions
must never violate these without explicit instruction and documented reasoning.

**Language**


All implementation is Rust except the Kotlin layer in peat-atak-plugin
No new language dependencies without explicit approval
No Python, no shell scripts for anything that belongs in Rust


**Dependency Flow**


peat (top-level crate) is the shared dependency anchor
Common types, traits, and error handling flow DOWN from peat
Individual repos depend on peat, never on each other directly
Circular dependencies are never acceptable


**Transport Agnosticism**


Peat protocol logic must never assume a specific transport
BLE, mesh, IP, serial — all are transports, all are interchangeable
Transport-specific code belongs in transport repos (peat-btle, peat-mesh)
  never in peat-node, peat-gateway, or peat core

**Interoperability First**


Every feature decision should be evaluated against: does this make Peat
  more or less interoperable with external systems?

Peat should never require a counterpart to run Peat software to integrate


**Error Handling**


TODO: Document error handling conventions (thiserror? anyhow? custom?)


**Async Runtime**


TODO: Document async runtime choice (Tokio? async-std?) and conventions


**No Unsafe Unless Necessary**


Unsafe Rust requires explicit justification in a code comment
FFI boundary (peat-atak-plugin) is the primary legitimate unsafe zone


-----

## The FFI Boundary

The only point where Rust leaves the ecosystem is the FFI layer in
peat-atak-plugin, which bridges to Android/Kotlin.

**Rules for Claude Code sessions at the FFI boundary:**


Never move business logic into the Kotlin layer — Kotlin is UI and Android
  lifecycle only

All Peat protocol logic stays in Rust, exposed via FFI
Memory management at the boundary is explicit — document ownership clearly
TODO: Document the FFI tooling in use (UniFFI? jni crate? manual bindings?)
TODO: Document how errors cross the FFI boundary
TODO: Document threading model at the boundary (Kotlin coroutines vs Rust async)


**What belongs in Rust:**


All Peat protocol logic
All mesh and transport logic
All registry sync logic
Data serialization/deserialization


**What belongs in Kotlin:**


Android UI
Android lifecycle management
ATAK plugin registration and hooks
Calling into Rust via FFI


-----

## Peat Core (WIP)

The top-level **peat** crate is currently a placeholder. Its intended role is
to be the shared dependency that gives the ecosystem a single source of truth
for types, traits, and errors.

**Intended contents:**


Core data types (messages, identities, capabilities)
Shared traits (Transport, Node, Registry)
Error types
TODO: Finalize what belongs here vs. stays in individual repos


**What does NOT belong in peat core:**


Transport implementations
Hardware-specific code
Platform-specific code (Android, ROS2, etc.)


Until peat core is stable, Claude Code sessions should be conservative aboutadding dependencies on it. Flag any assumptions about peat core types in PRdescriptions.

-----

## GitHub Workflow

**Org:** https://github.com/defenseunicorns

**Issue Format for Claude Code Sessions**

Each GitHub issue used as a Claude Code spec should include:

```
## Context
Which repo(s) this touches and why.

## Scope
What is in scope. What is explicitly out of scope.

## Acceptance Criteria
Specific, testable conditions for done.

## Constraints
Any architecture invariants, performance requirements, or
conventions that apply.

## Dependencies
Links to related issues or PRs in other repos.
```

**Branch Naming**


TODO: Document DU branch naming conventions


**PR Expectations**


Every PR must reference a GitHub issue
Claude Code PRs must include a summary of what was changed and why
CI must pass before review request
TODO: Document required reviewers per repo


**General Contractor Coordination**


Kit is the general contractor across all repos
Claude Code sessions are sub-contractors, scoped to one repo at a time
Cross-repo changes are coordinated through linked GitHub issues
No Claude Code session should make assumptions about another repo’s
  internals — use the public API/traits only

-----

## Per-Repo SKILL.md Index

Each repo has its own SKILL.md covering domain-specific rules, conventions,
and gotchas. Always read the per-repo SKILL.md before starting work.

|Repo            |SKILL.md location        |
|----------------|-------------------------|
|peat            |peat/SKILL.md            |
|peat-registry   |peat-registry/SKILL.md   |
|peat-mesh       |peat-mesh/SKILL.md       |
|peat-btle       |peat-btle/SKILL.md       |
|peat-node       |peat-node/SKILL.md       |
|peat-gateway    |peat-gateway/SKILL.md    |
|peat-lite       |peat-lite/SKILL.md       |
|peat-atak-plugin|peat-atak-plugin/SKILL.md|
|peat-rmw        |peat-rmw/SKILL.md        |
|peat-mavlink    |peat-mavlink/SKILL.md    |



TODO: Create per-repo SKILL.md files. Start with peat-registry andpeat-atak-plugin as the two most active repos.

-----

## Known Gotchas

This section grows over time. Add entries when Claude Code sessions makewrong assumptions or produce output that needs significant correction.


TODO: Populate from experience as sessions run


-----

*Last updated: May 2026*
*Maintained by: Kit Plummer, VP Data and Autonomy, Defense Unicorns*
