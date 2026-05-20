use anyhow::Result;
use loro::{LoroDoc, PeerID};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

pub struct LoroDocument {
    doc: Arc<LoroDoc>,
    peer_id: PeerID,
    doc_id: String,
}

impl LoroDocument {
    pub fn new(doc_id: String) -> Result<Self> {
        // PBTs need a deterministic primary peer_id so the reference model's
        // `loro_merge_text` prediction (which hardcodes peer_a=1, peer_b=2)
        // matches actual production merge behaviour. RGA tiebreaks concurrent
        // inserts at the same position by peer_id (lower wins), so a random
        // primary u64 + peer_idx+100 peers can flip ordering relative to the
        // reference's peer_a=1 < peer_b=2 assumption.
        let peer_id = match std::env::var("HOLON_LORO_PEER_ID") {
            Ok(s) => s
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("HOLON_LORO_PEER_ID must be a u64: {e}"))?,
            Err(_) => rand::random::<u64>(),
        };
        let doc = LoroDoc::new();
        // Install the rich-text mark policy (Bold/Italic/.../Link/Verbatim
        // expand types). Must run before any LoroText is created or marked,
        // and is a no-op if re-called — see `configure_text_styles` doc.
        crate::api::loro_backend::configure_text_styles(&doc);
        doc.set_peer_id(peer_id)?;

        info!(
            "Created LoroDocument '{}' with peer_id: {}",
            doc_id, peer_id
        );

        Ok(Self {
            doc: Arc::new(doc),
            peer_id,
            doc_id,
        })
    }

    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    /// Wrap an already-constructed `Arc<LoroDoc>` into a `LoroDocument`.
    /// Used by tests and `BlockCellRegistry::with_loro_doc` (test helper)
    /// to share a doc that was set up directly via the loro crate.
    pub fn from_existing(doc: Arc<LoroDoc>, doc_id: impl Into<String>) -> Self {
        let peer_id = doc.peer_id();
        Self {
            doc,
            peer_id,
            doc_id: doc_id.into(),
        }
    }

    pub fn peer_id(&self) -> PeerID {
        self.peer_id
    }

    /// Override the peer_id (used by IrohSyncAdapter to set Iroh-derived ID).
    pub fn set_peer_id(&mut self, peer_id: PeerID) -> Result<()> {
        self.peer_id = peer_id;
        self.doc.set_peer_id(peer_id)?;
        Ok(())
    }

    pub fn insert_text(&self, container: &str, index: usize, text: &str) -> Result<Vec<u8>> {
        let text_obj = self.doc.get_text(container);
        text_obj.insert(index, text)?;
        Ok(self
            .doc
            .export(loro::ExportMode::updates_owned(Default::default()))?)
    }

    pub fn get_text(&self, container: &str) -> Result<String> {
        let text_obj = self.doc.get_text(container);
        Ok(text_obj.to_string())
    }

    pub fn apply_update(&self, update: &[u8]) -> Result<()> {
        self.apply_update_with_origin("reconcile", update)
    }

    pub fn apply_update_with_origin(&self, origin: &str, update: &[u8]) -> Result<()> {
        self.doc.import_with(update, origin)?;
        debug!("Applied update of {} bytes", update.len());
        Ok(())
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.doc.export(loro::ExportMode::Snapshot)?)
    }

    pub fn with_read<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&LoroDoc) -> Result<R>,
    {
        f(&self.doc)
    }

    pub fn with_write<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&LoroDoc) -> Result<R>,
    {
        self.with_write_origin("ui_local", f)
    }

    pub fn with_write_origin<F, R>(&self, origin: &str, f: F) -> Result<R>
    where
        F: FnOnce(&LoroDoc) -> Result<R>,
    {
        self.doc.set_next_commit_origin(origin);
        let result = f(&self.doc)?;

        // Diagnostic only: exporting the owned update log is O(doc-size), and
        // this ran on EVERY write purely to log a byte count — making bulk
        // writes O(N²) (a 614-block org-file scan spent ~11s here, dominating
        // cold start). Gate behind the debug level so production (warn/info)
        // skips the export entirely; the commit itself already happened in the
        // closure, so this has no functional effect.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let updates = self
                .doc
                .export(loro::ExportMode::updates_owned(Default::default()))?;
            if !updates.is_empty() {
                debug!("Write committed, {} bytes to sync", updates.len());
            }
        }

        Ok(result)
    }

    pub fn doc(&self) -> Arc<LoroDoc> {
        self.doc.clone()
    }

    /// Export a history-compacted snapshot: current state plus no op history
    /// before the current frontiers (`ExportMode::shallow_snapshot`, the same
    /// mode `shared_tree::gc_after_extraction` uses). Safe for Holon because
    /// undo replays the persistent inverse-command log, not Loro history.
    /// Peers whose version vector predates the trim cannot receive an
    /// incremental delta; `IrohSyncAdapter` detects that and ships a full
    /// snapshot instead.
    pub fn export_compact_snapshot(&self) -> Result<Vec<u8>> {
        self.doc.commit();
        let frontiers = self.doc.oplog_frontiers();
        Ok(self
            .doc
            .export(loro::ExportMode::shallow_snapshot(&frontiers))?)
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let snapshot = self.export_snapshot()?;
        write_atomic(path, &snapshot)?;
        debug!("Saved LoroDoc snapshot to {}", path.display());
        Ok(())
    }

    /// Like [`save_to_file`] but writes a history-compacted snapshot
    /// ([`export_compact_snapshot`]).
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn save_compact_to_file(&self, path: &Path) -> Result<()> {
        let snapshot = self.export_compact_snapshot()?;
        let len = snapshot.len();
        write_atomic(path, &snapshot)?;
        debug!(
            "Saved compacted LoroDoc snapshot to {} ({} bytes)",
            path.display(),
            len
        );
        Ok(())
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn load_from_file(path: &Path, doc_id: String) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        // See `LoroDocument::new` for why this honours HOLON_LORO_PEER_ID.
        let peer_id = match std::env::var("HOLON_LORO_PEER_ID") {
            Ok(s) => s
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("HOLON_LORO_PEER_ID must be a u64: {e}"))?,
            Err(_) => rand::random::<u64>(),
        };

        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id)?;
        doc.import(&bytes)?;

        info!(
            "Loaded LoroDocument '{}' from {} with peer_id: {}",
            doc_id,
            path.display(),
            peer_id
        );

        Ok(Self {
            doc: Arc::new(doc),
            peer_id,
            doc_id,
        })
    }
}

