# Forming a Team & Assigning Leaders

> **Version**: 1.0
> **Last Updated**: 2026-06-03
> **Audience**: Software Engineers, Protocol Contributors, Integration Developers

---

This guide walks through how a group of independent nodes ("things") discover each
other, authenticate into a shared **formation**, elect a leader, take on roles, and
recover when the leader is lost. It is a *developer how-to* over the primitives in
`peat-protocol` (and the formation auth in `peat-mesh`) — it shows the actual types
and calls, not the wire format.

For the **normative** protocol definition, see [`spec/004-coordination.md`](../../spec/004-coordination.md).
For design rationale, see [ADR-024 (flexible hierarchy strategies)](../../adr/024-flexible-hierarchy-strategies.md)
and [ADR-066 (abstract hierarchy vocabulary)](../../adr/066-abstract-hierarchy-vocabulary.md).

## Table of Contents

1. [Concepts & vocabulary](#1-concepts--vocabulary)
2. [The lifecycle at a glance](#2-the-lifecycle-at-a-glance)
3. [Step 1 — Discovery](#3-step-1--discovery)
4. [Step 2 — Formation & authentication](#4-step-2--formation--authentication)
5. [Step 3 — Leader election](#5-step-3--leader-election)
6. [Step 4 — Role assignment](#6-step-4--role-assignment)
7. [Step 5 — Confirming the formation](#7-step-5--confirming-the-formation)
8. [Failover & re-election](#8-failover--re-election)
9. [State, sync & conflict resolution](#9-state-sync--conflict-resolution)
10. [Scaling up the hierarchy](#10-scaling-up-the-hierarchy)
11. [API reference](#11-api-reference)

---

## 1. Concepts & vocabulary

A **formation** is a set of nodes that share a `formation_id` and a secret key and have
mutually authenticated. Membership is the trust boundary: only nodes that can prove they
hold the formation key sync state with each other.

State aggregates through a fixed four-tier hierarchy above the individual node
(ADR-066). `HierarchyLevel` (`peat_mesh::beacon::HierarchyLevel`):

| Tier | `HierarchyLevel` | Meaning |
|------|------------------|---------|
| Platform | `Platform` (0) | A single node — a vehicle, sensor, handset, container. |
| Cell | `Cell` (1) | The smallest aggregation unit; has one leader and N members. |
| Cohort | `Cohort` (2) | A set of cells sharing a mission, role, region, or time window. |
| Federation | `Federation` (3) | An alliance of cohorts coordinating without central authority. |
| Coalition | `Coalition` (4) | Top tier; an alliance of federations. |

> **Vocabulary note.** This is the post-ADR-066 vocabulary (Cell/Cohort/Federation/Coalition).
> Older code/docs may still say Squad/Platoon/Company — those are the same tiers, renamed.
> Use the new terms in all new code.

Each tier has **its own** leader/coordinator. This guide focuses on the **cell** — the
unit where formation and leader election actually happen. Higher tiers aggregate cell
summaries upward (see §10).

The primitives live in `peat-protocol`:

```rust
use peat_protocol::cell::{
    CellCoordinator, FormationStatus, LeaderElectionManager, LeadershipScore,
    CellMessageBus,
};
use peat_protocol::models::{CellConfig, CellState, CellStateExt, CellRole, RoleScorer};
use peat_protocol::peat_schema; // proto types: capability::v1, node::v1, cell::v1
```

---

## 2. The lifecycle at a glance

```
   ┌─────────────┐   discovery    ┌──────────────┐  formation handshake  ┌────────────┐
   │  a node     │ ─────────────▶ │ candidate    │ ────────────────────▶ │  member of │
   │ (Platform)  │  mDNS / static │ peers found  │  HMAC challenge/resp  │  formation │
   └─────────────┘                └──────────────┘                       └─────┬──────┘
                                                                                │
                          ┌─────────────────────────────────────────────────────┘
                          ▼
   ┌──────────────┐ capability score ┌──────────────┐  RoleScorer  ┌──────────────────┐
   │ leader       │ ───────────────▶ │ leader_id    │ ───────────▶ │ members take     │
   │ election     │  + tie-break id  │ set on Cell  │              │ Sensor/Compute/… │
   └──────────────┘                  └──────┬───────┘              └────────┬─────────┘
                                            │                               │
                                            ▼                               ▼
                              ┌──────────────────────────────────────────────────┐
                              │ CellCoordinator: size + leader + roles + readiness │
                              │ (+ optional human approval)  →  FormationStatus    │
                              └──────────────────────────┬─────────────────────────┘
                                                         ▼
                                              Ready  ───────────▶  (heartbeats)
                                                         ▲                 │ leader lost
                                                         └──── re-election ◀┘
```

Five steps: **discover → form (authenticate) → elect → assign roles → confirm**, then a
steady state maintained by heartbeats with automatic **re-election** on leader loss.

---

## 3. Step 1 — Discovery

Before nodes can form, they must find each other's addresses. Discovery is provided by
`peat-mesh` and is pluggable (`peat_mesh::discovery`):

- **mDNS** (`MdnsDiscovery`) — zero-config discovery on a local/tactical LAN.
- **Static** — an explicit peer list (TOML/YAML or the credentials bundle's `peers`).
- **Kubernetes** — watches EndpointSlices for peer addresses.

Discovery only yields *candidate* addresses — it does **not** grant membership. A
discovered peer is admitted to the formation only after it passes the handshake (§4).
Discovery emits `DiscoveryEvent::PeerFound`, which the connector turns into a
formation-authenticated connection.

---

## 4. Step 2 — Formation & authentication

Membership is proven with a shared **formation key**, never by mere reachability.

### The formation key

`FormationKey` (`peat_mesh::security::FormationKey`) is derived from the `formation_id`
and a 32-byte shared secret via **HKDF-SHA-256** (FIPS-aligned per ADR-060):

```rust
use peat_mesh::security::FormationKey;

// From operator credentials (app_id + base64 shared key):
let key = FormationKey::from_base64(formation_id, base64_shared_key)?;

// Or directly from raw key material:
let key = FormationKey::new(formation_id, &shared_secret_32);
```

### The handshake

When one node dials another, they run a challenge-response over a dedicated ALPN before
any state is exchanged (`peat_protocol::network::formation_handshake`):

```rust
use peat_protocol::network::formation_handshake::{
    FORMATION_HANDSHAKE_ALPN,         // b"peat/formation-auth/1"
    perform_initiator_handshake,
    perform_responder_handshake,
};
```

The flow:

1. Initiator opens a stream on `FORMATION_HANDSHAKE_ALPN` and sends its `formation_id`.
2. Responder replies with a fresh random **nonce** (the challenge).
3. Initiator returns `HMAC-SHA-256(nonce ‖ formation_id)` using the formation key.
4. Responder verifies in constant time. Match ⇒ admitted; mismatch ⇒ rejected.

Because the nonce is fresh per handshake and the `formation_id` is mixed into the MAC,
the exchange is non-replayable and a node in a *different* formation (different id or
secret) is rejected. Only after success is the peer added to the formation's peer set and
allowed to sync.

> Most applications get this for free by standing up `peat_mesh::AutomergeBackend` with a
> `FormationKey`; the backend performs the handshake on every connection. Reach for the
> `perform_*_handshake` functions directly only if you are building a custom transport.

### Membership

The authenticated member set is recorded in the cell document, `CellState.members`
(an OR-Set CRDT — see §9), via `CellStateExt`:

```rust
use peat_protocol::models::{CellState, CellStateExt, CellConfig};

let mut cell = CellState::new(CellConfig::default());
cell.add_member("node-a".to_string());   // returns true if newly added
cell.add_member("node-b".to_string());
```

---

## 5. Step 3 — Leader election

Peat elects a leader **deterministically from capability scores** — there is no
multi-round consensus protocol, which keeps it correct and live under partition.

### Leadership score

`LeadershipScore::from_capabilities` reduces a node's advertised capabilities to a single
weighted score:

```rust
use peat_protocol::cell::LeadershipScore;

let score = LeadershipScore::from_capabilities(&capabilities);
// total = compute·0.30 + communication·0.25 + sensors·0.20 + power·0.15 + reliability·0.10
```

Ties are broken by **lexicographic node id**, so every node in a cell independently
computes the *same* winner without exchanging votes — this is what makes it split-brain
safe:

```rust
use std::cmp::Ordering;
let winner_is_me = my_score.compare(&their_score, my_id, their_id) == Ordering::Greater;
```

### Driving the election

`LeaderElectionManager` runs the state machine (`Candidate → Leader | Follower`) over the
cell message bus:

```rust
use std::sync::Arc;
use peat_protocol::cell::{LeaderElectionManager, CellMessageBus};

let bus = Arc::new(CellMessageBus::new(cell_id.clone(), my_node_id.clone()));
let election = LeaderElectionManager::new(
    cell_id.clone(),
    my_node_id.clone(),
    bus.clone(),
    my_capabilities,        // Vec<peat_schema::capability::v1::Capability>
);

election.start_election()?;                 // announce candidacy (round 1)
// for each inbound cell message:
election.process_election_message(&msg)?;   // compare scores, converge

match election.get_state() {                // ElectionState
    state => tracing::info!(?state, leader = ?election.get_leader()),
}
```

Defaults (from `LeaderElectionManager::new`): election timeout **5s**, heartbeat interval
**2s**, **3** missed heartbeats tolerated.

---

## 6. Step 4 — Role assignment

The **leader is elected** (§5). Every *other* role is **assigned** by fitness scoring.
`CellRole` (`peat_protocol::models::CellRole`):

| Role | Assigned? | Purpose |
|------|-----------|---------|
| `Leader` | elected, not assigned | Coordinates the cell. |
| `Sensor` | assigned | Detection / reconnaissance. |
| `Compute` | assigned | Processes data, runs analysis. |
| `Relay` | assigned | Extends network range. |
| `Strike` | assigned | Engages targets (effector platforms). |
| `Support` | assigned | Logistics / medical / maintenance. |
| `Follower` | assigned (default) | General member; no specialized role. |

`CellRole::assignable_roles()` returns everything except `Leader`. `RoleScorer` picks the
best role for a node from its config + live state:

```rust
use peat_protocol::models::{RoleScorer, CellRole};

if let Some((role, fitness)) = RoleScorer::best_role_for_platform(&node_config, &node_state) {
    tracing::info!(?role, fitness, "assigned role");
}
```

Scoring weights required capabilities (blocking — a missing required capability disqualifies
the role), preferred capabilities, operator fitness, and platform health. See `role.rs` and
[`spec/004-coordination.md` §7](../../spec/004-coordination.md).

---

## 7. Step 5 — Confirming the formation

A cell is not "ready" just because a leader exists. `CellCoordinator` gates the transition
to operational and is the single place that decides *done*:

```rust
use peat_protocol::cell::{CellCoordinator, FormationStatus};

let mut coordinator = CellCoordinator::new(cell_id.clone());

// members: &[(NodeConfig, NodeState, Option<CellRole>)]
let ready = coordinator.check_formation_complete(&members, leader_id.as_deref())?;
```

`check_formation_complete` requires, in order:

1. **Minimum size** (default 3 members).
2. **A confirmed leader** (`leader_id` is `Some`).
3. **Every member has an assigned role**.
4. **Required capability coverage** (default: Communication + Sensor present).
5. **Readiness ≥ threshold** (default 0.7).
6. **Human approval**, *if* any present capability requires oversight.

The result is reflected in `FormationStatus`:

```rust
match &coordinator.status {          // `status` is a public field
    FormationStatus::Forming          => { /* still gathering members/roles */ }
    FormationStatus::AwaitingApproval => {
        // a human-in-the-loop gate; resolve with:
        coordinator.approve_formation()?;     // → Ready
        // or coordinator.reject_formation("reason".into())?; // → Failed
    }
    FormationStatus::Ready            => { /* operational; may aggregate upward */ }
    FormationStatus::Failed(reason)   => { tracing::warn!(%reason, "formation failed"); }
}

assert!(coordinator.can_transition_to_hierarchical() == matches!(coordinator.status, FormationStatus::Ready));
```

---

## 8. Failover & re-election

The leader proves liveness with heartbeats; followers re-elect when those stop.

```rust
// Leader side — call on the heartbeat interval (default 2s):
election.send_heartbeat_if_leader()?;

// Follower side — call periodically to detect a dead leader:
if election.check_leader_failure()? {
    // ≥3 missed heartbeats (~6s): manager resets to Candidate, bumps the round,
    // and re-announces. The new round number fences out stale announcements.
}
```

Re-election reuses the same deterministic scoring, so absent the failed leader the cell
converges on the next-best node — again without a vote. Round numbers
(`election.get_round()`) monotonically increase so late messages from a previous round are
ignored.

**Under partition**, each side elects locally (liveness over global agreement). On heal,
the conflict resolves deterministically through CRDT merge (§9): the `leader_id` with the
later timestamp wins, and the losing partition's nodes converge to it.

---

## 9. State, sync & conflict resolution

Cell membership and leadership are not RPC state — they are fields of the `CellState`
**CRDT document**, synced through the formation's Automerge backend. The merge semantics
(`CellStateExt::merge`) are what make concurrent edits safe:

| Field | CRDT type | Merge rule |
|-------|-----------|------------|
| `members` | OR-Set | union (add/remove converge) |
| `leader_id` | LWW-Register | latest timestamp wins |
| capabilities | G-Set | union |

```rust
use peat_protocol::models::{CellState, CellStateExt};

// set_leader validates the node is a member first:
cell.set_leader("node-a".to_string())
    .expect("leader must be a current member");
assert!(cell.is_leader("node-a"));

// removing the leader clears the leadership register:
cell.remove_member("node-a");   // also clears leader_id
assert!(!cell.is_leader("node-a"));

// convergence after a partition:
cell.merge(&other_replica);     // OR-Set/LWW/G-Set per field
```

Because `leader_id` is LWW, two partitions that elected different leaders converge to one
deterministically on merge — no special reconciliation code path.

---

## 10. Scaling up the hierarchy

Cells don't talk to every node in a federation — they **aggregate**. A cell leader publishes
a `CellSummary`; a cohort coordinator reduces many `CellSummary`s into a `CohortSummary`;
that rolls up to `FederationSummary` and `CoalitionSummary`
(`peat-schema/proto/hierarchy.proto`).

Key points for developers:

- **Each tier elects/assigns its own coordinator** — a cohort coordinator is *not* a cell
  leader; it's chosen at the cohort tier. The same scoring primitives apply per tier.
- **Summaries are LatestOnly** collections (only the newest matters), so they sync cheaply.
- **Commands flow down, state flows up.** Message priority escalates when crossing a tier
  boundary (`RoutingContext` in `cell/messaging.rs`).

For the strategy that decides how nodes are grouped into tiers (static vs. dynamic vs.
hybrid), see [ADR-024](../../adr/024-flexible-hierarchy-strategies.md).

---

## 11. API reference

All paths are under `peat_protocol::` unless noted.

| Concern | Type / fn | Location |
|---------|-----------|----------|
| Formation key (HKDF-SHA-256) | `peat_mesh::security::FormationKey` — `from_base64`, `new` | `peat-mesh/src/security/formation_key.rs` |
| Handshake (HMAC-SHA-256 C/R) | `network::formation_handshake::{FORMATION_HANDSHAKE_ALPN, perform_initiator_handshake, perform_responder_handshake}` | `peat-protocol/src/network/formation_handshake.rs` |
| Leadership score | `cell::LeadershipScore::{from_capabilities, compare}` | `peat-protocol/src/cell/leader_election.rs` |
| Election state machine | `cell::LeaderElectionManager` — `start_election`, `process_election_message`, `check_leader_failure`, `send_heartbeat_if_leader`, `get_state`, `get_leader`, `get_round` | `peat-protocol/src/cell/leader_election.rs` |
| Cell message bus | `cell::CellMessageBus::new(cell_id, node_id)` | `peat-protocol/src/cell/messaging.rs` |
| Formation completion | `cell::CellCoordinator` — `check_formation_complete`, `approve_formation`, `reject_formation`, `can_transition_to_hierarchical`; `cell::FormationStatus` | `peat-protocol/src/cell/coordinator.rs` |
| Roles | `models::CellRole` (`assignable_roles`, `required_capabilities`); `models::RoleScorer::{best_role_for_platform, score_platform_for_role}` | `peat-protocol/src/models/role.rs` |
| Cell document | `models::{CellState, CellStateExt, CellConfig}` — `add_member`, `remove_member`, `set_leader`, `clear_leader`, `is_leader`, `merge` | `peat-protocol/src/models/cell/mod.rs` |
| Hierarchy levels | `peat_mesh::beacon::HierarchyLevel` | `peat-mesh/src/beacon/types.rs` |
| Schema (proto) | `peat_protocol::peat_schema::{capability, node, cell, hierarchy}::v1` | `peat-schema/proto/*.proto` |

### See also

- [`spec/004-coordination.md`](../../spec/004-coordination.md) — normative coordination protocol (formation, election, hierarchy, roles).
- [ADR-024](../../adr/024-flexible-hierarchy-strategies.md) — flexible hierarchy strategies (how tiers are formed).
- [ADR-066](../../adr/066-abstract-hierarchy-vocabulary.md) — the Cell/Cohort/Federation/Coalition vocabulary.
- [ADR-064](../../adr/064-deployment-formation-fallthrough.md) — deployment-time formation assignment policy.
- [DEVELOPER_GUIDE.md](./DEVELOPER_GUIDE.md) — the broader developer guide this fits within.
