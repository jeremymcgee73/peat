---
title: "Peat Protocol"
subtitle: "Concept Draft for Internal Review"
author:
  - "Kit Plummer, Defense Unicorns"
date: "21 August 2026"
version: "0.1"
documentclass: article
fontsize: 11pt
geometry: margin=1in
colorlinks: true
linkcolor: NavyBlue
urlcolor: NavyBlue
mainfont: "Arial"
monofont: "Menlo"
header-includes:
  - |
    \renewcommand{\maketitle}{\begin{titlepage}\centering\vspace*{0.2\textheight}{\Huge\bfseries Peat Protocol\par}\vspace{1.5em}{\Large Concept Draft for Internal Review\par}\vspace{3em}{\Large Kit Plummer, Defense Unicorns\par}\vspace{1em}{\large 21 August 2026\par}\vfill{\large Version 0.1\par}\end{titlepage}}
---

> **Document Status:** Early discussion draft for internal review. This document describes the proposed scope and architecture of a potential Internet-Draft. It has not been submitted to, adopted by, or endorsed by the IETF or IRTF. Quantitative results and protocol details identified as preliminary require validation before external submission.

## Abstract

This document proposes the Peat Protocol, a distributed coordination protocol for autonomous systems operating in constrained, partition-prone networks. Peat is intended to explore scalable coordination through CRDT-based hierarchical capability composition, with a design target of improving on all-to-all message complexity while maintaining eventual consistency under stated assumptions.

## Status of This Document

This is a Peat project concept draft prepared to support an internal decision on whether to begin an Internet-Draft development process and engage the IRTF Decentralization of the Internet Research Group (DINRG). It is not an Internet-Draft and does not claim IETF or IRTF consensus.

If the team approves proceeding, this material will be converted into RFC-aware source, reviewed for an appropriate IETF or IRTF stream and intended status, and developed through the applicable contribution and review process.

## Decision Requested

The team is asked to agree on the following starting direction:

1. Proceed with development of an individual Internet-Draft based on this concept.
2. Use DINRG as the initial venue for discussion, subject to confirmation with the research group chairs that the work fits the group charter.
3. Convert the approved concept into RFCXML-compatible source and derive text, HTML, and PDF artifacts from that source.
4. Identify technical reviewers and prospective co-authors for protocol, distributed-systems, security, and operational review.

Approval of this direction does not approve every protocol detail in this revision. Open design and validation work is listed below.

## Known Gaps Before External Submission

The following work is expected during development of an external draft:

- Define a versioned message envelope, framing, dispatch, validation limits, error handling, and transport-independent wire behavior.
- Specify implementable CRDT state or operation encodings, merge rules, causal metadata, anti-entropy, coordinated compaction mechanics, and backend-specific partition-recovery behavior consistent with the collection-history contract in Section 9.5.
- Complete deterministic state machines for discovery, cell formation, leader election, hierarchy changes, command handling, retries, and duplicate suppression.
- Make the security and privacy model self-contained, including authentication, authorization, replay protection, canonical protected bytes, key lifecycle, revocation, and metadata exposure.
- Reconcile the prose and Protocol Buffer schemas, including proto3 presence, defaults, duplicate fields, identifiers, extension behavior, and normative validation rules.
- Define the assumptions, methodology, and evidence supporting scalability, latency, and bandwidth claims; revise the targets where evidence does not support them.
- Decide which protocol elements require IANA registries and replace repository-local normative dependencies with stable public specification text or references.

## Copyright Notice

The repository version of this concept draft is released under CC BY 4.0. The Protocol Buffer definitions are released under CC0 1.0 (public domain). Any Internet-Draft contribution will use the then-current IETF Trust boilerplate and will undergo intellectual-property and licensing review before submission.

---

## 1. Introduction

### 1.1 Problem Statement

Autonomous systems operating in constrained environments face a fundamental coordination challenge: traditional all-to-all communication architectures exhibit O(n^2) message complexity. Project experiments indicate that some representative bandwidth-constrained configurations may encounter practical saturation near 20 participants; the threshold depends on link capacity, update rate, payload size, topology, and workload, and requires documented validation before external submission.

