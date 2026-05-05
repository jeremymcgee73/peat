---
name: <repo-name>
description: <one-line — what this skill is for, when to read it>
when_to_use: <triggers — file paths, task types, or PR labels that should pull this skill into context>
verifies_with: <one-line summary of what evidence proves a session in this repo is done>
---

# `<repo-name>` SKILL

> **Template.** Copy this file to `<repo>/SKILL.md` and fill in every angle-bracketed placeholder. Delete sections that don't apply, but keep the headings in order. The shape is load-bearing — it's how the agent knows what to attend to.

<One paragraph orientation: what this repo is, why it exists, its role in the ecosystem. Max 4 sentences. No history, no aspirations — just the current truth.>

## When this skill applies

- <Trigger: files under `<path>/` are touched>
- <Trigger: task involves `<topic>`>
- <Trigger: PR is labeled `<label>` or references issue type `<type>`>

If none of these apply, stop here and use `peat/SKILL.md` (ecosystem) only.

## Scope

**In scope:**
- <thing this skill covers>
- <thing this skill covers>

**Out of scope (route elsewhere):**
- <thing> → `<other-repo>/SKILL.md`
- <thing> → ecosystem skill `peat/SKILL.md`
- <thing> → open an issue, do not implement here

## Workflow

Numbered steps. Skipping a step requires explicit user instruction.

1. **Orient.** Read `peat/SKILL.md` (ecosystem), this file, the affected files, `git status`, `git log -10`.
2. **Locate the spec.** Confirm the task has a GitHub issue with Context / Scope / Acceptance / Constraints / Dependencies. If not, stop and ask the user.
3. **Plan.** Produce a 1–5 step plan. Cross-check against the hard invariants in `peat/SKILL.md` and the scope guards below.
4. **Implement.** Vertical slices. One concern per commit. Match existing conventions in this repo.
5. **Verify.** Run every command in the verification checklist. Capture output.
6. **Hand off.** Open PR referencing the issue. Summary states *what changed and why*. Flag cross-repo implications explicitly.

## Verification (exit criteria)

A session in this repo is not done until each of these produces evidence:

- [ ] `<verify command 1>` exits 0 — e.g., `cargo test -p <crate>`
- [ ] `<verify command 2>` exits 0 — e.g., `cargo clippy -p <crate> --all-targets -- -D warnings`
- [ ] `<verify command 3>` exits 0 — e.g., `cargo fmt --check`
- [ ] `<behavioral check>` — e.g., "binary runs against `<fixture>` and produces `<expected log line>`"
- [ ] `<cross-repo check, if applicable>` — e.g., "`peat-node` still builds against this branch"

"Seems right" or "the diff looks correct" is never sufficient.

## Anti-rationalization

Pre-written rebuttals to lies the agent hasn't yet told. Add a row each time a session uses an excuse to skip the workflow.

| Excuse | Rebuttal |
|---|---|
| "This change is too small to need a test." | If it's worth changing, it's worth one assertion. Add the test. |
| "I'll fix the lint warning later." | Later doesn't exist. Fix it before the commit. |
| "This refactor is right next door — I'll just clean it up." | Out of scope. Open a separate issue if it matters. |
| "The CI will catch it if I'm wrong." | CI is a backstop, not a substitute. Run the verify commands locally first. |
| "<repo-specific excuse>" | <repo-specific rebuttal> |
| "<repo-specific excuse>" | <repo-specific rebuttal> |

## Scope guards

- Touch only files the issue/user asked you to touch.
- Do not edit files in other `peat-*` repos. Surface cross-repo work as a separate issue.
- Do not add dependencies, languages, build systems, or runtimes without explicit user approval.
- Do not assume internals of other peat repos — use their public API/traits only.
- <repo-specific guard, e.g., "do not modify code under `vendor/` — it is generated">

## Gotchas

Add an entry each time a session produces output that needed correction. One line per gotcha plus a `Why:` line so future sessions can judge edge cases.

- *(none recorded yet)*

## References (read on demand, not by default)

- Ecosystem invariants: `peat/SKILL.md`
- <repo-specific architecture doc, if any>
- <link to relevant issue tracker / project board>

---
*Last updated: <YYYY-MM-DD>*
*Maintained by: <name>, <role>*
