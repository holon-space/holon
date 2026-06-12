//! Per-component last-synced *base* snapshot for 3-way block-sync diffing.
//!
//! The Loro→SQL projection historically diffed the live Loro authority against
//! the live SQL sink. That couples the diff to a transient, lossy cache: a
//! momentarily-incomplete sink row looks "deleted" (CDC churn), a lossy
//! `sort_key`/`properties` round-trip looks "changed" (update churn), and a
//! SQL-only delete that left the node alive in Loro gets *resurrected*.
//!
//! [`SyncBaseStore`] holds the **last state the projection actually wrote** —
//! a stable base, in Loro's own representation. Diffing the live Loro authority
//! against this base (never the cache) means re-projecting an unchanged
//! snapshot emits zero ops, and the cache's transient state can't drive the
//! diff. (Phase 3 of the block-sync rework.)
//!
//! One global doc today; the keying is a forward seam for per-`(peer, file)`
//! bases. In-memory with sidecar persistence (so the base survives restart and
//! the first post-boot projection doesn't re-emit the whole tree).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::loro_backend::SnapshotBlock;
#[cfg(test)]
use holon_api::block::Block;
use tracing::{info, warn};

/// Sidecar filename for the persisted base snapshot, alongside the Loro
/// frontiers sidecar (`SIDECAR_FILENAME`).
pub const BASE_SIDECAR_FILENAME: &str = "holon_tree.sync_base.json";

/// Identifies *whose* base for *which* file. A forward seam for the
/// multi-master replication model (`docs/Architecture/Replication.md`): each
/// component keeps a per-`(peer, file)` base it diffs against. The concrete
/// [`SyncBaseStore`] is one global doc today, so the single in-tree impl
/// ignores the key — but the [`BaseStore`] trait speaks the keyed form from the
/// start so Phase 3+ can introduce domain-scoped bases without touching every
/// call site.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BaseKey {
    pub peer: String,
    pub file: String,
}

impl BaseKey {
    /// The single global base used while there is one Loro doc for the whole
    /// tree (the Loro→SQL projection's base). Coexists with per-`(peer, file)`
    /// keys used by domain-scoped consumers (the org reconciler).
    pub fn global() -> Self {
        Self {
            peer: "global".to_string(),
            file: "*".to_string(),
        }
    }

    /// A per-file base for the named peer (e.g. the org reconciler's base for
    /// one org file). `file` is the canonical path string.
    pub fn file(peer: &str, file: &str) -> Self {
        Self {
            peer: peer.to_string(),
            file: file.to_string(),
        }
    }

    /// Encode to a single string for sidecar storage (JSON object keys must be
    /// strings). `peer`/`file` are joined by NUL, which neither contains.
    fn encode(&self) -> String {
        format!("{}\u{0}{}", self.peer, self.file)
    }

    fn decode(s: &str) -> Option<Self> {
        s.split_once('\u{0}').map(|(peer, file)| Self {
            peer: peer.to_string(),
            file: file.to_string(),
        })
    }
}

/// The KV interface over a per-`(peer, file)` last-projected base.
///
/// Extracted over the existing concrete [`SyncBaseStore`] (Phase 2): the
/// projector and later phases speak `get_base`/`put_base` so the diff source
/// can be made domain-scoped without rewriting consumers. The concrete impl is
/// global today and ignores the key.
pub trait BaseStore: Send + Sync {
    /// The last-projected snapshot for `key` (empty when never seeded).
    /// Returned as a shared handle — bases are document-sized, so handing out
    /// deep copies per projection pass would copy the whole document.
    fn get_base(&self, key: &BaseKey) -> Arc<HashMap<String, SnapshotBlock>>;
    /// Replace the base for `key` and persist.
    fn put_base(&self, key: &BaseKey, snapshot: HashMap<String, SnapshotBlock>);
    /// Whether `key`'s base has been seeded at least once (distinguishes
    /// "empty tree" from "cold boot, not yet projected").
    fn is_base_seeded(&self, key: &BaseKey) -> bool;
}

/// Per-`(peer, file)` last-projected block snapshots (the diff "base"). The
/// outer map is keyed by [`BaseKey`]; the inner map is block-id → [`Block`]. A
/// key being *present* (even with an empty inner map) means "seeded" — that
/// distinguishes a genuinely-empty base from a cold-boot one not yet projected.
///
/// `sidecar_path` is `Some` for persisted bases (the Loro→SQL projection, so the
/// base survives restart) and `None` for in-memory bases (the org reconciler,
/// which re-seeds per file from the consolidated store each session).
pub struct SyncBaseStore {
    base: Mutex<HashMap<BaseKey, Arc<HashMap<String, SnapshotBlock>>>>,
    sidecar_path: Option<PathBuf>,
}

impl SyncBaseStore {
    /// Build a persisted base store whose sidecar lives next to
    /// `frontiers_sidecar`. Used by the Loro→SQL projection.
    pub fn from_frontiers_sidecar(frontiers_sidecar: &Path) -> Self {
        let sidecar_path = frontiers_sidecar.with_file_name(BASE_SIDECAR_FILENAME);
        let base = load_base(&sidecar_path);
        Self {
            base: Mutex::new(base),
            sidecar_path: Some(sidecar_path),
        }
    }