Existing approaches suffer from:

- **Centralized architectures**: Single points of failure incompatible with partition-prone networks
- **Consensus protocols** (Paxos, Raft): Require majority availability, fail during partitions
- **Broadcast mesh**: O(n^2) scaling limits practical deployment to small teams

### 1.2 Solution Overview

Peat addresses these challenges through:

1. **Hierarchical organization**: Bounded cells with elected leaders reduce message paths
2. **CRDT-based state**: Eventual consistency without coordination overhead
3. **Capability composition**: Team capabilities emerge from individual platform capabilities
4. **Differential updates**: Only changes propagate, with a preliminary design target of substantial bandwidth reduction

### 1.3 Design Goals

| Goal | Target |
|------|--------|
| Message complexity | Target O(n log n) vs O(n^2) baseline, subject to a defined cost model |
| Bandwidth reduction | Preliminary target of 95%+ in representative workloads |
| Priority 1 latency | < 5 seconds through the 3-tier aggregation hierarchy (squad → platoon → company) |
| Scale | Validate 100+ nodes and evaluate architecture assumptions at 1000+ nodes |
| Partition tolerance | Full operation during network splits |

These values are design and validation targets for this concept draft, not general performance guarantees. An external draft will document the workload, topology, message and byte accounting, experimental setup, results, and limitations associated with each quantitative claim.

### 1.4 Scope

This specification defines:

- Node and capability data models
- Three-phase protocol operation (Discovery, Cell Formation, Hierarchical Operations)
- CRDT semantics for state synchronization
- Collection history, reconstructibility, retention, and durability semantics
- Hierarchical aggregation and command dissemination
- Message formats (Protocol Buffers)

This specification does NOT define:

- Transport layer binding (implementation-specific)
- CRDT implementation details (use conforming CRDT library)
- Physical/link layer requirements
- Specific platform integrations

---

## 2. Terminology

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in BCP 14 [RFC2119] [RFC8174] when, and only when, they appear in all capitals, as shown here.

### 2.1 Protocol Terms

| Term | Definition |
|------|------------|
| **Node** | A single platform (UAV, UGV, sensor, soldier system) participating in the Peat mesh |
| **Cell** | A bounded group of nodes (4–11 members, default maximum 8; see §7.1) with a single elected leader |
| **Beacon** | Discovery broadcast message advertising node presence and capabilities |
| **Capability** | A discrete function a node or cell can perform (sense, compute, relay, etc.) |
| **Composition** | The process of aggregating individual capabilities into team capabilities |
| **Phase** | One of three operational stages: Discovery, Cell, Hierarchy |

### 2.2 CRDT Terms

| Term | Definition |
|------|------------|
| **CRDT** | Conflict-free Replicated Data Type - data structure that can be replicated and updated independently with guaranteed convergence |
| **LWW-Register** | Last-Writer-Wins Register - CRDT where most recent write (by timestamp) wins |
| **G-Set** | Grow-only Set - CRDT supporting only additions, never removals |
| **OR-Set** | Observed-Remove Set - CRDT supporting additions and removals |
| **PN-Counter** | Positive-Negative Counter - CRDT supporting increment and decrement |

### 2.3 Collection History Terms

| Term | Definition |
|------|------------|
| **Collection Policy** | The independent synchronization, causal-retention, reconstructibility, segmentation, durability, and admission requirements for a collection |
| **Current-State Projection** | A replaceable representation of the latest derived state; it does not by itself preserve how that state was derived |
| **History Source** | The collection and stable source identity containing the domain events or states required for reconstruction |
| **History Segment** | A finite history unit that becomes immutable when sealed |
| **Reconstruction Checkpoint** | An optional derived snapshot used to reduce replay cost without independently authorizing deletion of required history |
| **Durability Target** | The persistence destination or replica count that must acknowledge a sealed segment before local removal can be considered |
| **Durability Acknowledgement** | Verifiable confirmation that the declared durability target persisted a segment; observation or attempted transmission is insufficient |

