//! Trait definitions for decoupling org-mode sync from concrete storage backends.
//!
//! The org crate never imports Loro or Turso types directly. All interaction
//! goes through these traits, which are implemented in the DI wiring layer.

use anyhow::Result;
use async_trait::async_trait;
use holon::sync::Document;
use holon_api::block::Block;
use holon_api::EntityUri;

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
    async fn iter_documents_with_blocks(&self) -> Vec<(EntityUri, Vec<Block>)>;

    /// Check if any of the given block IDs already exist under a DIFFERENT document.
    /// Returns Vec<(block_id, existing_parent_id)> for conflicts found.
    async fn find_foreign_blocks(
        &self,
        block_ids: &[EntityUri],
        expected_doc_uri: &EntityUri,
    ) -> Result<Vec<(EntityUri, EntityUri)>>;
}

/// CRUD operations on Document entities.
///
/// Convenience methods (`name_chain`, `find_by_name_chain`, `get_or_create_by_name_chain`)
/// are file-format agnostic — reusable by org, markdown, or any file-based adapter.
#[async_trait]
pub trait DocumentManager: Send + Sync {
    /// Find a document by parent_id and name.
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        name: &str,
    ) -> Result<Option<Document>>;

    /// Create a new document.
    async fn create(&self, doc: Document) -> Result<Document>;

    /// Get a document by its ID.
    async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Document>>;

    /// Walk parent chain to root, return name segments: ["projects", "todo"]
    async fn name_chain(&self, doc_id: &EntityUri) -> Result<Vec<String>> {
        let mut chain = Vec::new();
        let mut current_id = doc_id.clone();

        loop {
            let doc = self.get_by_id(&current_id).await?.ok_or_else(|| {
                anyhow::anyhow!("Document '{}' not found in name_chain", current_id)
            })?;

            if doc.id == EntityUri::doc_root() || doc.parent_id.is_sentinel() {
                break;
            }

            chain.push(doc.name.clone());
            current_id = doc.parent_id.clone();
        }

        chain.reverse();
        Ok(chain)
    }

    /// Resolve a name chain to a Document: ["projects", "todo"] → Document
    async fn find_by_name_chain(&self, chain: &[&str]) -> Result<Option<Document>> {
        let mut current_parent_id = EntityUri::doc_root();
        let mut current_doc: Option<Document> = None;

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

    /// Get or create the full chain, creating intermediate documents as needed.
    async fn get_or_create_by_name_chain(&self, chain: &[&str]) -> Result<Document> {
        assert!(!chain.is_empty(), "name chain must not be empty");

        let mut current_parent_id = EntityUri::doc_root();
        let mut current_doc: Option<Document> = None;

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
                    let new_doc = Document::new(
                        EntityUri::doc_random(),
                        current_parent_id.clone(),
                        segment.to_string(),
                    );
                    let created = self.create(new_doc).await?;
                    current_parent_id = created.id.clone();
                    current_doc = Some(created);
                }
            }
        }

        Ok(current_doc.unwrap())
    }
}
