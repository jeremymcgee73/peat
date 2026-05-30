# Peat self-hosted runners — operational reference

Living doc capturing which self-hosted runner has which tooling, labels,
and quirks. Update this when you change host state, not just ADRs.

All runners are user-level systemd template instances
(`actions-runner@<instance>.service`) under user `kit`. None are
org-scoped; each is registered against a specific repo per Peat's
"no org-level GH permissions" rule.

## peat-arm64-linux-gb10-mesh — runner id 21

- **Repo:** `defenseunicorns/peat-mesh`
- **Host:** GB10 (NVIDIA Grace, aarch64), Ubuntu 24.04
- **Service:** `actions-runner@actions-runner-mesh.service`
- **Install dir:** `/home/kit/Code/Peat/actions-runner-mesh`
- **Labels:** `self-hosted`, `Linux`, `ARM64`, `peat-arm64-linux-gb10`,
  `android`, `android-ndk-r27d-custom`

### Android toolchain

The aarch64 mesh runner uses two unofficial community rebuilds from the
same maintainer (`HomuHomu833`). Google ships no native Linux aarch64
build of any Android host tool — NDK, build-tools (AAPT2), or
platform-tools. See `peat-mesh/docs/adr/0012-android-ci-toolchain.md`
for the rationale and the lab-vs-CI scope boundary.

