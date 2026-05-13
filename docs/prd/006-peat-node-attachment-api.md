---
title: "PRD-006-peat-node-attachment-api: Path-Based Attachment Distribution API"
status: Draft
issue: "defenseunicorns/peat-node#56"
adrs: [018, 019, 025, 037, 043, 046]
---

## Implementation Spec: Expose Path-Based Attachment Submission via peat-node Connect RPC

### Objective

Allow a co-located consumer of peat-node to submit one or more local files for distribution across the mesh by *file path*, and have peat-node ingest, validate, and distribute them to either every reachable node or an explicit destination list — reusing the already-built `FileDistribution` substrate. v1 is path-based only (consumer and peat-node share a filesystem); client-streaming upload is explicitly deferred.

The substrate (blob store, iroh transfer, distribution-metadata sync, scope filtering) already exists. This PRD wires it up to the peat-node RPC surface, adds the validation/config teeth needed to ship it safely, and stubs the hooks for capability-aware targeting in a follow-up.

---

### Current State

**Implemented:**

| Component | Location | Status |
|---|---|---|
| `BlobStore` trait — local-side create/fetch/list | `peat-mesh/src/storage/blob_traits.rs:298–422` | Fully implemented; `create_blob_from_bytes` / `create_blob_from_stream` accept arbitrary content |
| `NetworkedIrohBlobStore` — iroh-blobs P2P transfer | `peat-mesh/src/storage/iroh_blob_store.rs` | Owned by `SidecarNode` (peat-node/src/node.rs:44); content-addressed, streaming, resumable |
| `FileDistribution` trait — distribution API | `peat-protocol/src/storage/file_distribution.rs:307–375` | `distribute(blob_token, scope, priority)`, `status`, `cancel`, `wait_for_completion`, `subscribe_progress` |
| `DistributionScope` — targeting enum | `peat-protocol/src/storage/file_distribution.rs:98–129` | `AllNodes` ✅, `Nodes { node_ids }` ✅, `Formation { formation_id }` ⚠️ TODO, `Capable { … }` ⚠️ TODO |
| `IrohFileDistribution` implementation | `peat-protocol/src/storage/file_distribution.rs:412–706` | Stores distribution doc in `file_distributions` collection (line 388); filters peers by scope (lines 463–477) |
| Distribution-doc sync via Automerge | implicit through `AutomergeSyncCoordinator` | Distribution status syncs as a regular CRDT document |
| Connect/gRPC surface | `peat-node/proto/sidecar.proto`, `peat-node/src/service.rs` | Service runs on UDS or TCP per `--listen`; proto compiled via `connectrpc_build` (build.rs) |

**Not wired (the gap):**

1. `sidecar.proto` has no attachment RPCs — only document CRUD + typed collections.
2. peat-node has no ingest path that reads a local file, hashes/validates it, calls `BlobStore::create_blob_from_stream`, then `FileDistribution::distribute`.
3. No allowlist/sandbox for caller-supplied filesystem paths — without one, the RPC becomes a file-read oracle for anyone who can talk to the socket.
4. No size/bundle limits, no concurrency caps, no per-distribution priority knob exposed.
5. Receive-side: `file_distribution.rs:620–621` flags TODO for observer-driven status updates on the receiver; out of scope for v1 but called out under "Future Work" below.
6. `DistributionScope::Capable` has no defined capability vocabulary yet. The two candidate dimensions — device-runtime (storage / GPU memory / CPU arch) and model-capability (ADR-018: precision / inference latency / model version) — target different things, and "which one does `Capable` mean for attachments" is itself an open architectural question. v1 ships an empty `CapableScope` marker and rejects the variant; the schema is deferred to a follow-on ADR. Ditto `Formation` lookup, which awaits a live formation-membership data source.

---

### API Contract

#### Proto additions (`peat-node/proto/sidecar.proto`)

