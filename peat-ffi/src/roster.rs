//! Persistent roster of known group peers, for reconnection.
//!
//! When a node joins a group (members are exchanged out-of-band, e.g. by
//! scanning a join token), it remembers the other members so it can
//! re-establish connections after a restart, a network change, or a move
//! between transports. Discovery answers "who is in the group"; the roster is
//! the *persisted* answer, so the node still knows the group after its process
//! is killed and relaunched (e.g. an OS background-kill on mobile).
//!
//! The roster is keyed by `node_id` (the stable endpoint id) and scoped by
//! `group_id`. It holds only NON-SECRET reachability data — node id,
//! last-known direct addresses, an optional relay url, a display name, and a
//! last-seen timestamp. It never holds key material (the formation key that
//! authenticates a connection lives elsewhere), so it is persisted as plain
//! JSON and needs no at-rest encryption — there is nothing secret here for the
//! FIPS rule to protect.
//!
//! This is the foundation slice for the reconnect supervisor: the supervisor
//! will walk the roster and, per `node_id`, keep at least one live path up over
//! whichever transport is currently reachable.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One known peer in the roster.
///
/// `node_id` is the stable identity (it does not change when the peer's
/// network address changes); `addresses` and `relay_url` are best-effort
/// reachability hints that may go stale, which is why the reconnect path also
/// falls back to relay rendezvous and live discovery rather than trusting them
/// alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct RosterEntry {
    /// Hex-encoded node id (stable endpoint id) — the roster key.
    pub node_id: String,
    /// Group this peer belongs to.
    pub group_id: String,
    /// Human-readable name (best-effort, for UI).
    pub name: String,
    /// Last-known direct addresses (e.g. "192.168.1.5:19001"). May be stale.
    pub addresses: Vec<String>,
    /// Optional relay url for rendezvous when direct addresses fail.
    pub relay_url: Option<String>,
    /// Unix-millis of the last successful contact, or 0 if never yet connected.
    pub last_seen_ms: u64,
}

/// Persistent, in-memory-cached roster keyed by `node_id`.
///
/// Reads are served from the in-memory map; every mutation writes the whole
/// roster back to `<dir>/roster.json` via a temp-file + rename so a crash
/// mid-write can't truncate the live file. The roster is small (group-sized,
/// tens of entries), so rewriting it wholesale on each change is cheaper than
/// maintaining an incremental on-disk format.
pub(crate) struct RosterStore {
    entries: Mutex<HashMap<String, RosterEntry>>,
    path: PathBuf,
}

impl RosterStore {
    /// Open the roster persisted at `<dir>/roster.json`, loading existing
    /// entries if present. A missing *or corrupt* file yields an empty roster
    /// (rewritten on the next upsert) rather than an error — a node must still
    /// start even when the roster file is unreadable.
    pub(crate) fn load(dir: &Path) -> Self {
        let path = dir.join("roster.json");
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<RosterEntry>>(&b).ok())
            .map(|v| {
                v.into_iter()
                    .map(|e| (e.node_id.clone(), e))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        RosterStore {
            entries: Mutex::new(entries),
            path,
        }
    }

    /// Persist the whole roster as a sorted JSON array. Best-effort: a write
    /// failure leaves the previous file intact (we write a temp then rename).
    fn persist(&self, entries: &HashMap<String, RosterEntry>) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut list: Vec<&RosterEntry> = entries.values().collect();
        list.sort_by(|a, b| a.node_id.cmp(&b.node_id)); // stable on-disk order
        if let Ok(json) = serde_json::to_vec_pretty(&list) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Insert or update an entry (keyed by `node_id`) and persist. Idempotent.
    /// A re-upsert never moves `last_seen_ms` backwards: callers that only know
    /// reachability (not a fresh contact) pass `0`, and the stored, more-recent
    /// timestamp is preserved.
    pub(crate) fn upsert(&self, mut entry: RosterEntry) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = map.get(&entry.node_id) {
            if entry.last_seen_ms < existing.last_seen_ms {
                entry.last_seen_ms = existing.last_seen_ms;
            }
        }
        map.insert(entry.node_id.clone(), entry);
        self.persist(&map);
    }

    /// Remove an entry by `node_id`; persists only if something was removed.
    /// Returns whether an entry was present.
    pub(crate) fn remove(&self, node_id: &str) -> bool {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let removed = map.remove(node_id).is_some();
        if removed {
            self.persist(&map);
        }
        removed
    }

