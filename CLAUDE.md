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
