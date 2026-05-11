# CLAUDE.md — `peat`

Before doing any work in this repo, read `SKILL.md`. This repo hosts both the **ecosystem skill** (used by every peat-* repo) and the **per-repo skill** for the `peat` top-level crate — they're in the same file, separated by a `---` break.

If your task touches another peat-* repo, read that repo's `SKILL.md` as well. The skill router in `SKILL.md` lists them.

## Quick orientation

- **Repo role:** Top-level crate; shared types, traits, errors. Dependency anchor for the Peat ecosystem.
- **Primary language:** Rust
- **Cheap sanity check:** `cargo check -p peat` (peat core is WIP — most behavioral verification lands in consumer repos)

## Hard rule

A task in this repo is not done until the verification checklist in `SKILL.md` produces evidence. "Seems right" or "the diff looks correct" is never sufficient.

Cross-repo changes require one PR per repo, linked through a tracking issue — not a single PR that reaches across repos.

## Hard rule: no consumer-specific references in peat

**peat is the generic mesh protocol.** Consumers (mobile-app plugins, wearable firmware, CLI tools, server bridges) live in their own repos and depend on peat. peat does NOT reference any specific consumer by name in code, comments, examples, READMEs, operational docs, JNI symbol names, package paths, or test fixtures.

Forbidden references include but are not limited to: vendor names (ATAK, WinTAK, iTAK, WearTAK, etc.), vendor-derived module/file names (e.g. `peat_<vendor>_client.rs`), package-path namespacing that includes a vendor (e.g. `com.defenseunicorns.<vendor>.peat.*`), and prose that says "the X plugin" / "for X" when describing what a generic consumer does.

**Acceptable generic terms:** "consumer", "consumer plugin", "CoT consumer", "mobile-app plugin", "wearable", "CLI tool", "server bridge". When a protocol name is structurally load-bearing (e.g. CoT XML, the TAK Server wire protocol that `peat-transport/src/tak/` bridges to), the *protocol* name may appear; the *consumer* name may not.

**The only places consumer names may appear** are: (1) ADRs (`docs/adr/`) when citing a real-world use case that motivated a design decision (even there, prefer generic language); and (2) genuine third-party identifiers that operational tooling targets verbatim — the host app's actual Android package id (`com.atakmap.*`), its activity classes (`ATAKActivity`), and the sibling repo's actual name (`peat-atak-plugin`). These are not "references to a consumer" in the rule's sense; they are external identifiers that the operational layer literally invokes by string. The SKILL.md grep gate excludes them.

If a task in this repo would introduce a consumer reference into code/comments/operational docs, do not write it. Find the generic equivalent or stop and surface the design tension explicitly.

This rule exists because: peat's value as a protocol depends on it being a peer-equal substrate for many consumers, not a bespoke runtime for one vendor. Every consumer-specific identifier that lands in peat couples the substrate to that vendor's roadmap and signals to other potential consumers that the substrate isn't generic.
