# ADR-064: Rust PR-Gating CI on Self-Hosted ARM64 Runners with Persistent CARGO_HOME

**Status**: Proposed
**Date**: 2026-05-30
**Authors**: Kit Plummer
**Related**: `peat-mesh/docs/adr/0012-android-ci-toolchain.md` (sibling per-repo ADR for the Android toolchain on the same self-hosted runner; defines the "PR-gating CI + releases stay on `ubuntu-latest` with official Google toolchain" rule that this ADR amends for Rust). Operational reference: `RUNNERS.md` (this repo's root; the cron and revert procedures referenced by D3 and the Consequences section live there as a "Planned: ADR-064 follow-up" stub until the first implementation PR fleshes them out).
**Triggered by**: Caching question raised during the 2026-05-30 Node-24 / runner-label session. Observation: ubuntu-latest Rust CI runs 3–7 min per matrix entry, dominated by registry download + crate compile that the GitHub-cache-API path can't fully eliminate. Three of the four self-hosted runners (`peat-arm64-linux-gb10-{btle,gateway,node}`) currently run only the Peat QA Review job (~1–2 min) and sit idle the rest of the time.

---

## Context

### What the runners look like today

Four self-hosted runners are registered, one per repo, all on the same host (`promaxgb10-e606`, NVIDIA GB10, aarch64, Ubuntu 24.04, user `kit`):

| Runner | Repo | Workload today | Daily utilization |
|---|---|---|---|
| `peat-arm64-linux-gb10-mesh` | peat-mesh | Android Functional Test (gradle, ~30 s) + Peat QA Review (~1–4 min) | substantive |
| `peat-arm64-linux-gb10-btle` | peat-btle | Peat QA Review only | very low |
| `peat-arm64-linux-gb10-gateway` | peat-gateway | Peat QA Review only | very low |
| `peat-arm64-linux-gb10-node` | peat-node | Peat QA Review only | very low |

All other CI — fmt, clippy, test, audit, feature-builds matrix, release/publish, SBOM — runs on `ubuntu-latest`. Caching is via `Swatinem/rust-cache@v2` against GitHub's cache API (10 GB per-repo cap, network round-trip).

The mesh runner additionally carries an unofficial Android toolchain (HomuHomu NDK + SDK rebuilds) because Google ships no Linux/aarch64 NDK or AAPT2. That toolchain is **lab-only**; releases and PR-gating Android builds explicitly run on `ubuntu-latest` to stay on Google's official toolchain. `peat-mesh/docs/adr/0012-android-ci-toolchain.md` captures this rule and the scope boundary.

### Why caching alone hits a ceiling

The quick wins landed in this session (peat-mesh #199/#201, peat-btle #69/#70, peat-gateway #130/#131, peat-node #110/#111) cover the affordable optimizations on `ubuntu-latest`:

- `Cargo.lock` checked in (peat-gateway), so the Swatinem cache key is stable across runs
- `taiki-e/install-action` for `cargo-audit` / `cargo-cyclonedx`, replacing per-run source compiles (was 1–2 min each)
- `cache-on-failure: true` so failed runs leave a warm cache for the next attempt
- peat-node migrated off hand-rolled `actions/cache` onto Swatinem

These compress the wall-clock floor but do not change the *shape* of the cost: each run still uploads/downloads multi-hundred-MB tarballs to/from GitHub's cache servers, and the 10 GB cap forces eviction of larger workspaces (peat-gateway's full feature matrix is the obvious cap-pressure case). Cache restore + compile still dominates the run.

The four self-hosted runners have persistent local disk (~3.6 TB free on the host's root partition per the 2026-05-29 inventory). A Rust workspace cache that lives on persistent local disk has zero upload/download cost, no cap, and survives runner restarts. The mesh runner's Android tests already exercise this property — they don't use `Swatinem/rust-cache` because Gradle's own state under `~/.gradle/` persists between jobs.

### The constraint these runners place on a CI move

1. **All four runners share one host.** If the GB10 box goes down, no Rust CI runs anywhere. `ubuntu-latest` has no such single point of failure.
2. **ARM64 only.** Moving compile to self-hosted means PR CI no longer exercises x86_64. Releases that publish x86_64 artifacts (peat-gateway and peat-node ship `linux/amd64` container images via Docker Buildx) MUST stay on x86_64 hosted runners; this ADR proposes nothing for the release path.
3. **Concurrency is host-bound, not runner-bound.** Each runner serves one job at a time, but all four share one host's CPU/RAM/disk. Running clippy+test+feature-matrix in parallel across all four runners can starve the host. `ubuntu-latest` parallelism is hardware-isolated by GitHub.
4. **Self-hosted PR runners running PR-author code is the standard security caveat.** For these four repos (private + trusted-contributor) the exposure is bounded but non-zero; fork PRs are not run today (the QA Review workflow already gates on `head.repo.full_name == github.repository`) and this proposal does not change that gate.

### Why this lives in an ADR

Two reasons:

1. It amends the rule recorded in `peat-mesh/docs/adr/0012-android-ci-toolchain.md` ("PR-gating CI + releases stay on `ubuntu-latest`"). That rule's stated reason — no Google-supplied aarch64 Android tooling — does not apply to Rust. The ADR makes the carve-out explicit so future readers don't read the Android rule as banning self-hosted CI categorically.
2. It affects all four runner-hosting repos with a coordinated infra change. Per repo policy, cross-repo changes require one PR per repo linked through a tracking issue. This ADR is that anchor.

---

## Decision

**Adopt a hybrid PR-gating CI model: move the heavy per-PR Rust matrix to the existing self-hosted ARM64 runners with persistent `CARGO_HOME`; keep lightweight gates and the entire release/publish path on `ubuntu-latest`.**

### D1. Which jobs move

For each of the four runner-hosting repos:

| Workflow / Job | Today | After |
|---|---|---|
| `ci.yml` Clippy (incl. matrix) | `ubuntu-latest` | self-hosted |
| `ci.yml` Test (incl. matrix) | `ubuntu-latest` | self-hosted |
| `ci.yml` Feature builds matrix (peat-gateway) | `ubuntu-latest` | self-hosted |
| `ci.yml` NATS sink integration (peat-gateway) | `ubuntu-latest` | `ubuntu-latest` (docker + service container) |
| `ci.yml` Format | `ubuntu-latest` | `ubuntu-latest` (fast; no benefit from cache) |
| `ci.yml` Security Audit | `ubuntu-latest` | `ubuntu-latest` (fetches advisory DB; no compile) |
| `ci.yml` Helm Lint (peat-node) | `ubuntu-latest` | `ubuntu-latest` |
| `ci.yml` Android Functional Test (peat-mesh) | self-hosted | self-hosted (unchanged) |
| `ci.yml` Cross-cluster sync (peat-node) | `ubuntu-latest` | `ubuntu-latest` (k3d × 2 — hosted KVM is required) |
| `release.yml`, `publish.yml`, `publish-maven.yml`, `sbom.yml`, `cross-cluster-sync.yml` | `ubuntu-latest` | `ubuntu-latest` (unchanged) |
| `qa-review.yml` | self-hosted | self-hosted (unchanged) |

The release path stays on `ubuntu-latest` so binary releases and container images continue to be produced on x86_64 hardware with GitHub-supplied toolchain. PR gating still hits a clean `ubuntu-latest` environment on the **NATS sink integration**, **Security Audit**, **Format**, and **release** workflows, so an x86_64-only regression is still caught before release.

### D2. Persistent CARGO_HOME and target dir

`actions/checkout@v6` defaults to `clean: true`, which would wipe `target/` between runs. Each migrated job sets:

```yaml
- uses: actions/checkout@v6
  with:
    clean: false
```

and relies on the runner's persistent working directory (`/home/kit/Code/Peat/actions-runner-<repo>/_work/<repo>/<repo>`) to keep `target/` across runs. Cargo's content-fingerprint-based rebuild detection is robust against a fresh `git checkout` over the existing working tree (it tracks Cargo.lock hash + source content, not file mtimes).

`CARGO_HOME` defaults to `/home/kit/.cargo` on the runner user, which is already persistent and shared across all jobs on that runner. No env override required. The registry index and `.cargo/registry/cache/` survive between runs automatically.

`Swatinem/rust-cache@v2` is removed from migrated jobs — its upload/download is pure overhead when the cache it would write to lives on the same disk it would read from.

### D3. Periodic prune

`target/` and `~/.cargo/registry/cache/` grow indefinitely. A weekly cron on the runner host prunes:

- `target/` directories older than 7 days (per `_work` tree)
- `~/.cargo/registry/cache/*/` to crates not referenced by any current `Cargo.lock` across the four `_work` trees

The cron is host-level (lives in `kit`'s crontab), documented in `RUNNERS.md`. If the prune fails or the disk fills, the worst case is the runner refuses jobs — CI degrades visibly rather than silently corrupting. Disk-fill is the single most likely operational failure mode.

### D4. Rollout: one repo at a time, observation period before the next

Order: **peat-btle** first (smallest matrix, lowest blast radius) → **peat-node** → **peat-mesh** → **peat-gateway** (largest matrix, highest payoff).

Between each repo's migration, leave 3 working days of observation. Watch for:

- Wall-clock change per matrix entry (target: 50%+ reduction on warm cache; cold cache is no worse than today)
- Disk fill rate on the host
- Whether `target/` content-fingerprint detection holds (false-positive rebuilds = back out)
- Whether the host load creates queueing on QA Review or Android Functional Test (which would erase the PR-latency win)

If any of these regress materially, the migration for that repo reverts via a one-line `runs-on:` change. The release path was never moved, so a revert never blocks a release.

### D5. Tracking issue

Cross-repo coordination lives in [peat#950](https://github.com/defenseunicorns/peat/issues/950). Each of the four implementation PRs links back to that issue per repo policy.

---

## Consequences

### Accepted

- **PR CI exercises ARM64 only.** x86_64-only regressions in dep code (e.g. an unconditional `target_arch = "x86_64"` block in a transitive crate) won't be caught until the release workflow's `cargo publish` step or the container build. This is the explicit trade — x86_64 release coverage is preserved, but the per-PR feedback loop is ARM64-only. The NATS sink integration job is left on `ubuntu-latest` partly as a sanity-check x86_64 compile-and-test pass per PR.
- **Single point of failure.** GB10 outage blocks all Rust PR CI across four repos until either the host comes back or workflows are reverted to `ubuntu-latest`. Revert is a one-line per-job change so MTTR is bounded; documented in `RUNNERS.md`.
- **Host-shared concurrency.** Running peat-gateway's full feature matrix concurrently with peat-mesh's Android tests will share the host. Mitigated by D4's per-repo rollout (we'll observe contention before stacking workloads).
- **One more operational responsibility.** Disk monitoring + the D3 cron become part of the runner-maintenance surface. Already in scope (the host today already needs disk monitoring for `_work` growth), this codifies it.

### Rejected alternatives

- **sccache with a shared backend (S3-compatible).** Same upload/download shape as `Swatinem/rust-cache`, just a different bucket. Wins are real but smaller than persistent local disk, and the operational cost (S3 bucket, IAM, lifecycle, network) is non-trivial. Revisit only if the SPOF problem in D4 forces us off self-hosted.
- **Larger GH-hosted runners (paid).** Buys parallelism and bigger disks; does not change the cache-API round-trip shape. Cost-per-minute scales linearly with our CI load. Revisit if the persistent-disk approach proves unstable.
- **Status quo with the in-flight caching PRs and nothing more.** Measured wall-clock floor is bounded by the cache-API round-trip and crate compile, both of which the in-flight PRs cannot eliminate. Acceptable today; the question is whether it stays acceptable as the workspace grows.
- **Move release CI to self-hosted too.** Breaks the x86_64 binary/container production guarantee. Explicitly out of scope.

### Reversibility

Per-job. Each migrated job's `runs-on:` flips between `ubuntu-latest` and `peat-arm64-linux-gb10-<repo>` independently. No data migration; the `ubuntu-latest` path still has its own Swatinem cache available if reverted.

---

## Open questions

1. Does Cargo's content-fingerprint rebuild detection actually hold under `actions/checkout@v6 with: clean: false` across a year-plus of branch churn? We assume yes; the D4 observation period checks empirically. If it doesn't, we'd need to add an explicit `cargo clean -p <changed-pkg>` step keyed off `git diff --name-only`, which complicates the workflow.
2. Should `target/` live under `$CARGO_TARGET_DIR` (a host-level shared path keyed on repo) rather than in the `_work` tree? Sharing across worktrees and branches inside the same repo's runner would compound the cache hit rate. Trade-off: a shared target dir is a lock-contention surface if two branches build simultaneously, and cargo's per-target lockfile is a single global lock per directory.
3. Is the per-repo runner model (`peat-arm64-linux-gb10-{repo}`) the right shape, or should the four collapse into one shared `peat-arm64-linux-gb10` runner used by all repos via repo-scoped registration? Today's model maps cleanly to the "no org-level GH permissions" rule; a shared runner would need an org-level registration we explicitly avoid.

Defer these to implementation if the decision lands; none are blocking.
