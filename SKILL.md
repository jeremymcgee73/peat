---
name: peat-ecosystem
description: Top-level skill for Claude Code sessions across any peat-* repo. Read first, then read the per-repo SKILL.md.
when_to_use: Any session touching files in a defenseunicorns/peat-* repository, or coordinating changes across more than one peat repo.
verifies_with: Each affected repo's CI green, no architecture invariant violated, PR references its issue with the required sections.
---

# Peat Ecosystem SKILL

Peat is an interoperability-first mesh registry sync platform built for heterogeneous autonomous systems in defense and edge environments. Its core value proposition is **interoperability that enables scale** — Peat connects systems that don't speak the same language across transport and protocol boundaries. Peat is developed under the Defense Unicorns GitHub org: https://github.com/defenseunicorns

## When this skill applies

- Any session touching files in a `peat-*` repo or the top-level `peat` crate
- Cross-repo changes affecting more than one peat repo
- Reviewing a PR in any peat repo

After reading this file, read the relevant per-repo SKILL.md from the router below. Per-repo skills are authored against `peat/SKILL_TEMPLATE.md`.

## Skill router

Read only what's relevant to the current task. Do not preload every per-repo skill.

| Repo | Purpose | Skill | Status |
|---|---|---|---|
| **peat** | Top-level crate; shared types, traits, errors. Dependency anchor. | This file (see "peat repo-specific" section below) | WIP / placeholder |
| **peat-registry** | Registry sync engine. Kubernetes integration. | `peat-registry/SKILL.md` | Active |
| **peat-mesh** | Mesh networking; multi-hop routing. | `peat-mesh/SKILL.md` | Active |
| **peat-btle** | BLE transport bridge. M5Stack/ESP32. | `peat-btle/SKILL.md` | Active |
| **peat-node** | Edge-hardware node implementation. | `peat-node/SKILL.md` | Active |
| **peat-gateway** | Mesh-to-external bridge layer. | `peat-gateway/SKILL.md` | Active |
| **peat-lite** | Constrained-environment implementation. | `peat-lite/SKILL.md` | Active |
| **peat-atak-plugin** | Android/Kotlin ATAK plugin via Rust FFI. | `peat-atak-plugin/SKILL.md` | Active |
| **peat-rmw** | ROS2 RMW integration. | `peat-rmw/SKILL.md` | Active |
| **peat-mavlink** | MavLink protocol integration. | `peat-mavlink/SKILL.md` | Active |

## Hard invariants (cross-cutting)

These rules apply in every repo. Violating one without explicit user approval is out of scope, full stop.

**Language.** Rust everywhere except the Kotlin layer in `peat-atak-plugin`. No new language dependencies. No Python. No shell scripts for anything that belongs in Rust.

**Dependency flow.** `peat` is the dependency anchor. Common types, traits, error handling flow *down* from `peat`. Repos depend on `peat`, never on each other directly. Circular dependencies are rejected.

**Transport agnosticism.** Peat protocol logic must not assume a transport. BLE, mesh, IP, serial are all interchangeable. Transport-specific code stays in transport repos (`peat-btle`, `peat-mesh`), never in `peat-node`, `peat-gateway`, or `peat` core.

**Interoperability first.** Every feature decision answers: does this make Peat more or less interoperable with external systems? Peat must never require a counterpart to run Peat software to integrate.

**Unsafe Rust.** Requires explicit justification in a code comment. The FFI boundary in `peat-atak-plugin` is the only routinely legitimate unsafe zone.

**FFI boundary direction.** All Peat protocol logic stays in Rust. Kotlin in `peat-atak-plugin` is UI and Android lifecycle only — never a destination for protocol, mesh, transport, registry, or serialization logic.

## Workflow

For any task in a peat repo:

1. **Orient.** Read this file. Read the per-repo SKILL.md from the router. Read `CLAUDE.md` if present. Run `git status` and `git log -10`.
2. **Locate the spec.** Confirm the task has a GitHub issue with the required sections (see "Issue format" below). If not, ask the user before implementing.
3. **Plan.** Produce a short plan. Check it against the hard invariants and the per-repo skill's scope guards.
4. **Implement** following the per-repo workflow. Vertical slices, one concern per commit.
5. **Verify** per the per-repo skill's exit criteria.
6. **Hand off.** Open PR referencing the issue. Summary states *what changed and why*. Flag any cross-repo implications.

## Verification (ecosystem-level)

