//! In-flight `(doc_key → origin)` map used by `IrohDocumentStore` to thread
//! origin labels from `upsert_with_origin` calls through to the broadcast-driven
//! observer that fetches docs out of the backend after a notification.
//!
//! ADR-059 §"Origin propagation through async observer pipelines": the iroh
//! observer task is decoupled in time and call stack from the upsert that
//! produced its trigger, so origin must be stashed at upsert time and recovered
//! at observation time. This map is the stash.
//!
//! ## Why a per-key FIFO queue, not a single slot
//!
//! Each `upsert_with_origin` triggers exactly one broadcast notification, and
//! the observer task pops one origin per notification. Rapid same-key upserts
//! (e.g. a watch publishing position updates every 250ms for one peripheral)
//! therefore produce N stashed origins that pair with N observer events 1:1.
//! A single-slot design would surface only the latest origin on the first
//! observer event and `None` on the rest — the original Slice 1.b.2.2 e2e
//! `rapid_same_key_burst_keeps_origin_present` test caught this.
//!
//! ## Bounding
//!
//! - **Hard cap of [`MAX_ENTRIES`] total entries across all keys**; on overflow
//!   the globally-oldest entry is evicted.
//! - **30-second TTL**; expired entries are swept lazily on each `insert`
//!   (no separate timer task — keeps the design dependency-free).
//!
//! Because a queue is FIFO and entries within a queue arrive in monotonically
//! increasing time order, the per-queue TTL sweep is O(k) where k is the
//! number of newly-expired entries on that queue.
//!
//! ## Known limitation: multi-peer interleaving (PR #808 QA Review, ARCH)
//!
//! The iroh backend's broadcast (`AutomergeStore::observer_tx`, peat-mesh
//! v0.9.0-rc.4) fires for **every** storage mutation — local
//! `coll.upsert` *and* remote-sync deltas applied via `sync_document` —
//! keyed only by `doc_key`. Nothing in the broadcast distinguishes
//! "this notification came from the local upsert that just stashed
//! origin" from "this notification came from a remote peer's Automerge
//! ops landing on the same key." Under interleaved load on a gateway
//! tablet (BLE-ingested `track-001` upserts concurrent with a remote
//! peer's `track-001` updates over the same window), the FIFO can:
//!
//! - **Mis-attribute remote-sync as BLE-origin** → `TransportManager`
//!   wrongly suppresses the BLE fan-out, so BLE peers miss legitimate
//!   remote updates.
//! - **Mis-attribute local BLE-origin upsert as `None`** (queue drained
//!   by an interleaving notification) → echo break fails; the BLE-origin
//!   doc is re-encoded out the BLE sink, forming the
//!   `BLE → Node → observer → BLE` loop.
//!
//! `BleTranslator::has_ble_marker` (a `_ble_marker: true` document field
//! stamped on the BLE encode path) is the load-bearing safety net that
//! prevents the second outcome in practice — but ADR-059 §"Schema impact"
//! deliberately keeps origin **off** the document, so the field marker
//! is *not* the ADR's invariant. A future LoRa/SBD transport won't have
//! an analogue.
//!
//! The architectural fix is to extend `peat-mesh`'s broadcast to carry
//! `(doc_key, origin)` inline so the observer can read attribution
//! without correlation. Tracked in #809; scheduled for peat-mesh rc.5
//! ahead of ADR-059 Slice 2 (`allowed_transports` fan-out + delete
//! propagation), at which point this whole module is deleted. Until
//! then: this map + `has_ble_marker` is the best-effort attribution
//! path, and the `rapid_same_key_burst_keeps_origin_present` e2e test
//! covers the single-node N→N pairing that is correct under those
//! constraints.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Hard cap on stashed entries (across all keys). Beyond this, the
/// globally-oldest entry is evicted on insert. Sized for ~1k concurrent
/// in-flight upserts; far above any realistic burst (BLE peripheral count
/// is O(10s), iroh peers O(100s)).
const MAX_ENTRIES: usize = 1024;

