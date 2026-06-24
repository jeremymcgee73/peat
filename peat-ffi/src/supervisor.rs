//! Per-peer reconnect supervisor — the state + backoff policy that keeps a live
//! path up to each known roster member.
//!
//! This module is pure policy. It owns the per-peer dial state machine
//! (`Idle → Connecting → Connected`, with `Backoff` between failed attempts) and
//! decides *whether* to dial a given peer *now*. It deliberately knows nothing
//! about transports, iroh, or async: the orchestration in `lib.rs` reads the
//! live connected-peer set, asks this supervisor which roster members are
//! eligible, spawns the actual dials, and reports each outcome back.
//!
//! Keeping the policy transport-agnostic does two things. It makes the logic
//! unit-testable with a clock passed in (no sleeps, no sockets). And it keeps
//! the cross-transport dedup honest: "connected" here means connected over ANY
//! transport, so a peer already reachable on BLE is reported connected and won't
//! also be dialed over the relay. The orchestrator feeds the union of all
//! transports' connected sets into [`Supervisor::reconcile`].

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// First retry delay after a failed dial.
const BACKOFF_BASE_MS: u64 = 2_000;
/// Ceiling on the (pre-jitter) exponential backoff.
const BACKOFF_MAX_MS: u64 = 300_000; // 5 min
/// A dial stuck in `Connecting` longer than this is treated as dead and becomes
/// eligible again — defends against a dial future that never resolves (the
/// underlying connect has its own ~5s timeout, so this is a generous backstop).
const CONNECTING_STUCK_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not connected and not currently dialing — eligible immediately.
    Idle,
    /// A dial is in flight (started at `since_ms`).
    Connecting,
    /// Connected over some transport — nothing to do.
    Connected,
    /// Backing off after a failure — eligible again at `next_ms`.
    Backoff,
}

#[derive(Debug, Clone, Copy)]
struct PeerDial {
    phase: Phase,
    /// Consecutive failed attempts; resets on success. Drives backoff growth.
    attempts: u32,
    /// For `Backoff`: unix-ms when the peer becomes eligible again.
    next_ms: u64,
    /// For `Connecting`: unix-ms the dial started (stuck detection).
    since_ms: u64,
}

impl Default for PeerDial {
    fn default() -> Self {
        PeerDial {
            phase: Phase::Idle,
            attempts: 0,
            next_ms: 0,
            since_ms: 0,
        }
    }
}

/// Tracks dial state for every known/seen peer, keyed by `node_id`.
pub(crate) struct Supervisor {
    peers: Mutex<HashMap<String, PeerDial>>,
}

impl Supervisor {
    pub(crate) fn new() -> Self {
        Supervisor {
            peers: Mutex::new(HashMap::new()),
        }
    }

    /// Reconcile the live connected set (node_ids connected over ANY transport)
    /// against tracked state. Returns the node_ids that NEWLY became connected
    /// this round (were not already `Connected`), so the caller can stamp their
    /// roster `last_seen`. Peers previously `Connected` but now absent from the
    /// live set drop back to `Idle` (eligible to re-dial immediately).
    pub(crate) fn reconcile(&self, connected: &HashSet<String>) -> Vec<String> {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let mut newly = Vec::new();
        for id in connected {
            let entry = map.entry(id.clone()).or_default();
            if entry.phase != Phase::Connected {
                newly.push(id.clone());
            }
            entry.phase = Phase::Connected;
            entry.attempts = 0;
        }
        for (id, entry) in map.iter_mut() {
            if entry.phase == Phase::Connected && !connected.contains(id) {
                entry.phase = Phase::Idle;
            }
        }
        newly
    }

    /// Whether `node_id` should be dialed at `now_ms`. Idle peers are always
    /// eligible; backing-off peers once their delay elapses; a dial stuck in
    /// `Connecting` past [`CONNECTING_STUCK_MS`]; connected peers never.
    pub(crate) fn eligible(&self, node_id: &str, now_ms: u64) -> bool {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(node_id.to_string()).or_default();
        match entry.phase {
            Phase::Idle => true,
            Phase::Connected => false,
            Phase::Backoff => now_ms >= entry.next_ms,
            Phase::Connecting => now_ms.saturating_sub(entry.since_ms) >= CONNECTING_STUCK_MS,
        }
    }

