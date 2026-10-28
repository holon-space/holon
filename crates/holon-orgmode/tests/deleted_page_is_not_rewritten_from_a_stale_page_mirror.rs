//! A page whose file the user deleted is not written back while the page
//! mirror still lists it.
//!
//! Production's `DocumentManager` answers `get_by_id` from a matview mirror
//! that its own CDC stream updates, apart from the block-change stream that
//! drives write-back. After an external file deletion cascade-deletes the
//! page from the store, a block change of that page can arrive before the
//! mirror drops the page; `doc_id_to_path` then still derives the deleted
//! file, and the write-back recreates it.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

/// The store after the cascade: the page and its blocks are gone.
struct EmptyStore;

#[async_trait]
impl BlockReader for EmptyStore {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(Vec::new())
    }
    async fn doc_block_topology(
        &self,
        _: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(Vec::new())
    }
    async fn get_block_authoritative(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// The page mirror before its CDC row for the deletion arrived.
struct StalePageMirror {
    page: Block,
}

#[async_trait]
impl DocumentManager for StalePageMirror {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        holon_filesystem::page_at_position(std::iter::once(&self.page), parent_id, title)
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok((*id == self.page.id).then(|| self.page.clone()))
    }
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }
}

struct NoopOrdering;

#[async_trait]
impl BlockOrdering for NoopOrdering {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        Ok(())
    }
    async fn prev_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn next_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn first_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn last_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn children(&self, _: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(Vec::new())
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_late_block_change_of_a_deleted_page_does_not_recreate_its_file() {
    let tmp = tempfile::tempdir().unwrap();
    // macOS: the controller compares the `/private/var` spelling literally.
    let root: PathBuf = std::fs::canonicalize(tmp.path()).unwrap();
    let mut page = Block::new_text(
        EntityUri::block("gamma-page"),
        EntityUri::no_parent(),
        "gamma".to_string(),
    );
    page.set_page(true);
    let page_id = page.id.clone();
    let mut controller = new_org_sync_controller(
        Arc::new(EmptyStore),
        Arc::new(StalePageMirror { page }),
        root.clone(),
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    );

    controller
        .on_block_changed(
            &page_id,
            &BlockDelta::Remove(EntityUri::block("gamma-stays")),
        )
        .await
        .expect("a block change of a deleted page must not fail the sync loop");

    let gamma = root.join("gamma.org");
    assert!(
        !gamma.exists(),
        "write-back recreated {} for a page the store no longer holds: {:?}",
        gamma.display(),
        std::fs::read_to_string(&gamma).ok(),
    );
}