/// Default TTL — 30s covers steady-state observer lag plus broadcast-channel
/// lagged-rescan windows comfortably while bounding stale-state retention.
const DEFAULT_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Entry {
    origin: Option<String>,
    inserted_at: Instant,
}

/// Bounded TTL'd map of `doc_key → FIFO queue of origins` for the iroh
/// observer pipeline.
pub(crate) struct PendingOrigins {
    map: Mutex<HashMap<String, VecDeque<Entry>>>,
    ttl: Duration,
}

impl PendingOrigins {
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// Construct with a custom TTL. Used by tests to verify sweep behavior
    /// without sleeping for the production 30s window.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Stash an origin for `doc_key`. Sweeps expired entries first; if the
    /// total entry count exceeds [`MAX_ENTRIES`] after insert, evicts the
    /// globally-oldest entry until at the cap.
    pub fn insert(&self, doc_key: String, origin: Option<String>) {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap();

        // Lazy TTL sweep. Each per-key queue is FIFO so we can break out
        // of the per-queue loop on the first non-expired entry.
        sweep_expired(&mut map, now, self.ttl);

        map.entry(doc_key).or_default().push_back(Entry {
            origin,
            inserted_at: now,
        });

        cap_total(&mut map, MAX_ENTRIES);
    }

    /// Pop the next stashed origin for `doc_key` in insertion order.
    /// Returns `None` if no entry exists or the popped entry is expired.
    pub fn pop(&self, doc_key: &str) -> Option<String> {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap();
        let queue = map.get_mut(doc_key)?;
        let entry = queue.pop_front()?;
        if queue.is_empty() {
            map.remove(doc_key);
        }
        if now.saturating_duration_since(entry.inserted_at) > self.ttl {
            return None;
        }
        entry.origin
    }

    /// Total entry count across all keys. Used by tests; not exposed
    /// publicly to discourage callers from reasoning about internal state.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().values().map(|q| q.len()).sum()
    }
}

impl Default for PendingOrigins {
    fn default() -> Self {
        Self::new()
    }
}

fn sweep_expired(map: &mut HashMap<String, VecDeque<Entry>>, now: Instant, ttl: Duration) {
    map.retain(|_, queue| {
        while let Some(front) = queue.front() {
            if now.saturating_duration_since(front.inserted_at) > ttl {
                queue.pop_front();
            } else {
                break;
            }
        }
        !queue.is_empty()
    });
}