    /// Build an in-memory (non-persisted) base store. Used by the org reconciler,
    /// which re-seeds each `(peer, file)` base from the consolidated store on
    /// first touch per session.
    pub fn in_memory() -> Self {
        Self {
            base: Mutex::new(HashMap::new()),
            sidecar_path: None,
        }
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    fn persist(&self) {
        let Some(path) = self.sidecar_path.as_ref() else {
            return;
        };
        // Encode the struct-keyed map to a string-keyed one (JSON object keys
        // must be strings). Serialize through references under the lock —
        // cloning the snapshots here would copy the whole document per persist.
        let bytes = {
            let base = self.base.lock().unwrap();
            let encoded: HashMap<String, &HashMap<String, SnapshotBlock>> =
                base.iter().map(|(k, v)| (k.encode(), v.as_ref())).collect();
            match serde_json::to_vec(&encoded) {
                Ok(b) => b,
                Err(e) => {
                    warn!("[SyncBaseStore] failed to serialize base: {e}");
                    return;
                }
            }
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("[SyncBaseStore] create sidecar dir failed: {e}");
                return;
            }
        }
        if let Err(e) = std::fs::write(path, bytes) {
            warn!(
                "[SyncBaseStore] write sidecar {} failed: {e}",
                path.display()
            );
        }
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    fn persist(&self) {
        // wasm32 demo is in-memory; no sidecar persistence.
    }
}

impl BaseStore for SyncBaseStore {
    fn get_base(&self, key: &BaseKey) -> Arc<HashMap<String, SnapshotBlock>> {
        self.base
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    fn put_base(&self, key: &BaseKey, snapshot: HashMap<String, SnapshotBlock>) {
        self.base
            .lock()
            .unwrap()
            .insert(key.clone(), Arc::new(snapshot));
        self.persist();
    }

    fn is_base_seeded(&self, key: &BaseKey) -> bool {
        self.base.lock().unwrap().contains_key(key)
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn load_base(path: &Path) -> HashMap<BaseKey, Arc<HashMap<String, SnapshotBlock>>> {
    // The sidecar stores an encoded-key map (`peer\0file` → blocks). A sidecar
    // written by the pre-Phase-3 single-map format won't decode to this shape,
    // so it falls through to "empty" — self-healing: the first projection
    // re-seeds from the SQL sink (one extra cold-boot read).
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            warn!(
                "[SyncBaseStore] failed to read base sidecar {}: {e}",
                path.display()
            );
            return HashMap::new();
        }
    };
    match serde_json::from_slice::<HashMap<String, HashMap<String, SnapshotBlock>>>(&bytes) {
        Ok(encoded) => {
            let base: HashMap<BaseKey, Arc<HashMap<String, SnapshotBlock>>> = encoded
                .into_iter()
                .filter_map(|(k, v)| BaseKey::decode(&k).map(|key| (key, Arc::new(v))))
                .collect();
            info!(
                "[SyncBaseStore] loaded base from {} ({} key(s))",
                path.display(),
                base.len()
            );
            base
        }
        Err(e) => {
            warn!(
                "[SyncBaseStore] base sidecar at {} is corrupt/old format ({e}); starting unseeded",
                path.display()
            );
            HashMap::new()
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn load_base(_: &Path) -> HashMap<BaseKey, Arc<HashMap<String, SnapshotBlock>>> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::EntityUri;

    fn block(id: &str) -> Block {
        Block::new_text(EntityUri::block(id), EntityUri::no_parent(), id.to_string())
    }

    /// `ids` are BARE block ids (no scheme) — `block()` mints the `EntityUri`.
    /// The map is keyed by the schemed `EntityUri` string, mirroring production
    /// (`loro_backend::snapshot_blocks_from_doc_settled` keys by
    /// `block.id.to_string()`).
    fn snapshot(ids: &[&str]) -> HashMap<String, SnapshotBlock> {
        ids.iter()
            .map(|id| {
                let block = block(id);
                (
                    block.id.to_string(),
                    SnapshotBlock {
                        block,
                        sort_key: "A0".to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn unseeded_key_reports_not_seeded_and_empty() {
        let store = SyncBaseStore::in_memory();
        let key = BaseKey::file("org", "/a.org");
        assert!(!store.is_base_seeded(&key));
        assert!(store.get_base(&key).is_empty());
    }

    #[test]
    fn put_then_get_round_trips_per_key() {
        let store = SyncBaseStore::in_memory();
        let key = BaseKey::file("org", "/a.org");
        store.put_base(&key, snapshot(&["a", "b"]));
        assert!(store.is_base_seeded(&key));
        let got = store.get_base(&key);
        assert_eq!(got.len(), 2);
        assert!(got.contains_key("block:a"));
    }

    #[test]
    fn empty_snapshot_still_counts_as_seeded() {
        // "seeded with an empty tree" must be distinguishable from "never seeded".
        let store = SyncBaseStore::in_memory();
        let key = BaseKey::file("org", "/empty.org");
        store.put_base(&key, HashMap::new());
        assert!(store.is_base_seeded(&key));
        assert!(store.get_base(&key).is_empty());
    }

    #[test]
    fn keys_are_isolated() {
        // Two files (and the global projection key) keep separate bases — no
        // collision in the one store.
        let store = SyncBaseStore::in_memory();
        let a = BaseKey::file("org", "/a.org");
        let b = BaseKey::file("org", "/b.org");
        let g = BaseKey::global();
        store.put_base(&a, snapshot(&["a"]));
        store.put_base(&b, snapshot(&["b1", "b2"]));
        store.put_base(&g, snapshot(&["g"]));
        assert_eq!(store.get_base(&a).len(), 1);
        assert_eq!(store.get_base(&b).len(), 2);
        assert_eq!(store.get_base(&g).len(), 1);
        assert!(store.get_base(&a).contains_key("block:a"));
        assert!(!store.get_base(&a).contains_key("block:b1"));
    }

    #[test]
    fn base_key_encode_decode_round_trips() {
        let key = BaseKey::file("org", "/some/path with spaces.org");
        let decoded = BaseKey::decode(&key.encode()).unwrap();
        assert_eq!(decoded, key);
    }
}