### 2.4 Military Terms

| Term | Definition |
|------|------------|
| **Squad** | Smallest tactical unit, typically 5-8 personnel/platforms (maps to Cell) |
| **Platoon** | 3-4 squads, typically 24-32 personnel/platforms |
| **Company** | 3-4 platoons, typically 96-128 personnel/platforms |
| **Echelon** | Level in the military hierarchy |

---

## 3. Protocol Overview

### 3.1 Three-Phase Operation

Peat operates in three sequential phases:

```
┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
│   Phase 1:       │     │   Phase 2:       │     │   Phase 3:       │
│   DISCOVERY      │ ──▶ │   CELL           │ ──▶ │   HIERARCHY      │
│                  │     │                  │     │                  │
│ • Beacon broadcast│     │ • Cell formation │     │ • Normal ops     │
│ • Peer discovery │     │ • Leader election│     │ • Aggregation    │
│ • Geohash bucket │     │ • Capability     │     │ • Commands       │
│                  │     │   exchange       │     │ • Differential   │
│                  │     │                  │     │   updates        │
└──────────────────┘     └──────────────────┘     └──────────────────┘
     O(√n)                    O(k^2)                   O(n log n)
```

### 3.2 Phase Transitions

Nodes MUST start in `PHASE_DISCOVERY`.

Transitions:

1. **DISCOVERY → CELL**: When node joins or forms a cell with sufficient members
2. **CELL → HIERARCHY**: When cell has elected leader and is assigned to a zone
3. **Regression**: Nodes MAY regress to earlier phases on partition recovery

### 3.3 Data Flow Architecture

```
                    ┌─────────────────────────────────┐
                    │         Company Summary         │
                    │    (aggregated from platoons)   │
                    └─────────────────────────────────┘
                                    ▲
                    ┌───────────────┴───────────────┐
                    │                               │
           ┌────────┴────────┐             ┌───────┴────────┐
           │ Platoon Summary │             │ Platoon Summary│
           │ (from squads)   │             │ (from squads)  │
           └────────┬────────┘             └───────┬────────┘
                    │                              │
        ┌───────────┼───────────┐                  │
        │           │           │                  │
   ┌────┴────┐ ┌────┴────┐ ┌────┴────┐       ┌────┴────┐
   │ Squad   │ │ Squad   │ │ Squad   │       │ Squad   │
   │ Summary │ │ Summary │ │ Summary │       │ Summary │
   └────┬────┘ └────┬────┘ └────┬────┘       └────┬────┘
        │           │           │                 │
     Nodes       Nodes       Nodes             Nodes
```

**Upward flow** (data/status): Individual state → Squad summary → Platoon summary → Company summary

**Downward flow** (commands): Company → Platoons → Squads → Individual nodes

---

## 4. Node Model

### 4.1 Node Structure

A Node consists of static configuration (`NodeConfig`) and dynamic state (`NodeState`).

```
Node
├── NodeConfig (immutable)
│   ├── id: UUID v4 (REQUIRED)
│   ├── platform_type: string (REQUIRED)
│   ├── capabilities: [Capability] (REQUIRED, G-Set)
│   ├── comm_range_m: float (OPTIONAL)
│   ├── max_speed_mps: float (OPTIONAL)
│   ├── operator_binding: HumanMachinePair (OPTIONAL)
│   └── created_at: Timestamp (OPTIONAL)
│
└── NodeState (CRDT-backed)
    ├── position: Position (REQUIRED, LWW-Register)
    ├── fuel_minutes: uint32 (OPTIONAL, PN-Counter)
    ├── health: HealthStatus (REQUIRED, LWW-Register)
    ├── phase: Phase (REQUIRED, LWW-Register)
    ├── cell_id: string (OPTIONAL, LWW-Register)
    ├── zone_id: string (OPTIONAL, LWW-Register)
    └── timestamp: Timestamp (REQUIRED)
```

