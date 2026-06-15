//! Generic CRDT key-value documents (CRDT-over-Automerge-over-BLE).
//!
//! One Automerge doc per named collection (e.g. `nodes`, `commands`, `cells`,
//! `mission`). Each record is stored as `ROOT[recordId] =
//! <record-as-JSON-string>` — a scalar string at a top-level key. This is the
//! simplest model that merges correctly across replicas WITHOUT the genesis
//! trick the Counter needs:
//!   - `ROOT` is universal in Automerge (same object on every doc), so two
//!     replicas putting DIFFERENT keys merge as a clean set-union — no
//!     conflict.
//!   - Same-key updates (e.g. a command going pending -> completed) are
//!     last-writer-wins on that one key, which is what we want (one fulfiller).
//!   - Merge is commutative/associative/idempotent, so relay loops, duplicate,
//!     stale, out-of-order, and dropped frames are all safe; a later snapshot
//!     catches a node up.
//!
//! The doc's `save()` bytes ride the dedicated `0xAF`/transport-2 crdt frame
//! (peat-btle relays it mesh-wide). NO lite-bridge. See
//! docs/crdt-counter-over-ble.md.

use automerge::{transaction::Transactable, Automerge, ReadDoc, ScalarValue, ROOT};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Manager for the per-collection Automerge KV docs, persisted under `dir`.
pub(crate) struct CrdtKvDocs {
    docs: Mutex<HashMap<String, Automerge>>,
    dir: PathBuf,
}

impl CrdtKvDocs {
    pub(crate) fn new(dir: PathBuf) -> Self {
        CrdtKvDocs {
            docs: Mutex::new(HashMap::new()),
            dir,
        }
    }

    fn path(&self, coll: &str) -> PathBuf {
        let safe: String = coll
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("kv-{safe}.automerge"))
    }

    fn load_doc(&self, coll: &str) -> Automerge {
        std::fs::read(self.path(coll))
            .ok()
            .and_then(|b| Automerge::load(&b).ok())
            .unwrap_or_else(Automerge::new)
    }

    fn persist(&self, coll: &str, doc: &Automerge) {
        if let Some(parent) = self.path(coll).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(self.path(coll), doc.save());
    }

    /// Run `f` against the (lazily-loaded) doc for `coll`. Does NOT persist.
    fn with_doc<R>(&self, coll: &str, f: impl FnOnce(&mut Automerge) -> R) -> R {
        let mut map = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        let doc = map
            .entry(coll.to_string())
            .or_insert_with(|| self.load_doc(coll));
        f(doc)
    }

    /// Upsert `key = value_json` (value_json must be a valid JSON value, stored
    /// verbatim as a string scalar). Returns the doc's `save()` bytes to
    /// broadcast.
    pub(crate) fn put(&self, coll: &str, key: &str, value_json: &str) -> Vec<u8> {
        let mut map = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        let doc = map
            .entry(coll.to_string())
            .or_insert_with(|| self.load_doc(coll));
        let _ = doc.transact::<_, _, automerge::AutomergeError>(|tx| {
            tx.put(ROOT, key, value_json.to_string())?;
            Ok(())
        });
        let bytes = doc.save();
        self.persist(coll, doc);
        bytes
    }

    /// All records as a JSON object string: `{ "<key>": <value>, ... }` where
    /// each value is the verbatim JSON stored under that key. Dart decodes this
    /// into the record list.
    pub(crate) fn all_json(&self, coll: &str) -> String {
        self.with_doc(coll, |doc| {
            let mut out = String::from("{");
            let mut first = true;
            for key in doc.keys(ROOT) {
                if let Ok(Some((automerge::Value::Scalar(s), _))) = doc.get(ROOT, &key) {
                    if let ScalarValue::Str(v) = s.as_ref() {
                        if !first {
                            out.push(',');
                        }
                        first = false;
                        let k = serde_json::to_string(&key).unwrap_or_else(|_| "\"\"".to_string());
                        out.push_str(&k);
                        out.push(':');
                        out.push_str(v); // v is already valid JSON
                    }
                }
            }
            out.push('}');
            out
        })
    }

    /// Merge an inbound peer doc (its `save()` bytes). Idempotent/commutative.
    pub(crate) fn merge(&self, coll: &str, bytes: &[u8]) {
        let mut incoming = match Automerge::load(bytes) {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut map = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        let doc = map
            .entry(coll.to_string())
            .or_insert_with(|| self.load_doc(coll));
        let _ = doc.merge(&mut incoming);
        self.persist(coll, doc);
    }

    /// Current `save()` bytes for `coll`, for periodic re-broadcast (catch-up).
    pub(crate) fn snapshot(&self, coll: &str) -> Vec<u8> {
        self.with_doc(coll, |doc| doc.save())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "peat-kv-test-{name}-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn concurrent_adds_to_different_keys_union() {
        // Two replicas each add a different record; after exchange, both have both.
        let a = CrdtKvDocs::new(tmpdir("a"));
        let b = CrdtKvDocs::new(tmpdir("b"));
        let a_bytes = a.put("commands", "req-1", r#"{"from":"alpha","qty":5}"#);
        let b_bytes = b.put("commands", "req-2", r#"{"from":"bravo","qty":3}"#);
        a.merge("commands", &b_bytes);
        b.merge("commands", &a_bytes);
        let aj = a.all_json("commands");
        let bj = b.all_json("commands");
        assert!(
            aj.contains("req-1") && aj.contains("req-2"),
            "A has both: {aj}"
        );
        assert!(
            bj.contains("req-1") && bj.contains("req-2"),
            "B has both: {bj}"
        );
    }

    #[test]
    fn same_key_update_is_last_writer() {
        // A pending command updated to completed; the update propagates.
        let a = CrdtKvDocs::new(tmpdir("u-a"));
        let b = CrdtKvDocs::new(tmpdir("u-b"));
        a.put("commands", "req-1", r#"{"status":"pending"}"#);
        b.merge("commands", &a.snapshot("commands"));
        let upd = a.put("commands", "req-1", r#"{"status":"completed"}"#);
        b.merge("commands", &upd);
        assert!(
            b.all_json("commands").contains("completed"),
            "{}",
            b.all_json("commands")
        );
    }

    #[test]
    fn merge_idempotent_and_reloads() {
        let dir = tmpdir("idem");
        {
            let a = CrdtKvDocs::new(dir.clone());
            let bytes = a.put("nodes", "n1", r#"{"name":"alpha"}"#);
            a.merge("nodes", &bytes);
            a.merge("nodes", &bytes);
            assert!(a.all_json("nodes").contains("alpha"));
        }
        let a2 = CrdtKvDocs::new(dir);
        assert!(
            a2.all_json("nodes").contains("alpha"),
            "persisted across reload"
        );
    }
}
