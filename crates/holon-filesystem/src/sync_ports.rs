//! Consumer-side ports for the format-agnostic file-sync engine.
//!
//! `FileSyncController` reads/writes blocks and documents, and registers
//! doc-id → path aliases, only through these traits — it never imports a
//! concrete storage backend (Loro, Turso, in-memory). The impls live in the
//! DI wiring layer of each backend crate.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::block::Block;

/// Read-only access to blocks, organized by document.
///
/// The org crate uses this to render blocks → org text without knowing
/// how blocks are stored (Loro, in-memory, SQL, etc.).
#[async_trait]
pub trait BlockReader: Send + Sync {
    /// Get all blocks for a document by its ID.
    async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>>;

    /// List all known documents with their blocks (for startup initialization).
    /// Returns (doc_id, blocks) pairs. Path resolution is the caller's concern.
    async fn iter_documents_with_blocks(&self) -> Result<Vec<(EntityUri, Vec<Block>)>>;

    /// Load `(file_id, content_hash)` pairs from the `file` table. Used by
    /// `FileSyncController` at startup to populate the projection-hash cache
    /// (`last_projection_hash`) before CDC has replayed file events into
    /// the in-process cache — so we can short-circuit on-disk-unchanged
    /// re-ingests without doing a single block-table SQL op.
    ///
    /// Returns the URI form (e.g. `file:projects/todo.org`) since the
    /// caller knows its own root directory and can resolve to a concrete
    /// `CanonicalPath`. Default impl returns empty so backends without a
    /// `file` table (in-memory tests) don't have to implement it.
    async fn load_file_hashes(&self) -> Result<Vec<(EntityUri, String)>> {
        Ok(Vec::new())
    }

    /// Phase 1 write-back: persist `content_hash` for a file row so the
    /// next boot's `load_file_hashes` returns the new value and the
    /// fast-path engages. Returns `Ok(())` whether the row existed or
    /// not — `OrgmodeSyncProvider` is the authoritative creator of file
    /// rows, and on first ingest the row may not yet exist; the next
    /// provider sync will create it and we'll succeed on a later
    /// re-ingest. Default impl is a no-op for backends without a `file`
    /// table.
    async fn persist_file_hash(&self, _: &EntityUri, _: &str) -> Result<()> {
        Ok(())
    }

    /// Phase 5 keystone: wait until the convergent block feed
    /// (`LiveData<Block>`, the `block` matview CDC stream) contains every id in
    /// `block_ids`. Returns `true` if all are present before `timeout_ms`,
    /// `false` on timeout.
    ///
    /// This replaced the `event_acks` watermark wait
    /// (`wait_for_cache_caught_up`): that wait was only a *timestamp proxy*
    /// for "the projection that produced these blocks has landed", whereas
    /// this is a *positional* predicate over the actual rows — it cannot
    /// return before the rows are visible, so it closes the load-bearing
    /// stale-cache re-render race (R1) without the watermark machinery.
    /// Default impl returns `true` for backends without a feed (in-memory
    /// tests).
    async fn wait_for_blocks_in_feed(&self, _: &[String], _: u64) -> bool {
        true
    }

    /// Check if any of the given block IDs already exist under a DIFFERENT
    /// document. Returns Vec<(block_id, owning_doc_uri)> for conflicts
    /// found.
    ///
    /// Default implementation uses `iter_documents_with_blocks()` to correctly
    /// attribute nested blocks to their document root (not just direct parent).
    async fn find_foreign_blocks(
        &self,
        block_ids: &[EntityUri],
        expected_doc_uri: &EntityUri,
    ) -> Result<Vec<(EntityUri, EntityUri)>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }

        let id_set: std::collections::HashSet<&EntityUri> = block_ids.iter().collect();
        let documents = self.iter_documents_with_blocks().await?;

        let mut conflicts = Vec::new();
        for (doc_uri, blocks) in &documents {
            if doc_uri == expected_doc_uri {
                continue;
            }
            for block in blocks {
                if id_set.contains(&block.id) {
                    conflicts.push((block.id.clone(), doc_uri.clone()));
                }
            }
        }
        Ok(conflicts)
    }
}

/// CRUD operations on page blocks (blocks tagged `"Page"`).
///
/// Convenience methods (`name_chain`, `find_by_name_chain`,
/// `get_or_create_by_name_chain`) are file-format agnostic — reusable by org,
/// markdown, or any file-based adapter. The "name" of a page is now its title:
/// the first line of `content`.
#[async_trait]
pub trait DocumentManager: Send + Sync {
    /// Find a page block by parent_id and title (first line of content).
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> Result<Option<Block>>;

    /// Create a new page block.
    async fn create(&self, doc: Block) -> Result<Block>;

    /// Get a page block by its ID.
    async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Block>>;