### 4.2 Node Identity

- Node `id` MUST be a valid UUID version 4
- Node `id` MUST be unique across the entire mesh
- Node `id` MUST NOT change during node lifetime

### 4.3 Health Status

Implementations MUST support the following health states:

| Status | Value | Description |
|--------|-------|-------------|
| NOMINAL | 1 | Fully operational |
| DEGRADED | 2 | Reduced capability but operational |
| CRITICAL | 3 | Failure imminent, limited operations |
| FAILED | 4 | Non-operational |

Nodes with `FAILED` health SHOULD be excluded from:

- Leader election candidates
- Capability aggregation
- Active mission assignment

### 4.4 Human-Machine Teaming

When a node has an associated human operator (`operator_binding`), the operator's rank and authority level affect:

1. **Leader election scoring**: Higher rank/authority increases leadership score
2. **Command authorization**: ROE may require specific authority levels
3. **Cognitive load adjustment**: Degraded operator performance reduces effective authority

---

## 5. Capability Model

### 5.1 Capability Types

| Type | Value | Description |
|------|-------|-------------|
| SENSOR | 1 | Sensing: cameras, radar, sonar, SIGINT |
| COMPUTE | 2 | Processing: inference, analysis |
| COMMUNICATION | 3 | Relay, mesh networking, BLOS |
| MOBILITY | 4 | Flight, ground movement, maritime |
| PAYLOAD | 5 | Cargo, weapons, countermeasures |
| EMERGENT | 6 | Created through composition |

### 5.2 Capability Composition

Cell leaders aggregate member capabilities using four composition patterns:

#### 5.2.1 Additive Composition

Sum individual capabilities:

```
team_capability = Σ individual_capabilities
```

Example: Total sensor coverage = sum of individual coverage areas

#### 5.2.2 Emergent Composition

New capabilities from combinations:

```
IF (sensor ∈ team AND compute ∈ team AND comms ∈ team)
THEN team.add(ISR_Chain)
```

Example: ISR chain emerges when team has sensor, compute, and communications

#### 5.2.3 Redundant Composition

Improved reliability through overlap:

```
team_reliability = 1 - Π(1 - individual_reliability)
```

Example: Detection probability improves with multiple sensors

#### 5.2.4 Constraint Composition

Team limited by weakest/strongest member:

```
team_speed = min(individual_speeds)
team_range = max(individual_ranges)
```

---

## 6. Phase 1: Discovery

### 6.1 Beacon Broadcasting

During Phase 1, nodes MUST:

1. Broadcast `Beacon` messages at configurable intervals (default: 1 second)
2. Include current position, capabilities, and state in beacons
3. Increment `sequence_number` monotonically with each beacon
4. Process received beacons from peers

### 6.2 Geographic Scoping

To target reduced discovery scope, implementations SHOULD use geographic hashing. The resulting complexity depends on node distribution, bucket occupancy, movement, and boundary-query behavior; O(√n) is a design hypothesis to be evaluated under an explicit model:

1. Compute geohash from current position (precision 5-6)
2. Broadcast beacons only within geohash bucket
3. Query neighboring buckets for boundary conditions

### 6.3 Beacon TTL

For multi-hop relay:

1. Initial beacon TTL SHOULD be 3 (configurable)
2. Each relay node decrements TTL
3. Beacons with TTL=0 MUST NOT be relayed

### 6.4 Phase Transition

Node transitions to `PHASE_CELL` when:

- Sufficient peers discovered (implementation-defined threshold)
- OR cell formation request received
- OR C2 directive received

---

## 7. Phase 2: Cell Formation

### 7.1 Cell Structure

A Cell consists of:

- 1 elected leader
- 4-11 members (configurable, default max_size=8)
- Aggregated capabilities

### 7.2 Leader Election

Leader election uses deterministic scoring:

```
score = technical_score × technical_weight + authority_score × authority_weight
```

Where:

- `technical_score` = f(compute, comms, sensors, power, reliability)
- `authority_score` = f(rank, authority_level, cognitive_load, fatigue) [if human present]
- Weights are policy-configurable