    /// Mark that a dial is starting for `node_id`.
    pub(crate) fn mark_connecting(&self, node_id: &str, now_ms: u64) {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(node_id.to_string()).or_default();
        entry.phase = Phase::Connecting;
        entry.since_ms = now_ms;
    }

    /// Mark a successful dial: connected, failure counter reset.
    pub(crate) fn mark_connected(&self, node_id: &str) {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(node_id.to_string()).or_default();
        entry.phase = Phase::Connected;
        entry.attempts = 0;
    }

    /// Mark a failed dial: schedule the next attempt with exponential backoff
    /// (capped) plus a per-peer deterministic jitter so a group that all fail at
    /// once doesn't re-dial in lockstep (thundering herd).
    pub(crate) fn mark_failed(&self, node_id: &str, now_ms: u64) {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(node_id.to_string()).or_default();
        entry.attempts = entry.attempts.saturating_add(1);
        let backoff = backoff_ms(entry.attempts, node_id);
        entry.phase = Phase::Backoff;
        entry.next_ms = now_ms.saturating_add(backoff);
    }

    /// Make every backing-off peer immediately eligible again with a fresh
    /// attempt ladder. Called when external conditions change such that a prior
    /// failure is likely stale — the network came up, or the app returned to
    /// foreground — so there's no reason to keep waiting out a backoff computed
    /// against the old conditions. Leaves `Connected` and in-flight
    /// `Connecting` peers untouched (resetting Connecting would spawn a
    /// duplicate dial alongside the one still running).
    pub(crate) fn reset_backoff_all(&self) {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        for entry in map.values_mut() {
            if entry.phase == Phase::Backoff {
                entry.phase = Phase::Idle;
                entry.attempts = 0;
            }
        }
    }

    /// Drop tracking for peers that are neither in `keep` (roster ∪ connected)
    /// so the map doesn't grow unbounded as the roster churns.
    pub(crate) fn retain(&self, keep: &HashSet<String>) {
        let mut map = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|id, _| keep.contains(id));
    }
}