    /// Update page metadata (e.g. todo_keywords) on the page block.
    async fn update_metadata(&self, doc: &Block) -> Result<()>;

    /// Walk parent chain to root, return title segments: ["projects", "todo"]
    async fn name_chain(&self, doc_id: &EntityUri) -> Result<Vec<String>> {
        let mut chain = Vec::new();
        let mut current_id = doc_id.clone();

        loop {
            if current_id == EntityUri::no_parent() || current_id.is_sentinel() {
                break;
            }

            let doc = self.get_by_id(&current_id).await?.ok_or_else(|| {
                anyhow::anyhow!("Page block '{}' not found in name_chain", current_id)
            })?;

            if doc.is_page() {
                chain.push(doc.title());
            }
            current_id = doc.parent_id.clone();
        }

        chain.reverse();
        Ok(chain)
    }

    /// Resolve a title chain to a page Block: ["projects", "todo"] → Block
    async fn find_by_name_chain(&self, chain: &[&str]) -> Result<Option<Block>> {
        let mut current_parent_id = EntityUri::no_parent();
        let mut current_doc: Option<Block> = None;

        for segment in chain {
            match self
                .find_by_parent_and_name(&current_parent_id, segment)
                .await?
            {
                Some(doc) => {
                    current_parent_id = doc.id.clone();
                    current_doc = Some(doc);
                }
                None => return Ok(None),
            }
        }

        Ok(current_doc)
    }

    /// Get or create the full chain, creating intermediate page blocks as
    /// needed.
    async fn get_or_create_by_name_chain(&self, chain: &[&str]) -> Result<Block> {
        assert!(!chain.is_empty(), "name chain must not be empty");

        let mut current_parent_id = EntityUri::no_parent();
        let mut current_doc: Option<Block> = None;

        for segment in chain {
            match self
                .find_by_parent_and_name(&current_parent_id, segment)
                .await?
            {
                Some(existing) => {
                    current_parent_id = existing.id.clone();
                    current_doc = Some(existing);
                }
                None => {
                    let mut new_doc = Block::new_text(
                        EntityUri::block_random(),
                        current_parent_id.clone(),
                        segment.to_string(),
                    );
                    new_doc.set_page(true);
                    let created = self.create(new_doc).await?;
                    current_parent_id = created.id.clone();
                    current_doc = Some(created);
                }
            }
        }

        Ok(current_doc.unwrap())
    }
}

/// Read/write binary image data for blocks.
///
/// Decoupled from Loro — the DI layer provides the concrete implementation.
/// The org sync controller uses this to materialize image files to disk
/// (block → file) and ingest them back (file → block).
#[async_trait]
pub trait ImageDataProvider: Send + Sync {
    /// Read image bytes for a block. Returns None if no data is stored.
    async fn read_image_data(&self, block_id: &EntityUri) -> Result<Option<Vec<u8>>>;

    /// Store image bytes for a block.
    async fn write_image_data(&self, block_id: &EntityUri, data: Vec<u8>) -> Result<()>;
}

/// Callback for registering doc_id → path aliases in the storage layer.
/// Implemented by Loro wiring; the controller itself doesn't know about Loro.
#[async_trait]
pub trait AliasRegistrar: Send + Sync {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path);
    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf>;
}

/// 3-way text-content merge for the no-store conflict path (SqlOnly /
/// `Consolidator::Store` mode).
///
/// When an org-file edit races a UI edit to the SAME block's text content, the
/// store-owner mode has no live CRDT to merge them, so a naive ingest resolves
/// by whole-value last-writer-wins — one side's characters are lost. The
/// merge-fidelity ladder (`docs/Architecture/Model.md`) says this mode must
/// instead degrade to a base-3-way TEXT merge: the org reconciler already holds
/// the common ancestor (the last-projected snapshot, read through the
/// [`BaseStore`](crate::BaseStore) seam), so it feeds `(base, theirs, mine)`
/// through this seam and ingests the merged text.
///
/// The merge is performed by a **transient** CRDT text — a throwaway document
/// spun up per conflict and dropped afterwards (Model.md: *transient = merge
/// function only*, never stored, never cached). The concrete impl lives in
/// holon-loro; holon-filesystem stays storage-agnostic and never depends on
/// Loro (holon-loro already depends on holon-filesystem — the reverse would be
/// a cycle).
pub trait ThreeWayTextMerge: Send + Sync {
    /// Merge concurrent edits of one block's text content. `base` is the common
    /// ancestor (last-projected snapshot from the BaseStore), `theirs` the
    /// on-disk edit, `mine` the current store content. Returns the merged text.
    /// Callers invoke this only when BOTH `theirs` and `mine` differ from
    /// `base` (a genuine concurrent edit); the non-conflict cases never reach
    /// here.
    fn merge_text(&self, base: &str, theirs: &str, mine: &str) -> Result<String>;
}
