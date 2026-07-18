# AGENTS.md — `<repo-name>`

> **Template.** Copy this file to `<repo>/AGENTS.md` and fill in every
> angle-bracketed placeholder. Keep this file small because it loads into
> context every session. Detailed workflow content belongs in the repo-scoped
> skill, not here.

For implementation, review, architecture, or debugging work in this repo, use
the `$<repo-name>` skill in
`.agents/skills/<repo-name>/SKILL.md`. Codex discovers that repo-scoped skill
automatically.

Also use the `$peat-ecosystem` skill for Peat's cross-repository architecture,
security invariants, dependency rules, and verification requirements. If a
task touches another peat-* repository, use that repository's skill as well.
Cross-repository changes require one PR per repository, linked through a
tracking issue.

## Quick orientation

- **Repo role:** <one line — what this repo does in the Peat ecosystem>
- **Primary language:** <Rust / Kotlin / mixed>
- **Cheap sanity check:** `<command — e.g., cargo check -p <crate>>`

## Hard rule

A task in this repo is not done until the verification checklist in the
`$<repo-name>` skill produces evidence. "Seems right" or "the diff looks
correct" is never sufficient.
