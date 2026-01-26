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
        let peer_id = rand::random::<u64>();
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

        let updates = self
            .doc
            .export(loro::ExportMode::updates_owned(Default::default()))?;

        if !updates.is_empty() {
            debug!("Write committed, {} bytes to sync", updates.len());
        }

        Ok(result)
    }

    pub fn doc(&self) -> Arc<LoroDoc> {
        self.doc.clone()
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let snapshot = self.export_snapshot()?;
        std::fs::write(path, snapshot)?;
        debug!("Saved LoroDoc snapshot to {}", path.display());
        Ok(())
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub fn load_from_file(path: &Path, doc_id: String) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let peer_id = rand::random::<u64>();

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
            if let Ok(mut seen) = origin_seen_clone.lock() {
                if seen.is_none() {
                    *seen = Some(event.origin.to_string());
                }
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
            if let Ok(mut seen) = origin_seen_clone.lock() {
                if seen.is_none() {
                    *seen = Some(event.origin.to_string());
                }
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
            if let Ok(mut seen) = origin_seen_clone.lock() {
                if seen.is_none() {
                    *seen = Some(event.origin.to_string());
                }
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
}