Tie-breaking: Lexicographically lowest node ID wins.

### 7.3 Membership Protocol

1. **Join Request**: Node sends `CellFormationRequest` to discovered peers
2. **Join Response**: Existing cell leader responds with `CellFormationResponse`
3. **State Update**: New member updates `cell_id` in `NodeState`
4. **Capability Recomputation**: Leader recomputes aggregated capabilities

### 7.4 Phase Transition

Cell transitions to `PHASE_HIERARCHY` when:

- Leader elected AND confirmed
- Minimum membership threshold met
- Zone assignment received (for multi-tier hierarchies)

---

## 8. Phase 3: Hierarchical Operations

### 8.1 Hierarchical Aggregation

Leaders at each level publish aggregated summaries:

| Level | Summary Type | Typical Size | Aggregates |
|-------|--------------|--------------|------------|
| Squad | `SquadSummary` | 5-8 nodes | Individual NodeState |
| Platoon | `PlatoonSummary` | 24-32 nodes | SquadSummary |
| Company | `CompanySummary` | 96-128 nodes | PlatoonSummary |

Each level is intended to reduce bandwidth relative to unaggregated dissemination through:

- Averaging positions to centroids
- Summarizing health to worst-case
- Aggregating capabilities through composition

### 8.2 Summary Contents

All summaries MUST include:

- Unit identifier
- Leader identifier
- Member/subordinate count
- Position centroid
- Worst health status
- Operational count
- Aggregated capabilities
- Readiness score [0.0, 1.0]
- Aggregation timestamp

### 8.3 Command Dissemination

Commands flow downward through the hierarchy:

1. Originator creates `HierarchicalCommand` with target scope
2. Command propagates to target level (platoon/squad/individual)
3. Targets execute command and send `CommandAcknowledgment`
4. Acknowledgments flow upward to originator

### 8.4 Priority Handling

| Priority | Behavior |
|----------|----------|
| ROUTINE (1) | Normal queue processing |
| PRIORITY (2) | Expedited processing |
| IMMEDIATE (3) | Preempts lower priority |
| FLASH (4) | Immediate execution, conflict override |

---

## 9. CRDT Semantics

### 9.1 CRDT Types Used

| Data | CRDT Type | Merge Semantics |
|------|-----------|-----------------|
| Position | LWW-Register | Latest timestamp wins |
| Health | LWW-Register | Latest timestamp wins |
| Phase | LWW-Register | Latest timestamp wins |
| Capabilities | G-Set | Union of all observed |
| Cell members | OR-Set | Add/remove with tombstones |
| Fuel | PN-Counter | Sum of increments minus decrements |

### 9.2 Timestamp Requirements

- Timestamps MUST use Unix epoch with nanosecond precision
- Implementations SHOULD use synchronized time sources (NTP, GPS)
- For LWW semantics, implementations MUST ensure monotonically increasing timestamps per node

### 9.3 Conflict Resolution

When concurrent updates conflict:

1. **LWW fields**: Higher timestamp wins
2. **G-Set fields**: Union (all values preserved)
3. **OR-Set fields**: Observed-remove semantics (see CRDT literature)
4. **Tie-breaking**: Lexicographically lowest node ID wins

Tombstone lifecycle for OR-Set fields (when tombstones may be garbage-collected, how long they MUST be retained across partitions) is implementation-defined in this revision. An implementation MUST NOT remove deletion metadata or causal history before the applicable collection policy's durability and stale-peer safety conditions are met. See Peat ADR-034 and ADR-076 for the design discussion that informs implementations.

### 9.4 Consistency Guarantees

Peat provides **eventual consistency** within a declared collection policy:

- Participating, authorized replicas using compatible effective policies converge after receiving the required data
- No coordination required during partitions
- Durability is established only by acknowledgement from the declared persistence target; observation or attempted transmission is not sufficient

Peat does NOT provide:

- Linearizability
- Strong consistency
- Total ordering of operations

