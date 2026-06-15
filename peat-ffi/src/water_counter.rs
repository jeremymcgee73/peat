//! Shared water-supply Counter — a self-contained Automerge CRDT document.
//!
//! The document holds a single `Counter` scalar at `ROOT["liters"]`, replicated
//! to every node. Its `save()` bytes ride the existing BLE frame bus; on
//! receipt a node `merge`s them. Automerge merge is **commutative, associative,
//! and idempotent**, so:
//!   - relay loops / duplicate frames are no-ops,
//!   - out-of-order or dropped frames reconcile (a later snapshot catches up),
//!   - concurrent increments from N nodes **sum** (no last-writer-wins
//!     clobber).
//!
//! This replaces the interim per-node-doc + client-side-summing counter and the
//! whole class of identity/dedup/membership/flicker bugs that came with it.
//! See docs/crdt-counter-over-ble.md.

use automerge::{transaction::Transactable, ActorId, Automerge, ReadDoc, ScalarValue, ROOT};
use std::path::PathBuf;
use std::sync::Mutex;

const COUNTER_FIELD: &str = "liters";

/// Fixed actor id used ONLY to create the counter object, identically on every
/// node. This is the crux of making the Automerge `Counter` sum across
/// replicas: the counter must be a SINGLE shared object. If each node created
/// its own `counter(0)` with its own actor, those would be CONFLICTING
/// assignments that merge by last-writer-wins (increments lost), not summed. By
/// creating the counter with the same actor + same first op everywhere, every
/// node's genesis change has the same hash (deduped on merge) and the counter
/// has one shared object id; per-node increments (each node's OWN random actor)
/// then all target that object and sum. Must be exactly 16 bytes.
const GENESIS_ACTOR: &[u8; 16] = b"peatwatergenesis";

/// A persisted Automerge document holding the shared `liters` Counter.
pub(crate) struct WaterCounter {
    doc: Mutex<Automerge>,
    /// Where `save()` bytes are persisted so the value survives a node restart.
    path: PathBuf,
}

impl WaterCounter {
    /// Always start from the deterministic genesis (shared counter object),
    /// then merge any persisted local state on top. Persisted state and peer
    /// docs are themselves genesis-based, so the genesis change dedups and all
    /// prior increments are retained.
    pub(crate) fn load_or_init(path: PathBuf) -> Self {
        let mut doc = Self::new_doc();
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(mut persisted) = Automerge::load(&bytes) {
                let _ = doc.merge(&mut persisted);
            }
        }
        WaterCounter {
            doc: Mutex::new(doc),
            path,
        }
    }

    /// A fresh doc whose `liters` Counter is created by the fixed genesis actor
    /// (identical first op everywhere -> shared counter object), after which
    /// the doc switches to a unique per-node actor so THIS node's
    /// increments are its own and sum with peers'.
    fn new_doc() -> Automerge {
        let mut doc = Automerge::new();
        doc.set_actor(ActorId::from(&GENESIS_ACTOR[..]));
        let _ = doc.transact::<_, _, automerge::AutomergeError>(|tx| {
            tx.put(ROOT, COUNTER_FIELD, ScalarValue::counter(0))?;
            Ok(())
        });
        doc.set_actor(ActorId::random());
        doc
    }

    fn read_value(doc: &Automerge) -> i64 {
        match doc.get(ROOT, COUNTER_FIELD) {
            Ok(Some((automerge::Value::Scalar(s), _))) => match s.as_ref() {
                ScalarValue::Counter(c) => i64::from(c),
                ScalarValue::Int(i) => *i,
                ScalarValue::Uint(u) => *u as i64,
                _ => 0,
            },
            _ => 0,
        }
    }

    fn persist(&self, doc: &mut Automerge) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.path, doc.save());
    }

    /// Current merged value of the shared counter.
    pub(crate) fn value(&self) -> i64 {
        let doc = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        Self::read_value(&doc)
    }

    /// Apply `delta` to the Counter, persist, and return the doc's `save()`
    /// bytes for the caller to broadcast.
    pub(crate) fn increment(&self, delta: i64) -> Vec<u8> {
        let mut doc = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        let _ = doc.transact::<_, _, automerge::AutomergeError>(|tx| {
            tx.increment(ROOT, COUNTER_FIELD, delta)?;
            Ok(())
        });
        let bytes = doc.save();
        self.persist(&mut doc);
        bytes
    }

    /// Merge an incoming peer doc (its `save()` bytes), persist, return the new
    /// value. Idempotent + commutative — safe with duplicate / stale / relayed
    /// bytes. Unparseable input is ignored (returns the current value).
    pub(crate) fn merge(&self, bytes: &[u8]) -> i64 {
        let mut incoming = match Automerge::load(bytes) {
            Ok(d) => d,
            Err(_) => return self.value(),
        };
        let mut doc = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        let _ = doc.merge(&mut incoming);
        let v = Self::read_value(&doc);
        self.persist(&mut doc);
        v
    }

    /// Current `save()` bytes for periodic re-broadcast (late-joiner catch-up).
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        let mut doc = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        doc.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "peat-water-test-{name}-{:?}.automerge",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn concurrent_increments_merge_to_sum() {
        // Two independent replicas each add water; after exchanging docs both
        // converge to the SUM (the CRDT property the interim model never had).
        let a = WaterCounter::load_or_init(tmp("a"));
        let b = WaterCounter::load_or_init(tmp("b"));

        let a_bytes = a.increment(5); // A: +5
        let b_bytes = b.increment(7); // B: +7 (concurrent)

        let a_val = a.merge(&b_bytes); // A merges B
        let b_val = b.merge(&a_bytes); // B merges A

        assert_eq!(a_val, 12, "A should see 5+7");
        assert_eq!(b_val, 12, "B should see 5+7");
        assert_eq!(a.value(), 12);
        assert_eq!(b.value(), 12);
    }

    #[test]
    fn merge_is_idempotent() {
        // Re-merging the same bytes (relay loop / duplicate frame) must not
        // double-count.
        let a = WaterCounter::load_or_init(tmp("idem-a"));
        let b = WaterCounter::load_or_init(tmp("idem-b"));
        let b_bytes = b.increment(9);

        assert_eq!(a.merge(&b_bytes), 9);
        assert_eq!(
            a.merge(&b_bytes),
            9,
            "second merge of same bytes is a no-op"
        );
        assert_eq!(a.merge(&b_bytes), 9, "third merge too");
    }

    #[test]
    fn decrement_and_then_merge() {
        let a = WaterCounter::load_or_init(tmp("dec-a"));
        let b = WaterCounter::load_or_init(tmp("dec-b"));
        a.increment(10);
        b.increment(-3); // consume
        let v = a.merge(&b.snapshot());
        assert_eq!(v, 7, "10 + (-3)");
    }

    #[test]
    fn value_survives_reload() {
        let path = tmp("reload");
        {
            let c = WaterCounter::load_or_init(path.clone());
            c.increment(4);
            c.increment(3);
            assert_eq!(c.value(), 7);
        }
        // Re-open from disk.
        let c2 = WaterCounter::load_or_init(path);
        assert_eq!(c2.value(), 7, "value persisted across reload");
    }
}