Beyond the per-repo verify checklist, an ecosystem-level change is not done until:

- [ ] Each affected repo's CI is green
- [ ] No new cross-repo cycle introduced (`peat` does not depend on its consumers; sibling repos do not depend on each other)
- [ ] PR references a GitHub issue with Context / Scope / Acceptance Criteria / Constraints / Dependencies sections
- [ ] If a hard invariant was waived, the PR description names which one and quotes the user approval

"Seems right" or "the diff looks correct" is never sufficient.

## Anti-rationalization

| Excuse | Rebuttal |
|---|---|
| "This change spans repos but they're tightly coupled — one big PR is cleaner." | One PR per repo, linked through a tracking issue. Atomicity across repos isn't real; reviewability is. |
| "I'll just import directly from `peat-mesh` into `peat-node` for this." | Cross-repo direct deps are a circular-dependency factory. Add the trait to `peat` core. |
| "It's only a tiny shell script for a build helper." | No shell scripts for things that belong in Rust. Justify the language choice with the user first. |
| "I'll write a quick TS/Python utility for this." | No new language dependencies without explicit approval. |
| "`peat` core doesn't have the type I need; I'll just put it in this repo for now." | Surface the gap as an issue against `peat`. Don't fork the type system. |
| "I'll move this Peat protocol logic into Kotlin to make the FFI simpler." | All Peat protocol logic stays in Rust. Kotlin is UI and Android lifecycle only. |
| "This change makes Peat assume the counterpart is also running Peat — fine for now, we'll generalize later." | Interoperability-first is the product. Generalize before merging, or don't merge. |

## Scope guards

- Kit is the general contractor across all repos. Claude Code sessions are sub-contractors, scoped to **one repo at a time**.
- Cross-repo changes are coordinated through linked GitHub issues, not by reaching across repos.
- Use the public API/traits of other peat repos. Never assume their internals.
- Do not add a dependency on `peat` core that assumes types or traits not yet stabilized — flag assumptions in the PR.
- Do not add new repos to the ecosystem without explicit user approval.

## Issue format

Each issue used as a Claude Code spec must include:

```
## Context
Which repo(s) this touches and why.

## Scope
What is in scope. What is explicitly out of scope.

## Acceptance Criteria
Specific, testable conditions for done.

## Constraints
Architecture invariants, performance requirements, conventions.

## Dependencies
Links to related issues or PRs in other repos.
```

## Gotchas

Populate as sessions run. One line per gotcha plus a `Why:` line.

- *(none recorded yet)*

---

# `peat` repo-specific skill

The remainder of this file is the per-repo skill for the `peat` top-level crate, since this repo also hosts the ecosystem skill above.

## Status

WIP / placeholder. Until `peat` core stabilizes, sessions should be conservative about adding dependencies on it and must flag any assumptions about peat-core types in PR descriptions.

## Intended contents

- Core data types (messages, identities, capabilities)
- Shared traits (`Transport`, `Node`, `Registry`)
- Error types

## Does NOT belong in `peat` core

- Transport implementations
- Hardware-specific code
- Platform-specific code (Android, ROS2, etc.)

## Workflow guard for changes to `peat` core

- Any new public type or trait requires a brief design note in the PR description.
- Any breaking change to a public type requires a list of downstream repos that need updating, in the PR description.
- Removing a public item requires confirming via `cargo check -p <consumer>` that no consumer in the workspace breaks (or coordinating an update PR in each consumer first).

---

# Open questions (for Kit)

These are TODOs that block the skill set from being complete. They're listed here so they're visible, not to imply the agent should resolve them autonomously.

- One-sentence statement on Peat's relationship to UDS (peer / complement vs. subset)
- Status confirmation per repo (active / experimental / deprecated)
- WearTAK integration location — inside `peat-atak-plugin` or separate repo
- Any private/internal repos not listed in the router
- Error handling conventions across the workspace (`thiserror` vs. `anyhow` vs. custom)
- Async runtime choice and conventions (Tokio vs. async-std vs. other)
- Branch naming convention for DU repos
- Required reviewers per repo
- FFI tooling for `peat-atak-plugin` (UniFFI vs. `jni` crate vs. manual bindings) — belongs in `peat-atak-plugin/SKILL.md` once decided
- FFI error-crossing convention — same
- FFI threading model (Kotlin coroutines ↔ Rust async) — same

---
*Last updated: 2026-05-05*
*Maintained by: Kit Plummer, VP Data and Autonomy, Defense Unicorns*