```proto
service PeatSidecar {
  // ... existing RPCs ...

  // --- Attachments ---

  // SendAttachments ingests one or more local files and queues them for
  // distribution to the specified scope. Each file is validated (size, hash,
  // path inside an allowlisted root) before any blob is created.
  // Returns synchronously with handles; transfer happens asynchronously.
  rpc SendAttachments(SendAttachmentsRequest) returns (SendAttachmentsResponse);

  // GetAttachmentDistribution returns the current status of a single
  // distribution by its ID.
  rpc GetAttachmentDistribution(GetAttachmentDistributionRequest)
      returns (GetAttachmentDistributionResponse);

  // SubscribeAttachmentBundle streams progress updates for every file in a
  // bundle until all distributions reach a terminal state or the client
  // disconnects.
  //
  // Late-subscribe semantics:
  //   * All-terminal at subscribe time (every distribution already in
  //     COMPLETED / FAILED / CANCELLED): the server emits exactly one
  //     AttachmentProgress snapshot per distribution carrying that terminal
  //     state, then closes the stream.
  //   * Mixed state at subscribe time (some distributions already terminal,
  //     others still PENDING / IN_PROGRESS): the server emits one snapshot
  //     for each already-terminal distribution first (so the client never
  //     silently misses a distribution that completed before subscribe),
  //     then streams live progress updates for the in-flight distributions,
  //     then closes the stream once every distribution has reached a
  //     terminal state.
  //   * All-live at subscribe time (no terminal distributions yet): live
  //     streaming only — no synthetic snapshots — until terminal close.
  // This contract makes the RPC useful for crash-recovery: a client can
  // re-attach by bundle_id and reliably learn the final outcome of every
  // distribution without racing the producer.
  rpc SubscribeAttachmentBundle(SubscribeAttachmentBundleRequest)
      returns (stream AttachmentProgress);

  // CancelAttachmentDistribution requests cancellation of an in-flight
  // distribution. Best-effort; already-transferred bytes on receivers are
  // not rolled back.
  rpc CancelAttachmentDistribution(CancelAttachmentDistributionRequest)
      returns (CancelAttachmentDistributionResponse);
}

message FileSpec {
  // Name of an allowlisted root configured on peat-node (see Configuration).
  string root_name = 1;
  // Path relative to that root. Must not start with "/" or contain "..".
  string relative_path = 2;
  // Expected file size in bytes. Must match on-disk size exactly.
  uint64 size_bytes = 3;
  // Expected sha256 of file contents — exactly 32 raw bytes (NOT hex-encoded).
  // A `bytes` field of any other length is rejected with INVALID_ARGUMENT
  // before the file is opened.
  bytes sha256 = 4;
  // Optional MIME type, forwarded to receivers in the distribution doc.
  optional string content_type = 5;
  // Optional display name presented to receivers; defaults to relative_path's basename.
  optional string display_name = 6;
}

message DistributionScopeSpec {
  oneof scope {
    AllNodesScope all_nodes = 1;
    NodeListScope node_list = 2;
    FormationScope formation = 3;
    // Capable scope is a reserved-but-rejected variant in v1: an empty
    // CapableScope marker; the capability vocabulary itself is deferred
    // to a follow-on ADR (see Future Work #2). Senders may include it
    // but peat-node will always reject it with FAILED_PRECONDITION in v1.
    CapableScope capable = 4;
  }
}

message AllNodesScope {}
message NodeListScope { repeated string node_ids = 1; }
message FormationScope { string formation_id = 1; }
// CapableScope is an intentionally-empty reserved variant in v1. The
// capability vocabulary (device-runtime vs. model-capability axes, and
// the exact predicate fields) is deferred to a follow-on ADR — pinning
// fields into the wire format before that decision would force either
// a breaking migration or a parallel CapableV2Scope when the schema
// lands. v1 senders MAY include CapableScope; peat-node always
// rejects it with FAILED_PRECONDITION. The marker exists so the oneof
// can grow without renumbering when the ADR ships.
message CapableScope {}

// AttachmentPriority records the QoS classification of an attachment
// bundle. Each value maps 1:1 onto peat-protocol::qos::QoSClass so the
// full five-tier vocabulary is reachable from the attachment surface —
// in particular LOW (P4) is exposed explicitly because peat-protocol's
// aging-promotion path is `Bulk → Low → Normal` (sync_queue.rs:118-119)
// and collapsing P4 out of the wire surface would either skip the
// intermediate aging tier or silently drop the aging promotion.
//
// v1-honesty: peat-node records the classification on the distribution
// document and uses it for ordering within its own queues, but does NOT
// enforce wire-level preemption between classes — a CRITICAL bundle
// will not actually pause an in-flight BULK transfer in v1. Cross-class
// preemption requires PRD-004 (Bandwidth Allocation to Sync Transport),
// which is the documented dependency in Future Work #5 of this PRD.
// Parallels the v1-honesty pattern on DistributionStatus.PARTIAL and
// NodeTransferState.
enum AttachmentPriority {
  ATTACHMENT_PRIORITY_UNSPECIFIED = 0;
  ATTACHMENT_PRIORITY_BULK = 1;     // QoSClass::Bulk (P5) — opportunistic; aging-eligible source
  ATTACHMENT_PRIORITY_LOW = 2;      // QoSClass::Low (P4)  — intermediate aging tier between Bulk and Normal
  ATTACHMENT_PRIORITY_ROUTINE = 3;  // QoSClass::Normal (P3) — default
  ATTACHMENT_PRIORITY_PRIORITY = 4; // QoSClass::High (P2)  — preempts BULK / LOW once PRD-004 lands (v2)
  ATTACHMENT_PRIORITY_CRITICAL = 5; // QoSClass::Critical (P1) — preempts everything once PRD-004 lands (v2)
}

message SendAttachmentsRequest {
  // 1..=max_files_per_bundle entries (config-bounded).
  repeated FileSpec files = 1;
  // Required. An unset oneof (or an omitted `scope` field, which decodes
  // to a default-constructed `DistributionScopeSpec` with no variant set)
  // is rejected with INVALID_ARGUMENT — there is no silent fallback to
  // AllNodes. See Validation Rule 10.
  DistributionScopeSpec scope = 2;
  // Defaults to ROUTINE when unspecified.
  AttachmentPriority priority = 3;
  // Optional client-supplied bundle ID for idempotency. If omitted, peat-node
  // assigns a UUIDv4. Re-submitting the same bundle_id with an identical
  // FileSpec set (same fields, same order) is a no-op that returns the
  // existing handles; re-submitting with any deviation is rejected
  // ALREADY_EXISTS. See Validation Rule 12.
  optional string bundle_id = 4;
}

message SendAttachmentsResponse {
  string bundle_id = 1;
  repeated AttachmentHandle handles = 2;
}

message AttachmentHandle {
  // Index into the request's `files` list — matches caller's submission order.
  uint32 file_index = 1;
  // iroh blob token (BLAKE3-based content address). Stable across retries.
  string blob_token = 2;
  // Distribution-doc ID for status lookup / subscription.
  string distribution_id = 3;
}

message GetAttachmentDistributionRequest { string distribution_id = 1; }
message GetAttachmentDistributionResponse {
  DistributionStatus status = 1;
  uint64 bytes_transferred = 2;
  uint64 bytes_total = 3;
  repeated NodeTransferState per_node = 4;
  optional string error = 5;
}

// NodeTransferState reflects the sender's outbound view of a transfer in v1.
// Because iroh-blobs is pull-based, `status` records whether the targeted
// peer connected to fetch the blob and how many bytes the sender served —
// not whether the receiver's local write succeeded. Receive-side completion
// reporting requires the observer hooks deferred to v2 (see Future Work).
message NodeTransferState {
  string node_id = 1;
  DistributionStatus status = 2;
  uint64 bytes_transferred = 3;
}

// DistributionStatus is the sender-side view of a distribution in v1.
// DISTRIBUTION_STATUS_COMPLETED means every targeted peer connected and
// pulled all bytes from this sender. DISTRIBUTION_STATUS_PARTIAL is
// reserved for v2 — until receive-side observer hooks land it will not
// fire reliably from sender-side state alone, so v1 implementations
// should emit COMPLETED on full sender-side success and FAILED on
// explicit transfer failure, leaving PARTIAL for the v2 contract.
enum DistributionStatus {
  DISTRIBUTION_STATUS_UNSPECIFIED = 0;
  DISTRIBUTION_STATUS_PENDING = 1;
  DISTRIBUTION_STATUS_IN_PROGRESS = 2;
  DISTRIBUTION_STATUS_COMPLETED = 3;
  DISTRIBUTION_STATUS_PARTIAL = 4;   // RESERVED — see Future Work #4 (v2)
  DISTRIBUTION_STATUS_FAILED = 5;
  DISTRIBUTION_STATUS_CANCELLED = 6;
}

message SubscribeAttachmentBundleRequest { string bundle_id = 1; }
message AttachmentProgress {
  string distribution_id = 1;
  string blob_token = 2;
  DistributionStatus status = 3;
  uint64 bytes_transferred = 4;
  uint64 bytes_total = 5;
  optional NodeTransferState changed_node = 6;
  optional string error = 7;
}

message CancelAttachmentDistributionRequest { string distribution_id = 1; }
message CancelAttachmentDistributionResponse { bool was_cancelled = 1; }
```