### 9.5 Collection History and Reconstructibility

Each production-writable collection MUST have an effective policy that treats
the following as independent dimensions:

1. synchronization behavior;
2. CRDT causal-history retention;
3. domain-event reconstructibility;
4. segmentation and retention;
5. durability target and acknowledgement; and
6. admission and over-budget behavior; and
7. epoch fencing and expired-history resurrection behavior.

No dimension silently weakens another. `LatestOnly` guarantees convergence of
current state only. It does not guarantee replay, provenance, derivation, or
correction history. `FullHistory` preserves the causal operations required by
the selected CRDT synchronization algorithm, but those operations are not
necessarily sufficient application-level history. `WindowedHistory` bounds
synchronization and does not by itself bound local storage.

Raw CRDT operations MUST NOT be treated as reconstructible domain history
unless the collection contract and schema define a complete deterministic
mapping from those operations to the required domain events.

#### 9.5.1 Current Projections and History Sources

A collection that claims bounded or complete reconstructibility MUST identify
a generic history source by collection identity and stable source identity. A
current-state projection MAY reference immutable event or history segments and
an optional reconstruction checkpoint and segment catalog. The protocol does
not require a domain-specific projection type.

Checkpoints MAY reduce replay cost. They MUST NOT independently authorize
deletion of events required by the declared history and retention policy.

#### 9.5.2 Segment Lifecycle

Reconstructible history MUST use finite segments. Implementations MUST rotate
an active segment when any configured time, event-count, serialized-byte, or
revision limit is reached. A sealed segment is immutable.

Segments progress through these states:

```text
Active -> Sealed -> Durably Acknowledged -> Retention Eligible -> Removed
```

A segment MUST NOT become retention-eligible until its declared durability
target has acknowledged persistence and its retention requirement is
satisfied. Attempted transmission, receipt, or observation is not durability
acknowledgement. Removing an eligible local copy does not imply a mesh-wide
deletion.

Complete history has no time-based expiry. Bounded reconstructible history MUST
declare a positive retention interval. A local-only durability target does not
authorize removal of its sole durable copy.

Each segment descriptor MUST include a stable source, epoch, and segment
identity plus an inclusive source-local sequence range. A sealed descriptor
MUST include a content encoding and SHA-256 identity of the exact immutable
payload bytes stored and transferred under that encoding. The digest excludes
the descriptor and MUST NOT be computed from a non-canonical re-encoding. A
descriptor MAY include time coverage, predecessor, and checkpoint references.
These identifiers support catalog
discovery, ordering, gap detection, immutable-content verification, restart,
and stale-writer fencing without requiring domain-specific payload knowledge.

#### 9.5.3 Partition and Capacity Behavior

If a node cannot satisfy a durability target or storage budget, it MUST apply
the declared behavior: retain locally, backpressure the producer, or reject the
write. It MUST NOT accept an event under a reconstructible-history claim and
silently discard the only qualifying copy.

Segment identities and rotation boundaries MUST survive restart. A closed epoch
MUST persist its active successor identity and reject subsequent writes with
that identity; implementations MUST NOT silently redirect stale writes. An
explicit policy MUST reject, quarantine, or remove-again history that returns
after retention expiry. Unknown resurrection behavior resolves to quarantine.
Unknown policy versions preserve causal history, reject unknown-epoch writes,
and authorize no history removal.

Implementations MUST expose the effective policy source, enforcement state,
durability progress, epoch and segment lifecycle, retention not-before time,
evaluation time, and retention eligibility. Invalid
policy combinations MUST fail validation rather than being rewritten to weaker
semantics. Peat ADR-076 records the rationale and compatibility rules.

---

## 10. Message Complexity Analysis

### 10.1 Discovery Phase: O(√n) Design Hypothesis

Geographic hashing limits discovery to local peers:

- n nodes distributed over area A
- Geohash bucket covers area A/b
- Expected peers per bucket: n/b
- Under a model where bucket selection keeps expected occupancy proportional to √n: O(√n) messages per node