| Component | Source | Path |
|---|---|---|
| JDK 17 | apt: `openjdk-17-jdk-headless` | `/usr/bin/java` |
| Android SDK (cmdline-tools, build-tools 36.1.0, platform-tools, licenses) | [HomuHomu833/android-sdk-custom](https://github.com/HomuHomu833/android-sdk-custom) release `36.0.2` | `~/Android/Sdk/{build-tools,cmdline-tools,platform-tools}` |
| Android platforms (android-34) | preserved from sdkmanager install (arch-agnostic) | `~/Android/Sdk/platforms/android-34` |
| NDK r27d (aarch64-linux-musl) | [HomuHomu833/android-ndk-custom](https://github.com/HomuHomu833/android-ndk-custom) release `r27` | `~/Android/Sdk/ndk/android-ndk-r27d` |
| adb (native aarch64) | HomuHomu SDK OR apt: `android-sdk-platform-tools-common` | `~/Android/Sdk/platform-tools/adb` or `/usr/bin/adb` |
| Rust Android targets | `rustup target add` | `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android` |
| cargo-ndk | `cargo install cargo-ndk` (4.1.2) | `~/.cargo/bin/cargo-ndk` |

A backup of the prior sdkmanager-installed (x86_64) SDK is at
`~/Android/Sdk.x86_64-bak`. Removable once the aarch64 toolchain has
proven itself across a few CI cycles.

### Runner `.env`

`actions-runner-mesh/.env` exposes `ANDROID_HOME`, `ANDROID_NDK_HOME`,
`ANDROID_NDK_ROOT`, and prepends the linux-arm64 NDK toolchain bin dir
to `PATH`. Loaded at runner startup, applied per-job. Restart with
`systemctl --user restart actions-runner@actions-runner-mesh` after
edits.

### Host-level Gradle override

AGP downloads its own AAPT2 jar from Google's Maven repo
(`aapt2-<v>-linux.jar`, x86_64) and prefers it over any SDK-installed
AAPT2. To force AGP onto the aarch64 binary, `~/.gradle/gradle.properties`
on this host sets:

```
android.aapt2FromMavenOverride=/home/kit/Android/Sdk/build-tools/36.1.0/aapt2
```

This is **user-level only** and deliberately not committed to any
project repo — CI on `ubuntu-latest` must continue to use Google's
bundled AAPT2.

### Verified invocation

End-to-end build + deploy + instrumented test against the lab Samsung
tablet (`SM-X210`, USB) verified on 2026-05-20:

```
cd peat-mesh/android-tests
ANDROID_NDK_HOME=~/Android/Sdk/ndk/android-ndk-r27d \
  ANDROID_HOME=~/Android/Sdk \
  ./gradlew connectedCheck -PandroidTestAbis=arm64-v8a
```

`android-tests`'s default ABI is `x86_64` (tuned for the GH emulator
CI job); the `-PandroidTestAbis=arm64-v8a` override is mandatory for
deploying to ARM64 lab hardware.

### Quirks

- The HomuHomu NDK ships `toolchains/llvm/prebuilt/linux-x86_64` as
  a **symlink to `linux-arm64`**. Gradle projects that hardcode
  `linux-x86_64/bin` (including `android-tests/build.gradle.kts:89`)
  resolve transparently — no patches needed.
- `cargo-ndk -p 24` collides with cargo's `--package` flag — use
  the long form `--platform 24` in workflows.
- Don't `sdkmanager "ndk;..."` or `sdkmanager "build-tools;..."` —
  it would download x86_64 binaries and silently break things. The
  aarch64 toolchain is managed manually from HomuHomu releases.
- The Android emulator is not installed and is not supported on
  this host (Google ships `emulator` only for `linux-x86_64`).
  On-device testing only.

### Lab devices

- **Samsung Galaxy Tab A9+ WiFi (`SM-X210`, serial `R95YA03CTGW`)** —
  connected via USB, authorized, used for arm64-v8a instrumented
  tests.

## Other runners

- `actions-runner-btle` → `defenseunicorns/peat-btle` — see runner's
  own `.env`; Android release builds for peat-btle currently run on
  `ubuntu-latest` (not this runner).
- `actions-runner-gateway` → `defenseunicorns/peat-gateway`.
- `actions-runner-node` → `defenseunicorns/peat-node`.
- `actions-runner` → `defenseunicorns/peat` (base ecosystem repo).

Update these stanzas as their tooling changes; no separate ADRs needed
unless a host-level decision (e.g. unofficial toolchain choice) carries
a provenance trade-off.

## Planned: ADR-064 follow-up (Rust PR-gating CI on self-hosted runners)

These procedures activate **only if [ADR-064](docs/adr/064-rust-ci-on-self-hosted-runners.md)
is accepted** and the implementation PRs land per the rollout in
[peat#950](https://github.com/defenseunicorns/peat/issues/950). They
are stubs today; the implementation PRs are expected to flesh them out
with the actual cron entries and the per-repo revert steps as those
land.

### Disk-prune cron (D3 of ADR-064)

Once heavy Rust CI runs on the self-hosted runners, both `target/`
(inside each runner's `_work` tree) and `~/.cargo/registry/cache/`
grow without bound. A weekly host-level cron (in `kit`'s crontab,
not committed in any repo) prunes:

1. `target/` directories under
   `/home/kit/Code/Peat/actions-runner-<repo>/_work/<repo>/<repo>/`
   that have not been touched in the last 7 days.
2. Crate tarballs under `~/.cargo/registry/cache/<index>/` that are
   not referenced by **any** current `Cargo.lock` across the four
   runner-hosting repos' `_work` trees.

Failure modes:

- If the prune fails or the disk fills, the runner refuses jobs and
  CI degrades visibly — a job will not silently produce a wrong
  result.
- The cron's exact command, schedule, and log location land with the
  first implementation PR (peat-btle, per ADR-064 §D4) and the cron
  spec is appended to this section at that time.

### Revert procedure (D4 / SPOF consequence of ADR-064)

Each migrated job's placement is one line of YAML
(`runs-on: peat-arm64-linux-gb10-<repo>` vs `runs-on: ubuntu-latest`).
If the GB10 host goes down or any migrated repo regresses past the
3-working-day observation window:

1. In the affected repo's `.github/workflows/ci.yml` (or `ci.yaml`),
   change the `runs-on:` of each migrated job back to `ubuntu-latest`.
2. Restore `Swatinem/rust-cache@v2` step (the implementation PR will
   leave a comment marker `# ADR-064: Swatinem removed for self-hosted`
   to make the revert mechanical).
3. Restore `clean: true` (or remove the explicit `clean: false` on
   `actions/checkout`).
4. Open a PR. No data migration; the GitHub Actions cache still has
   the old Swatinem keys (or builds fresh on first restore).

MTTR is bounded by review + merge of a single one-or-two-file PR per
affected repo. The release path was never migrated, so a revert never
blocks a release.
