# CLAUDE.md — `<repo-name>`

> **Template.** Copy this file to `<repo>/CLAUDE.md` and fill in every angle-bracketed placeholder. Keep this file small — it loads into context every session. Workflow content belongs in `SKILL.md`, not here.

Before doing any work in this repo, read **both** of:

1. `<repo>/SKILL.md` — the per-repo workflow, verification checklist, and scope guards.
2. `peat/SKILL.md` — the ecosystem skill (router, hard invariants, anti-rationalization).

If you cannot access `peat/SKILL.md` (e.g., this repo is checked out without the ecosystem anchor next to it), say so before proceeding — the architectural invariants live there, not here.

## Quick orientation

- **Repo role:** <one line — what this repo does in the Peat ecosystem>
- **Primary language:** <Rust / Kotlin / mixed>
- **Cheap sanity check:** `<command — e.g., cargo check -p <crate>>`

## Hard rule

A task in this repo is not done until the verification checklist in `SKILL.md` produces evidence. "Seems right" or "the diff looks correct" is never sufficient.