This result is conditional on the geographic distribution and bucket-selection assumptions. The external draft will define those assumptions and evaluate skewed, mobile, and boundary-heavy cases.

### 10.2 Cell Formation: O(k^2) per cell

Within cells of bounded size k:

- Full state exchange within cell: k^2 messages
- Total across n/k cells: O(kn)
- With constant k: O(n)

### 10.3 Hierarchical Operations: O(n log n) Design Target

With hierarchy depth d = log(n/k):

- Each level is expected to suppress a workload-dependent fraction of updates
- Updates propagate through d levels
- Total: O(n log n)

This is a preliminary upper-bound target for the described propagation model, not a measured guarantee. The cost model must distinguish messages, transmitted bytes, per-node load, and aggregate network load.

### 10.4 Comparison to Baseline

| Architecture | Messages (100 nodes) | Messages (1000 nodes) |
|--------------|---------------------|----------------------|
| All-to-all | 9,900 | 999,000 |
| Peat design estimate | ~664 | ~6,644 |
| Estimated reduction | 93% | 99.3% |

The Peat values are illustrative calculations based on n log2(n) rather than measured deployment results. They are included to motivate validation and MUST NOT be interpreted as guaranteed performance.

---

## 11. Security Considerations

### 11.1 Authentication

Implementations SHOULD provide:

- Node identity authentication (PKI/certificates)
- Operator credential verification
- Message integrity (signatures using Ed25519 or ECDSA-P256/P384 per Peat ADR-060 §5)

Peat ADR-044 specifies the end-to-end encryption and key-management model (including the MLS ciphersuite selection — FIPS-aligned suites only); Peat ADR-048 specifies the tactical membership-certificate and enrollment model that underpins node identity in disconnected and partitioned environments.

### 11.2 Authorization

Implementations SHOULD enforce:

- Role-based access control for commands
- Authority level requirements for ROE
- Cell membership authorization, with membership claims bound to the certificates defined in Peat ADR-048

### 11.3 Confidentiality

For classified or otherwise sensitive environments, implementations SHOULD provide:

- Message encryption using a FIPS 140-3 approved AEAD; AES-256-GCM RECOMMENDED (NIST SP 800-38D)
- Key agreement using ECDH on NIST curves P-256 or P-384 (NIST SP 800-56A); X25519 MAY be used only where module-level FIPS coverage has been independently verified
- Key derivation using HKDF-SHA-256 or HKDF-SHA-384 (NIST SP 800-56C / SP 800-108)
- Forward secrecy via ephemeral key agreement per session

The complete normative list of approved cryptographic primitives — including AEAD, signatures, key agreement, KDF, MAC, hashes, and the TLS/QUIC FIPS-mode provider requirement — is defined in Peat ADR-060 §5 "Cryptographic primitives (FIPS posture)". Implementations MUST NOT introduce non-approved primitives (e.g., ChaCha20-Poly1305, deterministic or order-preserving schemes) without an explicit superseding decision recorded in that ADR.

### 11.4 Denial of Service

Implementations SHOULD mitigate:

- Beacon flooding (rate limiting)
- Invalid capability claims (validation)
- Leader election manipulation (deterministic scoring; membership-certificate verification per ADR-048 before a node is admitted as an election candidate)

### 11.5 Compromised-Node Response

When a node is determined to be compromised (e.g., by observed misbehaviour, lost device, or out-of-band intelligence), implementations SHOULD support coordinated ejection from the mesh: revocation of the node's membership claims, propagation of the revocation through the OR-Set membership state, and exclusion from future leader-election candidacy. The detection criteria, quorum semantics for revocation, and recovery procedures are specified in Peat ADR-056; this revision of the protocol leaves them informative pending normative inclusion in a future draft.

---

## 12. IANA Considerations

This document has no IANA actions at this time.

Future versions may request:

- Protocol port assignment
- Capability type registry
- Command type registry

---

## 13. References

### 13.1 Normative References