/// Exponential backoff with cap + deterministic per-peer jitter.
///
/// `attempts` is 1-based (the first failure → ~BASE). Jitter is derived from the
/// node_id hash, not a RNG, so it is stable per peer and spread across the group;
/// it adds up to ~20% of the delay. Determinism is fine here — the goal is to
/// de-correlate peers from each other, not to be unpredictable.
fn backoff_ms(attempts: u32, node_id: &str) -> u64 {
    let shift = attempts.saturating_sub(1).min(8); // 2^8 * base = 512s, well past the cap
    let base = BACKOFF_BASE_MS.saturating_mul(1u64 << shift);
    let capped = base.min(BACKOFF_MAX_MS);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    node_id.hash(&mut h);
    let jitter = h.finish() % (capped / 5 + 1);
    capped.saturating_add(jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn idle_peer_is_eligible() {
        let s = Supervisor::new();
        assert!(s.eligible("aa", 0), "unseen peer starts Idle → eligible");
    }

    #[test]
    fn connecting_blocks_redial_until_stuck() {
        let s = Supervisor::new();
        s.mark_connecting("aa", 1_000);
        assert!(
            !s.eligible("aa", 1_000),
            "in-flight dial blocks a second dial"
        );
        assert!(
            !s.eligible("aa", 1_000 + CONNECTING_STUCK_MS - 1),
            "still blocked just before the stuck threshold"
        );
        assert!(
            s.eligible("aa", 1_000 + CONNECTING_STUCK_MS),
            "a stuck Connecting becomes eligible again"
        );
    }

    #[test]
    fn connected_peer_not_eligible() {
        let s = Supervisor::new();
        s.mark_connected("aa");
        assert!(!s.eligible("aa", 10_000));
    }

    #[test]
    fn backoff_grows_and_caps() {
        // Same peer, rising attempt count → non-decreasing delay, capped.
        let mut prev = 0;
        for attempts in 1..=12u32 {
            let d = backoff_ms(attempts, "node-x");
            assert!(d >= prev, "backoff is non-decreasing: {d} >= {prev}");
            // cap + up to 20% jitter
            assert!(
                d <= BACKOFF_MAX_MS + BACKOFF_MAX_MS / 5 + 1,
                "delay within cap+jitter: {d}"
            );
            prev = d.min(BACKOFF_MAX_MS);
        }
    }

    #[test]
    fn jitter_decorrelates_peers() {
        // Different node_ids at the same attempt get different delays (the whole
        // point of the per-peer jitter), so they won't re-dial in lockstep.
        let a = backoff_ms(5, "alpha");
        let b = backoff_ms(5, "bravo");
        assert_ne!(a, b);
    }

    #[test]
    fn failed_then_eligible_after_delay() {
        let s = Supervisor::new();
        s.mark_failed("aa", 1_000);
        assert!(
            !s.eligible("aa", 1_000),
            "not eligible immediately after failure"
        );
        // After the max backoff window has surely elapsed, it's eligible again.
        assert!(s.eligible("aa", 1_000 + BACKOFF_MAX_MS + BACKOFF_MAX_MS));
    }

    #[test]
    fn reconcile_reports_newly_connected_once() {
        let s = Supervisor::new();
        let first = s.reconcile(&set(&["aa", "bb"]));
        assert_eq!(first.len(), 2, "both newly connected on first sighting");
        let second = s.reconcile(&set(&["aa", "bb"]));
        assert!(
            second.is_empty(),
            "already-connected peers are not re-reported"
        );
    }

    #[test]
    fn reconcile_demotes_dropped_peer_to_eligible() {
        let s = Supervisor::new();
        s.reconcile(&set(&["aa"]));
        assert!(!s.eligible("aa", 5_000), "connected → not eligible");
        s.reconcile(&set(&[])); // aa dropped out of the live set
        assert!(
            s.eligible("aa", 6_000),
            "dropped peer becomes eligible to re-dial"
        );
    }

    #[test]
    fn success_resets_backoff() {
        let s = Supervisor::new();
        s.mark_failed("aa", 0);
        s.mark_failed("aa", 0); // attempts = 2
        s.mark_connected("aa");
        s.reconcile(&set(&[])); // drop it so it's Idle again, attempts should be 0
        s.mark_failed("aa", 0); // first failure after reset
                                // First-failure delay must be back at the base band, not the 2-attempt band.
        let first_delay = backoff_ms(1, "aa");
        let two_delay = backoff_ms(2, "aa");
        assert!(first_delay < two_delay, "attempts reset after a success");
    }

    #[test]
    fn reset_backoff_makes_failed_peers_eligible() {
        let s = Supervisor::new();
        s.mark_failed("aa", 1_000);
        s.mark_failed("bb", 1_000);
        assert!(!s.eligible("aa", 1_000), "backing off before reset");
        s.reset_backoff_all();
        assert!(s.eligible("aa", 1_000), "eligible immediately after reset");
        assert!(s.eligible("bb", 1_000));
    }

    #[test]
    fn reset_backoff_leaves_connecting_and_connected_alone() {
        let s = Supervisor::new();
        s.mark_connecting("aa", 1_000);
        s.mark_connected("bb");
        s.reset_backoff_all();
        // aa is still mid-dial (not stuck yet) so not eligible; bb still connected.
        assert!(
            !s.eligible("aa", 1_000),
            "in-flight dial not disturbed by reset"
        );
        assert!(
            !s.eligible("bb", 1_000),
            "connected peer not disturbed by reset"
        );
    }

    #[test]
    fn retain_prunes_unknown_peers() {
        let s = Supervisor::new();
        s.mark_failed("aa", 0);
        s.mark_failed("bb", 0);
        s.retain(&set(&["aa"]));
        // bb pruned → back to default Idle/eligible; aa kept in Backoff.
        assert!(s.eligible("bb", 0), "pruned peer resets to default");
        assert!(!s.eligible("aa", 0), "kept peer retains its Backoff");
    }
}
