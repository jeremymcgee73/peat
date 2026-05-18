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

## Hard rule: FIPS-approved cryptographic primitives only

**Every cryptographic algorithm used anywhere in the peat ecosystem must appear on the FIPS 140-3 approved list.** This is procurement-driven (tactical/DoD customers) and non-negotiable. The deployment target is "FIPS-mode aligned" — use only approved algorithms even where the consuming crypto module is not itself 140-3 validated.

**Approved primitives:**

- **AEAD:** AES-256-GCM (or AES-128-GCM). Fresh per-encryption nonce. NIST SP 800-38D.
- **Signatures:** Ed25519 (FIPS 186-5, Feb 2023) or ECDSA-P256/P384.
- **Key agreement:** ECDH on P-256/P-384. X25519 is *marginal* — Curve25519 is approved in SP 800-186 but CMVP module coverage of ECDH-with-X25519 is uneven; treat any X25519 reference as needing explicit review.
- **KDF:** HKDF-SHA-256 / HKDF-SHA-384 (SP 800-56C / SP 800-108).
- **MAC:** HMAC-SHA-256 / HMAC-SHA-384 (FIPS 198-1).
- **Hashes:** SHA-256 / SHA-384 / SHA-512 (FIPS 180-4).
- **TLS / QUIC (Iroh / rustls):** must run under a FIPS-mode provider such as `aws-lc-rs`. The default `ring` backend is **not** FIPS-validated.
- **MLS ciphersuite (ADR-044):** must be a FIPS-aligned suite (e.g. `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`), not the X25519/ChaCha20 suites.

**Explicitly non-approved (do not introduce):**

- **ChaCha20-Poly1305** — IETF RFC 8439, never blessed by NIST. Any new code or doc reference is a constraint violation. Existing references in ADR-006, ADR-044, ADR-048, ADR-049, `README.md`, `docs/spec/005-security.md`, and `docs/whitepaper/10b-spec-appendix.md` predate this rule and are tracked for amendment (see ADR-060 §References → Follow-up amendments).
- Deterministic / order-preserving / homomorphic encryption schemes.
- Any non-NIST primitive (Salsa20, BLAKE2/3 as a primary hash, etc.) without an explicit ADR justifying a deviation.

**If you find yourself proposing or reviewing a primitive that's not on the approved list:** stop. Either it should be on the list (then this rule needs amending via ADR), or the proposal should change. Do not silently introduce a primitive on the basis of "ecosystem convention" — driver #6 of ADR-060 overrides ecosystem convention.

The canonical reference is ADR-060 §5 "Cryptographic primitives (FIPS posture)". When in doubt, consult that section before consulting ADR-006 or ADR-044, since those predate the FIPS rule and currently contain ChaCha20-Poly1305 references queued for amendment.

## Hard rule: no consumer-specific references in peat

**peat is the generic mesh protocol.** Consumers (mobile-app plugins, wearable firmware, CLI tools, server bridges) live in their own repos and depend on peat. peat does NOT reference any specific consumer by name in code, comments, examples, READMEs, operational docs, JNI symbol names, package paths, or test fixtures.

Forbidden references include but are not limited to: vendor names (ATAK, WinTAK, iTAK, WearTAK, etc.), vendor-derived module/file names (e.g. `peat_<vendor>_client.rs`), package-path namespacing that includes a vendor (e.g. `com.defenseunicorns.<vendor>.peat.*`), and prose that says "the X plugin" / "for X" when describing what a generic consumer does.

**Acceptable generic terms:** "consumer", "consumer plugin", "CoT consumer", "mobile-app plugin", "wearable", "CLI tool", "server bridge". When a protocol name is structurally load-bearing (e.g. CoT XML, the TAK Server wire protocol that `peat-transport/src/tak/` bridges to), the *protocol* name may appear; the *consumer* name may not.

**The only places consumer names may appear** are: (1) ADRs (`docs/adr/`) and the whitepaper when citing a real-world use case that motivated a design decision (even there, prefer generic language); (2) `CHANGELOG.md` entries that record the history of vendor-name removals or other archival migration notes — a release-notes file can't usefully describe a rename without naming what was renamed; and (3) genuine third-party identifiers that operational tooling targets verbatim — the host app's actual Android package id (`com.atakmap.*`), its activity classes (`ATAKActivity`), and the sibling repo's actual name (`peat-atak-plugin`). These are not "references to a consumer" in the rule's sense; they are external identifiers that the operational layer literally invokes by string, or archival records. The SKILL.md grep gate excludes them.

If a task in this repo would introduce a consumer reference into code/comments/operational docs, do not write it. Find the generic equivalent or stop and surface the design tension explicitly.

This rule exists because: peat's value as a protocol depends on it being a peer-equal substrate for many consumers, not a bespoke runtime for one vendor. Every consumer-specific identifier that lands in peat couples the substrate to that vendor's roadmap and signals to other potential consumers that the substrate isn't generic.