Map `AttachmentPriority` 1:1 onto `peat-protocol::qos::QoSClass`: `BULK→Bulk` (P5), `LOW→Low` (P4), `ROUTINE→Normal` (P3), `PRIORITY→High` (P2), `CRITICAL→Critical` (P1). Exposing all five tiers (including P4/`Low`) keeps the wire surface aligned with the aging-promotion path `Bulk → Low → Normal` defined in `peat-protocol/src/qos/sync_queue.rs:118-119`; collapsing P4 would silently break the intermediate aging tier. Map `DistributionScopeSpec` → existing `peat-protocol::storage::file_distribution::DistributionScope`.

---

### Validation Rules

All validation must pass for **every** file in the bundle before *any* blob is created. The request fails atomically — partial ingestion is not allowed.

1. **Bundle size**: `request.files.len() ∈ [1, max_files_per_bundle]` (config).
2. **Bundle bytes**: `Σ size_bytes ≤ max_bundle_bytes` (config).
3. **Root name**: `root_name` must exist in the configured allowlist.
4. **Relative path safety**:
   - Must not be empty.
   - Must not start with `/`.
   - Must not contain `..` as a path component (string check *and* re-check after canonicalisation).
5. **Path resolution and TOCTOU-safe open**: `canonicalise(root + relative_path)` must still be a descendant of `canonicalise(root)`. Once the descendant check passes, the file must be opened with `O_NOFOLLOW` (and, where supported, via `openat` against an `O_PATH` directory file descriptor anchored at the canonicalised root) so that no symlink is traversed at open time and no path component can be swapped between canonicalisation and open. Without this, an attacker with write access to a path component could swap a directory for a symlink after Rule 5's check passes and cause an out-of-root read before the post-stream hash check (Rule 9) would catch a content swap.
6. **File metadata**: file must exist, be a regular file (not a directory, symlink to non-regular, FIFO, device), be readable.
7. **Size match**: on-disk size == `size_bytes` exactly.
8. **Per-file size cap**: `size_bytes ≤ max_file_bytes` (config).
9. **Hash format and match**: the `sha256` field length must be exactly 32 bytes (raw, not hex-encoded) — any other length is rejected `INVALID_ARGUMENT` with a `files[i].sha256` field path in the error detail before the file is opened. Then streaming sha256 of file contents == request's `sha256`, computed in the same pass as blob ingest (single read).
10. **Scope sanity**:
    - **Unset scope** (the `oneof scope` is empty — proto3's wire default for an unset `DistributionScopeSpec`, including the case where the caller omits `scope` on `SendAttachmentsRequest` entirely): reject `INVALID_ARGUMENT` with a `scope` field path. No silent fallback to `AllNodes` — a caller bug that drops the scope must not result in a 1 GiB bundle being fanned out to every reachable peer.
    - `AllNodes`: always accepted.
    - `NodeList`: `node_ids.len() ∈ [1, max_node_list_len]`; unknown node IDs are *not* a request-time error (a node may not yet be known to this peat-node) — they are recorded and surface in per-node status as `FAILED` after a configurable peer-discovery grace period.
    - `Formation`: if `formation_id` is not resolvable, fail `FAILED_PRECONDITION` (v1 — no async resolution).
    - `Capable`: always rejected `FAILED_PRECONDITION` in v1.
11. **Concurrency cap**: if the node already has `max_concurrent_distributions` in flight, reject `RESOURCE_EXHAUSTED` (or queue, if `attachment.queue_when_full` is true — config-controlled, default false).
12. **Idempotency / bundle-ID conflict**: if `bundle_id` is supplied and matches an already-known bundle still resident in the handle table (see "Bundle-table retention" in Configuration), behavior depends on the bundle's current `DistributionStatus` and whether the request payload matches:
    - Bundle status `PENDING` | `IN_PROGRESS` | `COMPLETED`:
      - **Identity check fields:** `root_name`, `relative_path`, `size_bytes`, `sha256`, for every file, in the same order. The optional metadata fields `content_type` and `display_name` are *not* part of the identity check — a resubmit that adds, removes, or changes either is still treated as identical. The original ingest's `content_type` / `display_name` values are retained; the resubmit does not overwrite them. Rationale: `sha256` already canonicalises content identity, optional metadata is presentational, and retry-on-RPC clients that re-marshal a request commonly populate optional fields they previously omitted — punishing that path with `ALREADY_EXISTS` would defeat the field's idempotency purpose.
      - Same `bundle_id` + identical identity-check fields → return the existing handles without re-reading files or creating new blobs. True idempotent no-op.
      - Same `bundle_id` + any deviation in the identity-check fields (different file count, different ordering, or any identity-field mismatch on any file) → reject `ALREADY_EXISTS` with the conflicting `bundle_id` and its `created_at` in the error detail. Prevents a caller from squatting on, silently rebinding, or overwriting an active bundle's handle table.
    - Bundle status `FAILED` | `CANCELLED`:
      - Accept the new request as a fresh ingest with the same `bundle_id`. The prior terminal handle table is replaced and the prior `distribution_id`s are dropped from lookup. This lets a consumer retry the same logical bundle after a transient failure without having to mint a new `bundle_id` (which would defeat their own audit/correlation use of the field). Subscribers attached to the prior terminal state are not migrated to the new bundle.
    - If `bundle_id` does not match any resident bundle (either never seen or evicted per the retention policy), it is treated as a fresh request.

Errors map to standard gRPC codes:
- Path/size/hash-length/hash-mismatch/root failures and unset `scope`: `INVALID_ARGUMENT` with a field path in the error detail.
- Size/concurrency caps: `RESOURCE_EXHAUSTED`.
- Unsupported scope (`Capable` in v1) and unresolvable `Formation`: `FAILED_PRECONDITION`.
- Bundle-ID reuse with different `FileSpec` set: `ALREADY_EXISTS`.
- Lookup against an unknown ID — `GetAttachmentDistribution(distribution_id=…)`, `SubscribeAttachmentBundle(bundle_id=…)`, `CancelAttachmentDistribution(distribution_id=…)`: `NOT_FOUND` with the offending `bundle_id` or `distribution_id` in the error detail. Applies uniformly across the three read/cancel/subscribe RPCs so downstream consumers can pattern-match a single code.
- Internal blob-store / sync failures: `INTERNAL`.

---

### Configuration

peat-node's CLI today is clap-with-env-vars (no config file). Following that convention, add the following flags to `peat-node/src/main.rs`'s `Args` (all `PEAT_NODE_ATTACHMENT_*`):

| Flag / env | Type | Default | Meaning |
|---|---|---|---|
| `--attachment-root` / `PEAT_NODE_ATTACHMENT_ROOT` | `name=path` (repeatable, comma-delimited) | *empty (RPC disabled)* | Allowlisted roots, e.g. `outbox=/var/lib/peat/outbox,media=/var/lib/peat/media` |
| `--attachment-max-file-bytes` / `..._MAX_FILE_BYTES` | `u64` | `268435456` (256 MiB) | Per-file hard cap |
| `--attachment-max-bundle-bytes` / `..._MAX_BUNDLE_BYTES` | `u64` | `1073741824` (1 GiB) | Per-request hard cap |
| `--attachment-max-files-per-bundle` / `..._MAX_FILES_PER_BUNDLE` | `u32` | `64` | Per-request file count cap |
| `--attachment-max-node-list-len` / `..._MAX_NODE_LIST_LEN` | `u32` | `256` | Cap on `NodeListScope.node_ids.len()` |
| `--attachment-max-concurrent-distributions` / `..._MAX_CONCURRENT_DISTRIBUTIONS` | `u32` | `4` | In-flight cap |
| `--attachment-queue-when-full` / `..._QUEUE_WHEN_FULL` | `bool` | `false` | If true, accept and queue beyond the in-flight cap; else reject |
| `--attachment-default-priority` / `..._DEFAULT_PRIORITY` | `Bulk\|Low\|Routine\|Priority\|Critical` | `Routine` | Default `AttachmentPriority` when unspecified |
| `--attachment-discovery-grace-secs` / `..._DISCOVERY_GRACE_SECS` | `u32` | `30` | Grace window for unknown node IDs in `NodeListScope` before they're marked `FAILED` |
| `--attachment-handle-retention-secs` / `..._HANDLE_RETENTION_SECS` | `u32` | `86400` (24h) | How long a terminal bundle's handle table is retained for `bundle_id` lookups, `SubscribeAttachmentBundle` late-attach, and `ALREADY_EXISTS` enforcement. After this window the bundle is evicted from the lookup table; a resubmit of an evicted `bundle_id` is treated as fresh. `0` disables retention entirely (no idempotency, no late-subscribe) — discouraged. |
| `--attachment-max-known-bundles` / `..._MAX_KNOWN_BUNDLES` | `u32` | `4096` | Hard cap on the handle-table size. When exceeded, LRU eviction kicks in even before the retention window expires. Protects long-running edge nodes from unbounded growth proportional to lifetime send volume. |

**Safety default:** if no `--attachment-root` is configured, all four attachment RPCs return `UNIMPLEMENTED`. This makes "RPC exposed but unsafe" impossible by default — operators must consciously opt in by naming the roots that may be read.

**Bundle-table retention tradeoff:** `--attachment-handle-retention-secs` directly controls how long `ALREADY_EXISTS` can fire against a completed `bundle_id` and how long a client can crash-recover via `SubscribeAttachmentBundle`. Shorter window = lower memory, weaker idempotency / recovery guarantee; longer window = stronger guarantee, more memory. Default 24h trades off for the rPi/Jetson edge-node case where a long-running peat-node accumulates state proportional to lifetime send volume.

**Handle-table durability across restarts:** the handle table is **in-memory only** in v1 — `Arc<DashMap<bundle_id, BundleRecord>>` per Step 3, no persistence layer. A peat-node restart (operator stop, OTA push, crash) drops every `bundle_id` lookup. Post-restart consequences:

- `SubscribeAttachmentBundle(bundle_id)` returns `NotFound` for any `bundle_id` whose subscriber re-attaches after a *server-side* restart, even within the retention window. The late-subscribe "crash-recovery" contract in the `SubscribeAttachmentBundle` doc-comment covers *client* crashes, not server restarts.
- `ALREADY_EXISTS` enforcement resets. A `bundle_id` ingested before the restart can be resubmitted with a different `FileSpec` set immediately after.
- All other state — Iroh content-addressed blobs, in-flight distribution documents synced via Automerge — is unaffected by the handle-table loss; the table only governs the `bundle_id → BundleRecord` lookup.

Consumers should not build durable-bundle assumptions against this surface. If v2 needs durable handle tables (persistence + recovery on startup + retention semantics across restart), that lands as a separate spec addition; the v1 contract is explicit so the limitation isn't discovered in production.

Internal config struct lives in `peat-node/src/service.rs` (or a new `peat-node/src/attachments/config.rs`) and is plumbed into the service like the existing `SidecarConfig`.

---

### Implementation Steps

#### Step 1: Proto + generated code

**Files:** `peat-node/proto/sidecar.proto`, regenerated by `build.rs`

Add the messages and RPCs from the API Contract section. `cargo build` re-runs `connectrpc_build`.

**Estimated:** ~200 proto lines, generated code is free.

#### Step 2: Attachment service module

**File:** `peat-node/src/attachments/mod.rs` (new), `peat-node/src/attachments/config.rs` (new), `peat-node/src/attachments/validate.rs` (new), `peat-node/src/attachments/ingest.rs` (new)

Layout:
- `config::AttachmentConfig` — owns the parsed root allowlist (`HashMap<String, PathBuf>` with canonicalised paths) and all caps. Built from `Args` in `main.rs`.
- `validate::validate_request(req, cfg)` — runs all twelve rules above against a `SendAttachmentsRequest`. Returns `Result<ValidatedBundle, tonic::Status>` where `ValidatedBundle` carries resolved absolute paths.
- `ingest::ingest_bundle(validated, blob_store, file_distribution, scope, priority)` — for each file: check `blob_exists_locally(expected_token)` first; if present, treat as a pre-existing blob and *do not* register it for rollback. Otherwise open the file with `O_NOFOLLOW`, stream into a `sha256` hasher *and* `create_blob_from_stream` in the same pass, verify the hash post-stream (else delete the just-created blob and bail), then call `FileDistribution::distribute` and collect handles. Atomic-on-failure: on abort, best-effort delete *only the blobs this request newly created* — never delete a pre-existing blob token, because iroh-blobs is content-addressed and the same token may already be referenced by another live distribution. Rolling that back would pull the rug out from under unrelated work.
- `mod` re-exports the service handlers.

The single-pass hash+ingest is the perf-critical bit. Use `tokio::io::AsyncReadExt` over the file, push each chunk into both `Sha256::update` and the blob-store stream sink. A `tee`-style wrapper avoids double-reading large files.

**Estimated:** ~400 lines new.

#### Step 3: RPC handlers in service.rs

**File:** `peat-node/src/service.rs`

Implement the four new RPC methods on the existing `PeatSidecarService`. Each delegates to the `attachments::` module. The bundle subscription handler:
- Looks up all distribution_ids for the bundle (held in an `Arc<DashMap<bundle_id, BundleRecord>>` populated by `SendAttachments`, where `BundleRecord` carries the `Vec<distribution_id>`, the `FileSpec` set for the conflict check, a `created_at`, and a `last_touched_at` used by the retention/LRU eviction loop).
- For each distribution, subscribes to `FileDistribution::subscribe_progress`.
- Multiplexes the resulting streams into a single `tokio::sync::mpsc` channel that backs the server-streaming response.
- Terminates when every distribution reaches a terminal state (`Completed`, `Partial`, `Failed`, `Cancelled`).

**Estimated:** ~250 lines.

#### Step 4: Wire into SidecarNode and Args

**File:** `peat-node/src/node.rs`, `peat-node/src/main.rs`

- Add `attachment_config: AttachmentConfig` to `SidecarConfig`.
- In `SidecarNode::new`, build a `FileDistribution` (`IrohFileDistribution`) from the existing `NetworkedIrohBlobStore` + `AutomergeSyncCoordinator` and store it as a new field.
- In `main.rs`, parse the new CLI flags and pass through.

**Estimated:** ~80 lines modified.

#### Step 5: Helm chart + Zarf manifest

**Files:** `peat-node/chart/peat-node/values.yaml`, `peat-node/chart/peat-node/templates/deployment.yaml`, `peat-node/zarf.yaml`

- Expose the new env vars under `attachment:` in `values.yaml` (default empty roots → RPC disabled).
- Add a volume mount example for `outbox` so operators can see the wiring.
- Zarf manifest mirrors.

**Estimated:** ~40 YAML lines.

---

### Testing Plan

#### Unit Tests (`peat-node/src/attachments/*`)

1. `validate_rejects_unknown_root` — `root_name=missing` ⇒ `InvalidArgument`.
2. `validate_rejects_absolute_relative_path` — `relative_path=/etc/passwd` ⇒ `InvalidArgument`.
3. `validate_rejects_parent_traversal` — `relative_path=../../etc/passwd` ⇒ `InvalidArgument`.
4. `validate_rejects_symlink_escape` — create a symlink inside the root pointing outside; ⇒ `InvalidArgument`.
5. `validate_rejects_size_mismatch` — `size_bytes` ≠ on-disk size ⇒ `InvalidArgument`.
6. `validate_rejects_size_cap` — file > `max_file_bytes` ⇒ `ResourceExhausted`.
7. `validate_rejects_bundle_cap` — Σ sizes > `max_bundle_bytes` ⇒ `ResourceExhausted`.
8. `validate_rejects_too_many_files` — > `max_files_per_bundle` ⇒ `ResourceExhausted`.
9. `validate_rejects_capable_scope_v1` — empty `CapableScope` ⇒ `FailedPrecondition`. Asserts the v1 reserved-but-rejected contract regardless of whether the variant carries any payload.
10. `ingest_hash_mismatch_cleans_up_blob` — supply wrong sha256; verify the partial blob is deleted before the error returns.
11. `ingest_atomic_on_partial_failure` — 3-file bundle where file 2 fails validation; verify file 1's blob is also deleted.
12. `idempotent_resubmit_same_bundle` — same `bundle_id` + identical file set ⇒ same handles, no second ingest.
13. `bundle_id_reuse_with_different_files_rejected` — submit `bundle_id=X` with file set A; resubmit `bundle_id=X` with file set B (different size_bytes on one file). Assert `AlreadyExists` and that bundle A's distributions are untouched.
14. `validate_rejects_wrong_length_sha256` — `sha256` field of 16 bytes and 64 bytes (a hex-encoded sha256 string would be 64 ASCII bytes); both ⇒ `InvalidArgument` with `files[0].sha256` in the field path, no file open attempted.
15. `validate_opens_with_nofollow` — create a regular file inside the root and a symlink inside the root pointing at it; resolve the symlink path through the request. The descendant check passes (target is in-root), but the open must still fail because `O_NOFOLLOW` refuses the symlink traversal at open time. Asserts the v1 TOCTOU mitigation is in place, not just the canonicalisation check.
16. `validate_rejects_unset_scope` — submit a `SendAttachmentsRequest` whose `scope` oneof is unset (and separately one that omits the field entirely). Both ⇒ `InvalidArgument` with `scope` in the field path. Asserts that the validator never silently treats unset as `AllNodes`.
17. `bundle_id_terminal_state_allows_reuse_with_different_files` — submit `bundle_id=X` with file set A and force it to `FAILED` (e.g. via cancellation or transport-error fault injection). Resubmit `bundle_id=X` with file set B. Assert: accepted as a fresh bundle (not `AlreadyExists`), new handles returned, prior `distribution_id`s no longer resolvable via `GetAttachmentDistribution`. Then repeat against a `CANCELLED` bundle. Locks in the Rule 12 terminal-state branch.
18. `rollback_preserves_pre_existing_blob_tokens` — pre-populate the blob store with content C (token `T`). Submit a 2-file bundle where file 1 has content C (so its content-address resolves to the existing `T`) and file 2 fails validation (e.g. wrong sha256), triggering rollback. Assert: bundle aborts; `blob_exists_locally(T)` is *still true* after rollback; only blobs newly created by this request were deleted. Locks in content-address rollback safety.
19. `idempotent_resubmit_ignores_optional_metadata_changes` — submit `bundle_id=X` with `FileSpec{content_type=None, display_name=None}` and let it complete. Resubmit `bundle_id=X` with the same identity-check fields but `content_type="application/pdf"` and `display_name="report.pdf"` populated. Assert: response returns the existing handles (no `AlreadyExists`, no new ingest), and a subsequent `GetAttachmentDistribution` shows the *original* (None) metadata — the resubmit did not overwrite. Locks in the Rule 12 "optional metadata not in identity check, original values retained" semantics.

#### Integration Tests (`peat-node/tests/`)

20. `attachments_disabled_when_no_root` — service started with empty allowlist returns `Unimplemented` for all four RPCs.
21. `send_all_nodes_distributes_to_two_peers` — three-node test cluster, send 1 MiB file with `AllNodesScope`. Poll `GetAttachmentDistribution` until the sender reports `COMPLETED`. Then directly inspect each receiver node's `NetworkedIrohBlobStore`: assert `blob_exists_locally(token)` is true and that a `fetch_blob(token)` round-trip produces bytes whose sha256 matches the source. The receiver-side blob-store assertion is independent of v2's receive-side observer hooks and tightens the v1 acceptance signal beyond sender-side state alone.
22. `send_node_list_only_delivers_to_listed` — three-node cluster, scope = `NodeList{[node_b]}`. After sender reports `COMPLETED`, directly assert on `node_b`'s blob store that `blob_exists_locally(token)` is true and the fetched content hashes correctly; assert on `node_c`'s blob store that `blob_exists_locally(token)` is false. Same independence rationale as test 21.
23. `subscribe_emits_progress_then_terminal` — send 4 MiB file, subscribe, assert at least one `IN_PROGRESS` frame and exactly one terminal frame.
24. `cancel_in_flight_stops_transfer` — start a large transfer, cancel mid-flight, assert status flips to `CANCELLED` within 1 s.
25. `unknown_node_id_marked_failed_after_grace` — `NodeList{[nonexistent]}`; assert that after `discovery_grace_secs`, per-node status is `FAILED`.
26. `concurrent_cap_returns_resource_exhausted` — `max_concurrent_distributions=2`, fire three in parallel, assert the third gets `ResourceExhausted` (with `queue_when_full=false`).
27. `lookup_unknown_ids_return_not_found` — three sub-cases against a freshly-started service with no bundles yet: `GetAttachmentDistribution(distribution_id="missing")`, `SubscribeAttachmentBundle(bundle_id="missing")`, `CancelAttachmentDistribution(distribution_id="missing")`. Each must return `NotFound` (not `InvalidArgument`, `Internal`, or empty success) with the offending ID in the error detail.
28. `subscribe_after_terminal_emits_snapshot_then_eof` — send a 2-file bundle and wait until both distributions reach a terminal state (one `COMPLETED`, one driven to `FAILED` via injected transport error). Then call `SubscribeAttachmentBundle(bundle_id)`. Assert: the stream emits exactly two `AttachmentProgress` frames (one per distribution, each carrying the terminal `DistributionStatus`) and then closes cleanly — no `FAILED_PRECONDITION`, no empty stream, no hang. Locks in the late-subscribe contract for crash-recovery patterns.
29. `subscribe_mixed_state_emits_snapshot_for_terminal_then_live_for_inflight` — send a 2-file bundle. Use fault injection to drive distribution A to a terminal state (`FAILED`) while distribution B is still `IN_PROGRESS` (e.g. throttle B's transport). Then call `SubscribeAttachmentBundle(bundle_id)`. Assert ordering: the stream emits (i) exactly one `AttachmentProgress` snapshot for distribution A carrying `FAILED`, (ii) one or more live `IN_PROGRESS` / terminal frames for distribution B until it reaches terminal, (iii) a clean stream close. Distribution A's terminal state must never be silently dropped. Locks in the mixed-state branch of the late-subscribe contract.
30. `evicted_bundle_id_treated_as_fresh_request` — start the service with `--attachment-max-known-bundles=1`. Ingest bundle X (`bundle_id=X`, files A) and drive it to a terminal state. Ingest a second bundle Y (`bundle_id=Y`, files B) — this forces eviction of X under the LRU policy. Resubmit `bundle_id=X` with files C (different from A). Assert: the resubmit is accepted as a fresh bundle (new `distribution_id`s returned, no `AlreadyExists`), and `GetAttachmentDistribution(distribution_id=<X's original distribution_id>)` now returns `NotFound`. Verifies the eviction-then-resubmit branch of Rule 12 against the live LRU, not just the in-memory data structure.

#### Cross-cutting tests

31. Any test that exercises distribution status read-back **must** use the real `AutomergeBackend` via `peat-node`'s standard construction path, not `InMemoryBackend`. The publish/scan asymmetry in `InMemoryBackend` hides bugs (an entry written via `publish` may not appear in a subsequent `scan` against the same backend instance) that surface in production where every node runs on `AutomergeBackend`. Tests that exercise `SendAttachments → GetAttachmentDistribution` or `SendAttachments → SubscribeAttachmentBundle` paths must therefore be wired against the real backend, not its in-memory fake.

---

### Acceptance Criteria

- [ ] `SendAttachments` accepts a multi-file request, validates every file atomically, creates iroh blobs, and starts distributions in a single RPC round-trip.
- [ ] Validation rejects path traversal, symlink escape, unknown roots, size mismatch, hash mismatch, and oversized bundles with appropriate gRPC codes.
- [ ] With no `--attachment-root` configured, all four attachment RPCs return `Unimplemented`.
- [ ] `AllNodesScope` delivers to every reachable mesh peer; `NodeListScope` delivers only to listed peers.
- [ ] `SubscribeAttachmentBundle` emits progress updates and a terminal frame for every distribution in the bundle.
- [ ] Hash mismatch on ingest leaves no orphaned blobs (`blob_exists_locally` returns false for the token).
- [ ] Concurrent distribution cap is enforced; over-cap requests return `ResourceExhausted` (or queue iff `queue_when_full=true`).
- [ ] Idempotent re-submission with the same `bundle_id` returns existing handles without re-reading files; re-submission with a deviating `FileSpec` set on a known `bundle_id` returns `AlreadyExists`.
- [ ] `sha256` field of any length other than 32 raw bytes is rejected `InvalidArgument` before any file is opened.
- [ ] After Rule 5's descendant check passes, file opens use `O_NOFOLLOW` (or equivalent), and a swap of an in-root path component for a symlink between canonicalise and open fails the open rather than reading the symlink target.
- [ ] Proto doc-comments on `DistributionStatus` and `NodeTransferState` explicitly note the v1 sender-side-view limitation; `DISTRIBUTION_STATUS_PARTIAL` is documented as reserved for v2 and is not emitted by v1 sender state machines.
- [ ] `SendAttachments` with an unset `scope` oneof (or `scope` omitted entirely) returns `InvalidArgument` with `scope` in the field path — no silent fallback to `AllNodes`.
- [ ] `GetAttachmentDistribution`, `SubscribeAttachmentBundle`, and `CancelAttachmentDistribution` each return `NotFound` (with the offending ID in the error detail) when called with an unknown `distribution_id` / `bundle_id`.
- [ ] A `bundle_id` whose current state is `FAILED` or `CANCELLED` is reusable: resubmit with the same or different `FileSpec` set is accepted as fresh and replaces the prior terminal handle table.
- [ ] `SubscribeAttachmentBundle` on a bundle whose distributions have all reached terminal state emits exactly one `AttachmentProgress` per distribution (carrying the terminal status) and then closes the stream — no hang, no precondition error.
- [ ] `SubscribeAttachmentBundle` on a mixed-state bundle (some distributions terminal, others in-flight at subscribe time) emits one snapshot for each already-terminal distribution before streaming live updates for the in-flight ones — already-terminal state is never silently dropped.
- [ ] A `bundle_id` that has been evicted from the handle table (either by retention timeout or LRU pressure) is treated as a fresh request on resubmit; the prior `distribution_id`s become `NotFound`.
- [ ] Rule 12 identity check covers only `root_name`, `relative_path`, `size_bytes`, `sha256` (in order); a resubmit that differs only in `content_type` or `display_name` is treated as identical and the original metadata is retained.
- [ ] `AttachmentPriority` proto doc-comment explicitly notes that v1 records the classification only — wire-level preemption between QoS classes is deferred to PRD-004 (Future Work #5) and v1 implementations must not claim preemption they don't enforce.
- [ ] `AttachmentPriority` exposes all five QoSClass tiers (including `LOW`/P4); the per-value mapping is documented in the proto enum doc-comment and in the AttachmentPriority section of the PRD, and the aging-promotion path `Bulk → Low → Normal` is reachable from the attachment surface.
- [ ] Handle-table durability is explicitly in-memory in v1: a peat-node restart drops all `bundle_id` lookups; post-restart resubmits are treated as fresh requests; `SubscribeAttachmentBundle` returns `NotFound` for pre-restart bundle_ids. The limitation is documented in Configuration, not left to implementation.
- [ ] Bundle rollback on partial-failure deletes only blobs that this request newly created; pre-existing blob tokens referenced by other live distributions are not deleted.
- [ ] Bundle handle-table retention is bounded by both `--attachment-handle-retention-secs` (default 24h) and `--attachment-max-known-bundles` (default 4096 with LRU eviction); a long-running peat-node does not grow handle-table memory proportional to lifetime send volume.
- [ ] All caps are configurable via CLI flags / `PEAT_NODE_ATTACHMENT_*` env vars; defaults are documented in the Helm `values.yaml`.
- [ ] No deadlocks under concurrent send/subscribe/cancel.

---

### Future Work (out of scope for v1, captured here)

1. **Client-streaming upload** — `SendAttachmentStream(stream Chunk)` for consumers not colocated with peat-node. Same validation framework, swap path-read for chunk consumption.
2. **Capability-aware targeting** — define the capability vocabulary (a follow-on ADR), then add the corresponding predicate fields to `CapableScope` and wire `IrohFileDistribution::known_peers`' `Capable` branch (file_distribution.rs:476, currently TODO) to filter against the live capability registry. ADR-018 (AI Model Capability Advertisement) is one candidate axis but is model-focused; a device-runtime axis (storage / GPU / CPU arch) is the more obvious fit for attachment distribution and may warrant its own ADR. The v1 empty `CapableScope` marker exists so the oneof can grow without renumbering once the schema is decided.
3. **Formation resolution** — back `FormationScope` with a live formation-membership lookup (currently TODOed at file_distribution.rs:474).
4. **Receive-side observer hooks** — currently flagged at file_distribution.rs:620–621. Distribution status today only reflects the sender's view; the receiver fetches via iroh but doesn't write status back through `FileDistribution`. Out of v1 scope but blocks accurate `Partial` reporting.
5. **Per-priority bandwidth share** — once PRD-004 (bandwidth allocation) lands, route attachment distributions through the same `BandwidthAllocation` so bulk attachments don't starve P1 traffic.
6. **Receive-side sink config** — peat-node currently only reads attachments out; once we land receiver-side write-to-disk, mirror the allowlist model for safe sink paths.

---

### Estimated Effort

| Component | New lines | Modified lines | Effort |
|-----------|-----------|----------------|--------|
| Proto + generated code | ~200 | — | 1 hr |
| `attachments/config.rs` + arg parsing | ~120 | ~30 (main.rs) | 2 hrs |
| `attachments/validate.rs` (12 rules) | ~250 | — | 3 hrs |
| `attachments/ingest.rs` (single-pass hash+blob) | ~200 | — | 3 hrs |
| RPC handlers in `service.rs` | ~250 | ~10 | 3 hrs |
| Subscription multiplexer | ~120 | — | 2 hrs |
| `node.rs` / `SidecarConfig` wiring | — | ~80 | 1 hr |
| Helm + Zarf | ~40 | ~20 | 1 hr |
| Unit tests (19) | ~610 | — | 4–5 hrs |
| Integration tests (11) | ~750 | — | 5–6 hrs |
| **Total** | **~2080** | **~140** | **~3–4 days** |

The substrate is already implemented in peat-mesh and peat-protocol. This work is purely the peat-node API surface, validation, and operator-facing config.

---

### Cross-repo coordination

This PRD is implemented entirely in **peat-node**. No changes are required to `peat`, `peat-mesh`, or `peat-protocol` for v1 — the existing `BlobStore`, `FileDistribution`, and `DistributionScope` APIs are sufficient. If the integration uncovers a missing trait method (e.g. a blob-store `delete_local` not currently public), that lands as a separate PR in the source repo, tracked through the same epic issue per the cross-repo policy in `peat/SKILL.md`.