- [RFC2119] Bradner, S., "Key words for use in RFCs to Indicate Requirement Levels", BCP 14, RFC 2119, March 1997.
- [RFC8174] Leiba, B., "Ambiguity of Uppercase vs Lowercase in RFC 2119 Key Words", BCP 14, RFC 8174, May 2017.
- [RFC4122] Leach, P., Mealling, M., and R. Salz, "A Universally Unique IDentifier (UUID) URN Namespace", RFC 4122, July 2005.

### 13.2 Informative References

- Shapiro, M., Preguiça, N., Baquero, C., and M. Zawirski, "Conflict-free Replicated Data Types", SSS 2011.
- Kleppmann, M., "Making Sense of Stream Processing", O'Reilly Media, 2016.
- NATO STANAG 4586, "Standard Interfaces of UAV Control System (UCS) for NATO UAV Interoperability".
- NIST FIPS 140-3, "Security Requirements for Cryptographic Modules", March 2019.
- NIST FIPS 186-5, "Digital Signature Standard (DSS)", February 2023.
- NIST FIPS 198-1, "The Keyed-Hash Message Authentication Code (HMAC)", July 2008.
- NIST SP 800-38D, "Recommendation for Block Cipher Modes of Operation: Galois/Counter Mode (GCM) and GMAC", November 2007.
- NIST SP 800-56A Rev. 3, "Recommendation for Pair-Wise Key-Establishment Schemes Using Discrete Logarithm Cryptography", April 2018.
- NIST SP 800-56C Rev. 2, "Recommendation for Key-Derivation Methods in Key-Establishment Schemes", August 2020.
- Peat ADR-034, "Record Deletion and Tombstone Management".
- Peat ADR-044, "End-to-End Encryption and Key Management".
- Peat ADR-048, "Membership Certificates and Tactical Trust".
- Peat ADR-056, "Compromised Node Ejection".
- Peat ADR-060, "Encryption Tiers — At-Rest and In-Transit Across the Peat Stack". *(authoritative FIPS primitive list — see §5)*
- Peat ADR-076, "Reconstructible Collection History Contract".

---

## Appendix A. Protocol Buffer Schema

The proposed Protocol Buffer definitions are available in `spec/proto/`; the
canonical collection-history schema is currently generated from
`peat-schema/proto/history.proto`:

- `cap/v1/common.proto` - Common types
- `cap/v1/node.proto` - Node model
- `cap/v1/capability.proto` - Capability model
- `cap/v1/cell.proto` - Cell model
- `cap/v1/beacon.proto` - Discovery beacons
- `cap/v1/composition.proto` - Composition rules
- `cap/v1/hierarchy.proto` - Hierarchical summaries
- `cap/v1/command.proto` - Command dissemination
- `peat-schema/proto/history.proto` (`peat.history.v1`) - Collection history policy, segment identity, and enforcement status

---

## Appendix B. Example Message Flows

### B.1 Discovery and Cell Formation

```
Node A                    Node B                    Node C
  │                         │                         │
  │──── Beacon ────────────▶│                         │
  │                         │──── Beacon ────────────▶│
  │◀──────────── Beacon ────│                         │
  │                         │◀──────────── Beacon ────│
  │◀─────────────────────────────────── Beacon ───────│
  │                         │                         │
  │── CellFormationReq ────▶│                         │
  │                         │── CellFormationReq ────▶│
  │◀── CellFormationResp ───│                         │
  │                         │◀── CellFormationResp ───│
  │                         │                         │
  │        [Leader Election: B wins]                  │
  │                         │                         │
  │◀─── SquadSummary ───────│─── SquadSummary ───────▶│
```

### B.2 Hierarchical Command Flow

```
Company Cmd ─────▶ Platoon Leader ─────▶ Squad Leader ─────▶ Node
                        │                     │               │
                        │◀──── Squad Ack ─────│◀──── Ack ─────│
    ◀── Platoon Ack ────│                     │               │
```

---

## Author's Address

Kit Plummer
Defense Unicorns
Email: kitplummer@defenseunicorns.com
GitHub: https://github.com/defenseunicorns/peat