/// Crash-safe file write: temp file in the same directory, then rename.
/// A crash mid-save previously truncated the snapshot (plain `fs::write`),
/// losing the whole document store.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_loro_document() -> Result<()> {
        let doc = LoroDocument::new("test-doc".to_string())?;
        assert_ne!(doc.peer_id().to_string(), "");
        assert_eq!(doc.doc_id(), "test-doc");
        Ok(())
    }

    #[test]
    fn test_text_operations() -> Result<()> {
        let doc = LoroDocument::new("test-doc".to_string())?;

        doc.insert_text("editor", 0, "Hello")?;
        let text = doc.get_text("editor")?;
        assert_eq!(text, "Hello");

        doc.insert_text("editor", 5, " World")?;
        let text = doc.get_text("editor")?;
        assert_eq!(text, "Hello World");

        Ok(())
    }

    #[test]
    fn test_update_export_and_apply() -> Result<()> {
        let doc1 = LoroDocument::new("shared-doc".to_string())?;
        let doc2 = LoroDocument::new("shared-doc".to_string())?;

        let update = doc1.insert_text("editor", 0, "Collaborative")?;

        doc2.apply_update(&update)?;

        let text1 = doc1.get_text("editor")?;
        let text2 = doc2.get_text("editor")?;

        assert_eq!(text1, text2);
        assert_eq!(text1, "Collaborative");

        Ok(())
    }

    #[test]
    fn test_concurrent_edits_merge() -> Result<()> {
        let doc1 = LoroDocument::new("shared-doc".to_string())?;
        let doc2 = LoroDocument::new("shared-doc".to_string())?;

        let update1 = doc1.insert_text("editor", 0, "Hello")?;
        doc2.apply_update(&update1)?;

        let update2a = doc1.insert_text("editor", 5, " from doc1")?;
        let update2b = doc2.insert_text("editor", 5, " from doc2")?;

        doc1.apply_update(&update2b)?;
        doc2.apply_update(&update2a)?;

        let text1 = doc1.get_text("editor")?;
        let text2 = doc2.get_text("editor")?;

        assert_eq!(text1, text2);
        assert!(text1.contains("Hello"));

        Ok(())
    }

    #[test]
    fn test_different_documents_isolated() -> Result<()> {
        let doc_a = LoroDocument::new("doc-a".to_string())?;
        let doc_b = LoroDocument::new("doc-b".to_string())?;

        doc_a.insert_text("editor", 0, "Document A")?;
        doc_b.insert_text("editor", 0, "Document B")?;

        let text_a = doc_a.get_text("editor")?;
        let text_b = doc_b.get_text("editor")?;

        assert_eq!(text_a, "Document A");
        assert_eq!(text_b, "Document B");

        Ok(())
    }

    #[test]
    fn test_origin_tagging_ui_local_via_with_write() -> Result<()> {
        let doc = LoroDocument::new("origin-test".to_string())?;
        let origin_seen = Arc::new(std::sync::Mutex::new(None::<String>));
        let origin_seen_clone = origin_seen.clone();

        let _sub = doc.doc().subscribe_root(Arc::new(move |event| {
            if let Ok(mut seen) = origin_seen_clone.lock()
                && seen.is_none()
            {
                *seen = Some(event.origin.to_string());
            }
        }));

        doc.with_write(|d| {
            let tree = d.get_tree("test_tree");
            tree.enable_fractional_index(0);
            let _node = tree.create(None)?;
            Ok(())
        })?;

        let seen = origin_seen.lock().unwrap();
        assert_eq!(
            seen.as_deref(),
            Some("ui_local"),
            "with_write should tag origin as 'ui_local'"
        );
        Ok(())
    }

    #[test]
    fn test_origin_tagging_reconcile_via_apply_update() -> Result<()> {
        let doc1 = LoroDocument::new("origin-test-1".to_string())?;
        let doc2 = LoroDocument::new("origin-test-2".to_string())?;

        // Create content in doc1
        doc1.with_write(|d| {
            let tree = d.get_tree("test_tree");
            tree.enable_fractional_index(0);
            let _node = tree.create(None)?;
            Ok(())
        })?;
        let snapshot = doc1.export_snapshot()?;

        let origin_seen = Arc::new(std::sync::Mutex::new(None::<String>));
        let origin_seen_clone = origin_seen.clone();

        let _sub = doc2.doc().subscribe_root(Arc::new(move |event| {
            if let Ok(mut seen) = origin_seen_clone.lock()
                && seen.is_none()
            {
                *seen = Some(event.origin.to_string());
            }
        }));

        doc2.apply_update(&snapshot)?;

        let seen = origin_seen.lock().unwrap();
        assert_eq!(
            seen.as_deref(),
            Some("reconcile"),
            "apply_update should tag origin as 'reconcile'"
        );
        Ok(())
    }

    #[test]
    fn test_origin_tagging_custom_via_with_write_origin() -> Result<()> {
        let doc = LoroDocument::new("origin-test-custom".to_string())?;
        let origin_seen = Arc::new(std::sync::Mutex::new(None::<String>));
        let origin_seen_clone = origin_seen.clone();

        let _sub = doc.doc().subscribe_root(Arc::new(move |event| {
            if let Ok(mut seen) = origin_seen_clone.lock()
                && seen.is_none()
            {
                *seen = Some(event.origin.to_string());
            }
        }));

        doc.with_write_origin("org_reload", |d| {
            let tree = d.get_tree("test_tree_2");
            tree.enable_fractional_index(0);
            let _node = tree.create(None)?;
            Ok(())
        })?;

        let seen = origin_seen.lock().unwrap();
        assert_eq!(
            seen.as_deref(),
            Some("org_reload"),
            "with_write_origin should pass through the custom origin"
        );
        Ok(())
    }

    #[test]
    fn compact_save_round_trips_state_and_shrinks_history() -> Result<()> {
        let dir = tempfile::TempDir::new()?;
        let doc = LoroDocument::new("compact-test".to_string())?;
        // Many small edits build up op history that compaction can shed.
        for i in 0..200 {
            doc.insert_text("content", 0, &format!("edit-{i} "))?;
        }
        let expected = doc.get_text("content")?;

        let full_path = dir.path().join("full.loro");
        let compact_path = dir.path().join("compact.loro");
        doc.save_to_file(&full_path)?;
        doc.save_compact_to_file(&compact_path)?;

        let full_size = std::fs::metadata(&full_path)?.len();
        let compact_size = std::fs::metadata(&compact_path)?.len();
        assert!(
            compact_size < full_size,
            "compacted snapshot ({compact_size}B) should be smaller than full ({full_size}B)"
        );

        // Both formats reload to identical current state.
        let from_full = LoroDocument::load_from_file(&full_path, "from-full".to_string())?;
        let from_compact = LoroDocument::load_from_file(&compact_path, "from-compact".to_string())?;
        assert_eq!(from_full.get_text("content")?, expected);
        assert_eq!(from_compact.get_text("content")?, expected);

        // A doc reloaded from a compacted snapshot keeps working and saving.
        from_compact.insert_text("content", 0, "post-reload ")?;
        let resaved = dir.path().join("resaved.loro");
        from_compact.save_compact_to_file(&resaved)?;
        let reloaded = LoroDocument::load_from_file(&resaved, "reloaded".to_string())?;
        assert_eq!(
            reloaded.get_text("content")?,
            format!("post-reload {expected}")
        );
        Ok(())
    }
}