fn cap_total(map: &mut HashMap<String, VecDeque<Entry>>, max: usize) {
    let total: usize = map.values().map(|q| q.len()).sum();
    if total <= max {
        return;
    }
    let mut overage = total - max;
    while overage > 0 {
        // Find the key whose queue front is the globally-oldest entry.
        // Scoped block so the immutable borrow ends before the mutable
        // get_mut below.
        let oldest_key = {
            map.iter()
                .filter_map(|(k, q)| q.front().map(|e| (k.clone(), e.inserted_at)))
                .min_by_key(|(_, t)| *t)
                .map(|(k, _)| k)
        };
        let Some(k) = oldest_key else { break };
        let queue = match map.get_mut(&k) {
            Some(q) => q,
            None => break,
        };
        queue.pop_front();
        if queue.is_empty() {
            map.remove(&k);
        }
        overage -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_pop_returns_origin() {
        let p = PendingOrigins::new();
        p.insert("tracks:doc-1".into(), Some("ble".into()));
        assert_eq!(p.pop("tracks:doc-1"), Some("ble".into()));
    }

    #[test]
    fn pop_consumes_entry() {
        let p = PendingOrigins::new();
        p.insert("tracks:doc-1".into(), Some("ble".into()));
        let _ = p.pop("tracks:doc-1");
        assert_eq!(
            p.pop("tracks:doc-1"),
            None,
            "second pop must be empty (consumed on read)"
        );
    }

    #[test]
    fn pop_absent_key_returns_none() {
        let p = PendingOrigins::new();
        assert_eq!(p.pop("nope"), None);
    }

    #[test]
    fn rapid_same_key_drains_in_fifo_order() {
        // N upserts to the same key produce N entries; pop drains them
        // in insertion order. This is the invariant that pairs each
        // observer broadcast notification with its originating upsert.
        let p = PendingOrigins::new();
        for i in 0..5 {
            p.insert("tracks:doc-1".into(), Some(format!("origin-{}", i)));
        }
        assert_eq!(p.len(), 5);
        for i in 0..5 {
            assert_eq!(p.pop("tracks:doc-1"), Some(format!("origin-{}", i)));
        }
        assert_eq!(p.pop("tracks:doc-1"), None, "queue exhausted");
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn empty_queue_removed_after_drain() {
        // Internal cleanup invariant: once the queue for a key is empty,
        // its map entry must be removed so `len()` is accurate.
        let p = PendingOrigins::new();
        p.insert("tracks:doc-1".into(), Some("ble".into()));
        let _ = p.pop("tracks:doc-1");
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn ttl_sweep_drops_expired_on_next_insert() {
        let p = PendingOrigins::with_ttl(Duration::from_millis(50));
        p.insert("tracks:expires".into(), Some("ble".into()));
        std::thread::sleep(Duration::from_millis(80));
        p.insert("tracks:fresh".into(), Some("iroh".into()));

        assert_eq!(p.len(), 1, "expired entry must be swept on next insert");
        assert_eq!(
            p.pop("tracks:expires"),
            None,
            "expired entry must not be poppable"
        );
        assert_eq!(p.pop("tracks:fresh"), Some("iroh".into()));
    }

    #[test]
    fn pop_returns_none_for_expired_entry() {
        let p = PendingOrigins::with_ttl(Duration::from_millis(20));
        p.insert("tracks:doc-1".into(), Some("ble".into()));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(p.pop("tracks:doc-1"), None);
    }

    #[test]
    fn bounded_cap_evicts_oldest_when_full() {
        // Insert MAX_ENTRIES + extras with no consumer; total entry
        // count must stay ≤ MAX_ENTRIES, and the earliest entries are gone.
        let p = PendingOrigins::new();
        for i in 0..(MAX_ENTRIES + 100) {
            p.insert(format!("tracks:doc-{}", i), Some("ble".into()));
        }
        assert!(
            p.len() <= MAX_ENTRIES,
            "total {} exceeds cap {}",
            p.len(),
            MAX_ENTRIES
        );
        // First 100 evicted; most recent ones survive.
        assert_eq!(p.pop("tracks:doc-0"), None);
        assert_eq!(
            p.pop(&format!("tracks:doc-{}", MAX_ENTRIES + 99)),
            Some("ble".into())
        );
    }

    #[test]
    fn bounded_cap_evicts_oldest_across_keys() {
        // Mixed-key burst: per-queue FIFO + global eviction order means
        // the oldest entry across the whole map is what gets dropped,
        // not the oldest within a single key. Key A's queue front is the
        // globally-oldest, so its earliest entries get evicted first
        // when key B's later inserts push over the cap.
        let p = PendingOrigins::new();
        for i in 0..MAX_ENTRIES {
            p.insert("a".into(), Some(format!("a-{}", i)));
        }
        for i in 0..50 {
            p.insert("b".into(), Some(format!("b-{}", i)));
        }

        assert!(p.len() <= MAX_ENTRIES);

        // Key B's 50 entries are all newer than key A's earliest 50,
        // so B is intact end-to-end.
        for i in 0..50 {
            assert_eq!(
                p.pop("b"),
                Some(format!("b-{}", i)),
                "b queue must be intact in FIFO order"
            );
        }
        // Key A: first 50 evicted, "a-50" .. "a-(MAX_ENTRIES-1)" remain.
        assert_eq!(
            p.pop("a"),
            Some("a-50".to_string()),
            "key a must have dropped its first 50 entries"
        );
    }

    #[test]
    fn none_origin_round_trips() {
        // origin=None is a meaningful value (sync-delivered events, etc.).
        let p = PendingOrigins::new();
        p.insert("tracks:doc-1".into(), None);
        assert_eq!(p.pop("tracks:doc-1"), None);
        // Consumed — second pop is also None, but for "absent" reasons.
        assert_eq!(p.pop("tracks:doc-1"), None);
        assert_eq!(p.len(), 0);
    }
}
