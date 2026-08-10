# `supply-chain/` — `cargo-vet` configuration

This directory holds the `cargo-vet` configuration for the `peat` workspace. `config.toml` itself is **rewritten by `cargo vet` on every invocation** — it gets alphabetized, comments are stripped, and any operational guidance embedded inline disappears. This README is the durable home for that operational guidance so it survives `cargo vet`'s linter pass.

## Why the entries in `config.toml` exist

### `[policy.peat]`, `[policy.peat-protocol]`, `[policy.peat-schema]` (`audit-as-crates-io = true`)

These three crates are first-party workspace members **and** are published to crates.io. Without these policy blocks, `cargo-vet` flags them as "non-crates.io-fetched packages match published crates.io versions" because the local path dep and the published artifact resolve to the same version. `audit-as-crates-io = true` tells vet to audit the local copy as if it were the crates.io version — which is accurate, because the workspace *is* the publisher.

**Footgun: add the entry only AFTER the crate's first publish.** `audit-as-crates-io = true` makes `cargo vet` attempt to fetch the crate from crates.io. If the crate is not yet published, the fetch fails with `Cannot fetch crate information` and CI breaks. The `[policy.peat]` entry, in particular, exists because `peat:0.9.0-rc.4` was published to crates.io as a reserved-name placeholder on 2026-05-11; once a name is reserved with a real version, every subsequent workspace version bump trips the "non-crates.io-fetched" check unless the policy is declared.

### `[[exemptions.peat]]`, `[[exemptions.peat-protocol]]`, `[[exemptions.peat-schema]]` per-version blocks

Each release of a first-party workspace crate that has been published gets an exemption stanza at `criteria = "safe-to-deploy"`. The exemption records that *we* (as the publisher) are the trust root for that version. A future CI pass can replace any of these with proper `cargo vet certify` audits if desired, but the exemption is the minimum needed to keep CI green after a version bump.

**Operational workflow.** When the workspace cuts a new rc release (e.g. `0.9.0-rc.11`), three new exemption stanzas must land in `config.toml` — one for each first-party crate. Forgetting this is the most common CI failure on docs-only PRs that didn't intend to touch supply-chain. The peat#870 docs branch hit exactly this when main bumped to rc.10 without matching exemptions.

### Temporary first-party Git dependency policy

Peat#1066 temporarily enables `[policy.peat-mesh] audit-as-crates-io = false`
while the workspace consumes the merged peat-mesh application-delivery source,
including the bounded received-document query from peat-mesh#389, at exact commit
`3d2985e5c974ff4124d55d6bbc652c9a61ae8d9c`. This is a reproducible Git pin,
not a mutable branch or local-path dependency. Remove the policy and source pin
after the prerequisite is published and the workspace returns to crates.io.

Normal crates.io versions remain covered by the `[[trusted.peat-mesh]]` and
`[[trusted.peat-btle]]` publisher-trust entries in `audits.toml`; do not retain
the Git policy after the source-build exception ends.

### Third-party `[[exemptions.*]]` entries

The non-first-party exemptions (cc, cesu8, crypto-common, der, plist, etc.) are crates that haven't been audited by any of the imported audit sources (bytecode-alliance, google, isrg, mozilla) at the versions we currently consume. Each one represents a "this version slipped in unaudited; explicitly trusting it for now" admission. `cargo vet prune` may suggest removing entries that are no longer needed; that's safe to run after an Cargo.lock update.

## When `cargo vet` rewrites `config.toml`

Any invocation of `cargo vet` — including `cargo vet check`, `cargo vet`, `cargo vet certify`, `cargo vet trust` — re-emits `config.toml` in canonical form. The canonical form alphabetizes `[policy.*]` and `[[exemptions.*]]` blocks, drops free-form comments, and normalizes whitespace. This is by design (so `config.toml` diffs are reviewable), but it means in-file documentation cannot live in `config.toml` itself.

**Convention:** all operational guidance about *why* an entry exists lives in this README. Add to it when a new policy or exemption block needs context that isn't self-evident from the entry itself.