    /// Fetch a single entry by `node_id`.
    pub(crate) fn get(&self, node_id: &str) -> Option<RosterEntry> {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.get(node_id).cloned()
    }

    /// All entries, sorted by `node_id` for a stable order.
    pub(crate) fn list(&self) -> Vec<RosterEntry> {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<RosterEntry> = map.values().cloned().collect();
        v.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        v
    }

    /// Entries belonging to a single `group_id`.
    pub(crate) fn list_by_group(&self, group_id: &str) -> Vec<RosterEntry> {
        self.list()
            .into_iter()
            .filter(|e| e.group_id == group_id)
            .collect()
    }

    /// Record a successful contact: stamps `last_seen_ms` and persists. No-op
    /// if the node isn't in the roster (callers upsert before marking seen).
    ///
    /// Wired into the reconnect path: `try_dial_roster_peer` calls this on a
    /// successful dial, and `run_supervisor_tick` calls it when reconcile finds
    /// a roster member already connected over some transport.
    pub(crate) fn mark_seen(&self, node_id: &str, now_ms: u64) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = map.get_mut(node_id) {
            e.last_seen_ms = now_ms;
            self.persist(&map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "peat-roster-test-{name}-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn entry(node_id: &str, group: &str) -> RosterEntry {
        RosterEntry {
            node_id: node_id.to_string(),
            group_id: group.to_string(),
            name: format!("peer-{node_id}"),
            addresses: vec!["192.168.1.5:19001".to_string()],
            relay_url: Some("https://relay.example".to_string()),
            last_seen_ms: 0,
        }
    }

    #[test]
    fn upsert_get_remove_roundtrip() {
        let store = RosterStore::load(&tmpdir("crud"));
        store.upsert(entry("aa", "g1"));
        assert_eq!(store.get("aa").unwrap().group_id, "g1");
        assert!(store.remove("aa"));
        assert!(store.get("aa").is_none());
        assert!(!store.remove("aa"), "second remove reports absent");
    }

    #[test]
    fn upsert_updates_in_place_no_duplicates() {
        let store = RosterStore::load(&tmpdir("update"));
        store.upsert(entry("aa", "g1"));
        let mut updated = entry("aa", "g1");
        updated.addresses = vec!["10.0.0.9:5000".to_string()];
        store.upsert(updated);
        assert_eq!(store.list().len(), 1, "same node_id updates, not appends");
        assert_eq!(store.get("aa").unwrap().addresses, vec!["10.0.0.9:5000"]);
    }

    #[test]
    fn upsert_never_moves_last_seen_backwards() {
        let store = RosterStore::load(&tmpdir("lastseen"));
        let mut e = entry("aa", "g1");
        e.last_seen_ms = 1000;
        store.upsert(e);
        // A reachability-only re-upsert (last_seen 0) must not erase the stamp.
        store.upsert(entry("aa", "g1"));
        assert_eq!(store.get("aa").unwrap().last_seen_ms, 1000);
        store.mark_seen("aa", 2000);
        assert_eq!(store.get("aa").unwrap().last_seen_ms, 2000);
    }

    #[test]
    fn list_by_group_filters() {
        let store = RosterStore::load(&tmpdir("group"));
        store.upsert(entry("aa", "g1"));
        store.upsert(entry("bb", "g1"));
        store.upsert(entry("cc", "g2"));
        assert_eq!(store.list_by_group("g1").len(), 2);
        assert_eq!(store.list_by_group("g2").len(), 1);
        assert_eq!(store.list_by_group("none").len(), 0);
    }

    #[test]
    fn persists_across_reload() {
        let dir = tmpdir("reload");
        {
            let store = RosterStore::load(&dir);
            store.upsert(entry("aa", "g1"));
            store.mark_seen("aa", 4242);
        }
        let reopened = RosterStore::load(&dir);
        let e = reopened.get("aa").expect("survived reload");
        assert_eq!(e.last_seen_ms, 4242);
    }

    #[test]
    fn corrupt_file_yields_empty_roster_not_error() {
        let dir = tmpdir("corrupt");
        std::fs::write(dir.join("roster.json"), b"{ this is not json").unwrap();
        let store = RosterStore::load(&dir);
        assert_eq!(store.list().len(), 0);
        // ...and it recovers: a subsequent upsert rewrites a valid file.
        store.upsert(entry("aa", "g1"));
        assert_eq!(RosterStore::load(&dir).list().len(), 1);
    }
}
