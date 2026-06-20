# ADR-072: Synced-Folder Lifecycle & File Handling Policy

**Status**: Proposed
**Date**: 2026-06-20
**Relates to**: ADR-071 (Subscription-Based Convergence), ADR-025 (Blob Transfer), ADR-047 (Firmware OTA Distribution)

## Executive Summary

The file-drop sync surface delivers files (drop a file in an outbox, it lands in
every interested node's inbox) but says nothing about a file's **lifecycle**.
Three questions have no defined answer today:

1. What happens when a delivered (inbox) or source (outbox) file is **deleted**?
2. What does re-placing the **same content** (checksum-confirmed) mean?
3. Who owns **versioning** — peat-node or the application?

ADR-071 settled *who* converges data (receiver-evaluated need). This ADR settles
*what happens to the file over time*: a **publisher-declared lifecycle/handling
policy** carried on the distribution document — the synced metadata that already
exists per ADR-071 — plus the receiver-side semantics for deletion, idempotent
re-drop, and version ordering.

**Initial implementation is unidirectional**: a watched root is either an outbox
(source) or an inbox (sink). A single container *may* configure both (distinct
roots), making the deployment effectively bidirectional — but **v1 applies no
conflict management**. True bidirectional folder sync (a shared root with
conflict resolution) is explicitly out of scope here and deferred to its own ADR.

## Context

### What exists today

- A file dropped in an outbox root is hashed, ingested into the
  content-addressed blob store, and published as a **`DistributionDocument`**
  that gossips mesh-wide. Interested nodes converge it (ADR-071) and write the
  bytes at `inbox/<relative_path>` (the inbox mirrors the sender's layout).
- The distribution document is the **sender-owned metadata half** — serialized
  as a single JSON scalar at `ROOT.metadata`, written only by the publisher
  (receivers write their own keyed entries under `ROOT.node_statuses`). Wholesale
  replacement of that scalar is therefore contention-free: a publisher state
  change (e.g. a retraction) is a safe Automerge field update, not a race.
- Blobs are **content-addressed and immutable**: new content = new hash = new
  distribution. The document carries `blob_hash`, `blob_size`, `blob_metadata`,
  `collection` (ADR-071), `status` (`"distributing"`, `"cancelled"`, …) and
  `cancelled_at`.

### The gap

The document is an **announcement** ("blob H was published under collection C at
time T"), not a **living file object**. It expresses no lifecycle policy:

- **Deletion self-heals, silently.** The receive watcher's `already_delivered`
  gate keys on the file existing at `inbox/<relpath>`. Delete the inbox file and
  the next sweep re-fetches and rewrites it, because the document still exists
  and the need predicate still says "I lack it." Deleting an outbox file does
  nothing to delivered copies. Neither behavior is declared or configurable.
- **Re-drop semantics are implicit.** Identical content is deduped only
  incidentally (by the size check / outbox `(size, mtime)` dedup), not by an
  explicit content-identity rule.
- **There is no version relationship between distributions.** Content-addressing
  gives immutability and dedup, but not *ordering*: hash H1 and H2 are just two
  blobs; nothing says H2 supersedes H1.

## Decision

### 1. Directionality: unidirectional primitive; bidirectionality is emergent and unmanaged (v1)

A watched root is **single-purpose**:

- an **outbox** (source) — stable files auto-distribute; or
- an **inbox** (sink) — converged blobs land.

The node **never** promotes an inbox file into a new distribution. This is the
rule that prevents echo loops and keeps "who is the source of truth" answerable.

A container **may** configure both an outbox and an inbox (distinct roots),
making the *deployment* bidirectional. In v1 this is **two independent one-way
flows with no conflict resolution**: if the same logical path is written from
both ends, each node ends at last-arrival-wins, with no merge and no
conflict-copy. **Initial implementation is unidirectional.** A shared-root,
conflict-managed bidirectional sync (Syncthing-style) is a distinct design and
is deferred — see Phasing.

### 2. Lifecycle policy is publisher-declared, carried on the document

A `lifecycle` block is added to the sender-owned metadata half (the
`ROOT.metadata` scalar). Because only the publisher writes that scalar, the
policy gossips with the distribution and **every node applies the same
publisher-declared handling**. Policy may be bound per-distribution or
defaulted per-collection. Illustrative shape:

```
lifecycle:
  on_source_delete: Retain | Cascade      # default: Retain
  on_receiver_delete: Optout | Redeliver   # default: Optout
  # version ordering (§5) and future retention hints live here too
```

### 3. Deletion semantics

- **Source (outbox) file deleted → `Retain` (default).** Deleting your outbox
  copy does **not** claw back delivered copies — delivered data belongs to the
  receiver. `Cascade` (opt-in) publishes a **retraction**: the publisher flips
  `status` → `"retracted"` and sets `retracted_at` (built on the existing
  `status`/`cancelled_at` machinery — a safe sender scalar update). A receiver
  observing the retraction deletes its inbox copy. Retraction gossips like any
  field update and is **DDIL-eventual**: an offline node applies it on return.
- **Receiver (inbox) file deleted → `Optout` (default).** A deleted inbox file
  is honored as **local disinterest**: the node records a **local tombstone**
  (keyed on `distribution_id` / `blob_hash`) so the convergence watcher does not
  re-fetch it. This replaces today's implicit always-redeliver. `Redeliver`
  (strict-mirror, opt-in) restores self-heal for apps that want the inbox folder
  to be authoritative.

The receiver tombstone is **node-local** (it expresses *this node's* opt-out) and
does not propagate; it is distinct from the publisher's retraction, which is the
authoritative withdrawal of the distribution itself.

### 4. Idempotent re-drop (content identity)

Dedup identity is the **content hash**, not the path or size.

- Re-placing **byte-identical** content is a **no-op**: the blob is already held,
  so no new distribution is published and no re-fetch occurs.
- Re-placing **different** content at the same logical path is a **new version**
  (§5), not a conflict — directionality (§1) guarantees a single writer per path
  in the unidirectional model, so latest-wins at the path is unambiguous.

This formalizes the dedup on hash (the current size-based `already_delivered`
gate and outbox `(size, mtime)` dedup are tightened to hash identity).

### 5. Versioning: peat-node owns the mechanism, the app owns the policy

The logical object identity is `(collection, relative_path)`. peat-node carries a
**monotonic `version`** for that identity on the distribution document.

- **peat-node owns the mechanism**: assigning/incrementing the version, exposing
  it on the document, the **version-gap need input** for ADR-071 Phase 2 ("I hold
  v3, v4 is published → I need v4"), and latest-wins overwrite.
- **The application owns the policy**: semantic version *meaning*,
  approval/canary gates, rollback decisions, signing/attestation, and
  retention-of-N-old-versions. The document is the **contract surface** between
  the two.

This keeps peat-node's promise intact — *mechanism, not policy* — and is the same
division ADR-047 draws for firmware (the mesh carries the artifact + version; the
orchestration layer decides what to do with it).

### 6. On-disk metadata sidecar (optional, Phase 2)

Mirror the synced metadata + lifecycle policy to a sidecar next to the file
(e.g. `inbox/<path>.peat.json`) so an application watching the folder sees
provenance, handling policy, and version **without a gRPC call** — symmetric on
the outbox (drop a sidecar to declare policy/version for a file). This makes the
synced document's contract visible at the filesystem boundary the file-drop UX
already uses. Optional; deferred to Phase 2.

## Architecture

```
publisher (outbox):
    file stable → hash → publish DistributionDocument{
        blob_hash, collection, version,            # identity + ordering
        lifecycle{on_source_delete, on_receiver_delete, …}  # policy
    }
    outbox file deleted:
        Retain  → no-op (delivered copies stand)
        Cascade → publish retraction (status=retracted, retracted_at)

receiver (inbox), per ADR-071 need:
    converge blob → write inbox/<relative_path> (+ optional sidecar)
    re-drop identical hash      → no-op (already held)
    inbox file deleted:
        Optout    → record local tombstone; watcher stops re-fetching
        Redeliver → self-heal (re-fetch on next sweep)
    observe retraction          → delete inbox copy
    observe higher version       → converge new version, latest-wins overwrite
```

## Phasing

- **Phase 1 — lifecycle policy + deletion + idempotent re-drop.** `lifecycle`
  block on the document; source-delete `Retain`/`Cascade` with publisher
  retraction; receiver-delete `Optout`/`Redeliver` with node-local tombstones;
  hash-based idempotent re-drop. **Unidirectional only.**
- **Phase 2 — versioning + sidecar.** `version` per `(collection, path)` driving
  ADR-071 Phase 2 version-gap need and latest-wins; optional on-disk metadata
  sidecar.
- **Deferred — bidirectional folder sync.** A shared-root, conflict-managed sync
  (conflict-copies or last-writer-wins-with-vector-clocks). Distinct design; its
  own ADR if pursued. v1's "both roots configured" is explicitly *not* this.

## Consequences

**Positive**
- Deletion stops being a silent, unconfigurable self-heal; it becomes a
  publisher-declared, uniformly-applied policy.
- The distribution document becomes the single contract surface for file
  lifecycle, deletion, and version ordering — visible (optionally) on disk.
- Versioning slots directly into ADR-071's pluggable need predicate (Phase 2)
  without a new targeting mode.
- Mechanism/policy split keeps peat-node domain-agnostic; apps layer semantics.

**Negative / risks**
- New persisted node-local state (receiver opt-out tombstones) with its own
  lifecycle (record / clear / GC).
- Retraction + cascade is a destructive remote action; the default (`Retain`)
  is chosen to make destruction strictly opt-in, but operators enabling
  `Cascade` carry fat-finger risk that must be surfaced in tooling.
- "Both roots configured = bidirectional, no conflict management" is a sharp
  edge; it must be documented loudly so it isn't mistaken for real folder sync.
- Version identity keyed on `(collection, relative_path)` assumes a stable path;
  renames present as a delete + new object, which interacts with deletion policy.

## Cross-repo impact

- **peat-protocol / peat-mesh**: `lifecycle` block and `version` on the
  distribution document; `retracted`/`retracted_at` publisher state; receive
  watcher honors lifecycle policy and node-local opt-out tombstones; dedup
  tightened to content-hash identity.
- **Sidecar gRPC surface**: declare lifecycle policy + version on publish
  (per-distribution or per-collection default); subscription surface (ADR-071)
  unchanged.
- **peat-node**: outbox/inbox single-purpose root handling; node-local tombstone
  store; optional on-disk metadata sidecar writer/reader.

Implementation lands as one issue/PR per repo, linked through a tracking issue.
