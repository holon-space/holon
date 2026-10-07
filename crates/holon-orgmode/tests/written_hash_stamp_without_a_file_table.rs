//! A backend that does not say how it records the hash of a written file must
//! not let the stamp report the hash as recorded.
//!
//! @pbt kind harness
//! @pbt covers written-hash-stamp — a write-back whose hash the backend cannot
//! record is disclosed as unrecorded, never as recorded

#![cfg(feature = "di")]

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

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

/// Implements no `file` table method: the backend under test.
struct Store {
    blocks: Mutex<Vec<Block>>,
}

#[async_trait]
impl BlockReader for Store {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self.blocks.lock().unwrap().clone())
    }

    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .get_blocks(doc_id)
            .await?
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == *id)
            .cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        unreachable!("the write-back under test lists no documents")
    }
}

struct Docs {
    doc: Block,
}

#[async_trait]
impl DocumentManager for Docs {
    async fn find_by_parent_and_name(
        &self,
        _: &EntityUri,
        _: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }

    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok((self.doc.id == *id).then(|| self.doc.clone()))
    }

    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }

    async fn name_chain(&self, _: &EntityUri) -> anyhow::Result<Vec<String>> {
        Ok(vec!["doc".to_string()])
    }
}

struct Ordering {
    store: Arc<Store>,
}

#[async_trait]
impl BlockOrdering for Ordering {
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
    async fn children(&self, parent_id: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .store
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id.clone())
            .collect())
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

/// Records the two written-files signals; every other signal is irrelevant
/// here.
#[derive(Default)]
struct Disclosures(Mutex<Vec<String>>);

impl holon_filesystem::WritebackDisclosure for Disclosures {
    fn writeback_degraded(&self, _: &str) {}
    fn writeback_stalled(&self, _: &Path, _: &str) {}
    fn writeback_resumed(&self, _: &Path) {}
    fn ingest_refused(&self, _: &Path, _: &str, _: &str) {}
    fn ingest_recovered(&self, _: &Path) {}
    fn vault_file_emptied(&self, _: &Path) {}
    fn writeback_lossy(&self, _: &Path, _: &str) {}
    fn writeback_faithful(&self, _: &Path) {}
    fn block_in_two_files(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&Path>,
        _: &[&Path],
        _: bool,
    ) {
    }
    fn deleted_block_kept_in_file(&self, _: &EntityUri, _: &Path) {}
    fn deletion_undone(&self, _: &EntityUri, _: &Path, _: &[&Path]) {}
    fn undone_deletion_resolved(&self, _: &EntityUri) {}
    fn deletion_ended_by_edit(&self, _: &EntityUri, _: &Path) {}
    fn file_edit_overruled(&self, _: &EntityUri, _: &Path, _: &str, _: holon_api::HolonChange) {}
    fn vault_sync_not_started(&self, _: &Path, _: &str) {}
    fn vault_state_unreadable(&self, _: &Path, _: &Path, _: &str) {}
    fn vault_start_incomplete(&self, _: &Path, _: &str, _: &str) {}
    fn vault_backlog_ingesting(&self, _: &Path, _: usize, _: usize) {}
    fn vault_backlog_ingested(&self, _: &Path) {}
    fn written_files_unrecorded(&self, _: &Path, files: &[&Path], cause: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("unrecorded {files:?}: {cause}"));
    }
    fn written_files_recorded(&self, _: &Path) {
        self.0.lock().unwrap().push("recorded".to_string());
    }
    fn deleted_block_gone_from_file(&self, _: &EntityUri) {}
    fn block_in_one_file_again(&self, _: &EntityUri) {}
}

#[tokio::test]
async fn a_backend_without_a_hash_record_discloses_the_written_file_as_unrecorded() {
    let doc_id = EntityUri::block("stamp0000001");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "Doc");
    doc.set_page(true);
    let block = Block::new_text(EntityUri::block("b1"), doc_id.clone(), "First heading");
    let store = Arc::new(Store {
        blocks: Mutex::new(vec![block.clone()]),
    });
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let disclosures = Arc::new(Disclosures::default());
    let mut controller = new_org_sync_controller(
        store.clone(),
        Arc::new(Docs { doc }),
        root.clone(),
        Arc::new(Ordering {
            store: store.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_writeback_disclosure(disclosures.clone());

    controller
        .seed_holder_from_authority(&doc_id)
        .await
        .unwrap();
    let mut edited = block;
    edited.content = "First heading, edited".to_string();
    *store.blocks.lock().unwrap() = vec![edited.clone()];
    controller
        .on_block_changed(
            &doc_id,
            &BlockDelta::Upsert {
                block: edited,
                prev: None,
            },
        )
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(root.join("doc.org"))
            .unwrap()
            .contains("First heading, edited"),
        "the fixture must write the file back, else no hash is owed and this test proves nothing",
    );
    assert!(
        controller.owes_hash_stamps(),
        "the write-back owes its hash"
    );

    let stamped = controller.stamp_written_hashes().await;

    let calls = disclosures.0.lock().unwrap().clone();
    assert!(
        stamped.is_err(),
        "the stamp reported success although the backend recorded nothing; calls: {calls:?}",
    );
    assert!(
        calls.len() == 1 && calls[0].starts_with("unrecorded") && calls[0].contains("doc.org"),
        "the written file must be disclosed as unrecorded, naming it, and never as recorded: \
         {calls:?}",
    );
}
