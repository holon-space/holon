//! An ingest that overrules a block Holon moved discloses a lost file edit
//! only when the file's version of the block differs from its base in what the
//! file can hold. On the first ingest of a file the base is read from the
//! store, whose blocks differ from a parse of the same block in fields the
//! file does not hold as the store does (an `_`-prefixed property key among
//! them); such a difference is no edit of the editor's.
//!
//! The store read and the tree's answer are taken apart here, as a Holon move
//! that lands between the ingest's base read and its classification leaves
//! them.
//!
//! @pbt kind harness
//! @pbt covers delete-races-file-edit — an overruled block untouched by the
//! editor is not disclosed as a lost edit when its base came from the store

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::consolidator::Seen;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

const DOC_ID: &str = "20261006T090000";
const ALPHA: &str = "20261006T090001";
const BETA: &str = "20261006T090002";

type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

/// The upstream tree, in which Beta has moved away from the document's root;
/// the store still holds it there.
#[derive(Clone)]
struct MovedBetaTree {
    store: Store,
}

#[async_trait]
impl BlockOrdering for MovedBetaTree {
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
    async fn children(&self, parent: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.parent_id == *parent && b.id != EntityUri::block(BETA))
            .map(|b| b.id.clone())
            .collect())
    }
    async fn ever_seen(&self, _: &EntityUri) -> OrderingResult<Seen> {
        Ok(Seen::Live)
    }
    fn has_upstream_consolidator(&self) -> bool {
        true
    }
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("every ingest op names the block it acts on");
        let id = EntityUri::parse(id).expect("the ingest names blocks by uri");
        if let Some(existing) = self.store.lock().unwrap().get_mut(&id) {
            if let Some(text) = params.get("content").and_then(|v| v.as_string()) {
                existing.content = text.to_string();
            }
        }
        Ok(())
    }
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        panic!("this ingest deletes nothing, got delete_in_tree({params:?})");
    }
}

struct StoreReader(Store);

#[async_trait]
impl BlockReader for StoreReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.id != *doc_id)
            .cloned()
            .collect())
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
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Default)]
struct PageStore {
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

#[async_trait]
impl DocumentManager for PageStore {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .lock()
            .unwrap()
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
    }
    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

/// Records the blocks disclosed as an overruled file edit.
#[derive(Default)]
struct Overruled(Mutex<Vec<String>>);

impl holon_filesystem::WritebackDisclosure for Overruled {
    fn writeback_degraded(&self, _: &str) {}
    fn writeback_stalled(&self, _: &std::path::Path, _: &str) {}
    fn writeback_resumed(&self, _: &std::path::Path) {}
    fn ingest_refused(&self, _: &std::path::Path, _: &str, _: &str) {}
    fn ingest_recovered(&self, _: &std::path::Path) {}
    fn vault_file_emptied(&self, _: &std::path::Path) {}
    fn writeback_lossy(&self, _: &std::path::Path, _: &str) {}
    fn writeback_faithful(&self, _: &std::path::Path) {}
    fn block_in_two_files(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&std::path::Path>,
        _: &[&std::path::Path],
        _: bool,
    ) {
    }
    fn block_in_one_file_again(&self, _: &EntityUri) {}
    fn deleted_block_kept_in_file(&self, _: &EntityUri, _: &std::path::Path) {}
    fn deleted_block_gone_from_file(&self, _: &EntityUri) {}
    fn deletion_undone(&self, _: &EntityUri, _: &std::path::Path, _: &[&std::path::Path]) {}
    fn undone_deletion_resolved(&self, _: &EntityUri) {}
    fn deletion_ended_by_edit(&self, _: &EntityUri, _: &std::path::Path) {}
    fn file_edit_overruled(
        &self,
        block_id: &EntityUri,
        _: &std::path::Path,
        file_text: &str,
        change: holon_api::HolonChange,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{block_id} {change}: {file_text:?}"));
    }
    fn vault_sync_not_started(&self, _: &std::path::Path, _: &str) {}
    fn vault_state_unreadable(&self, _: &std::path::Path, _: &std::path::Path, _: &str) {}
    fn vault_start_incomplete(&self, _: &std::path::Path, _: &str, _: &str) {}
    fn written_files_unrecorded(&self, _: &std::path::Path, _: &[&std::path::Path], _: &str) {}
    fn written_files_recorded(&self, _: &std::path::Path) {}
}

#[tokio::test]
async fn a_store_only_field_is_no_overruled_edit() {
    let doc = EntityUri::block(DOC_ID);
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);

    let store: Store = Arc::default();
    let alpha = Block::new_text(EntityUri::block(ALPHA), doc.clone(), "Alpha");
    let mut beta = Block::new_text(EntityUri::block(BETA), doc.clone(), "Beta");
    beta.properties.insert(
        "_held_by_the_store".to_string(),
        holon_api::Value::String("not in the file".to_string()),
    );
    for block in [alpha, beta] {
        store.lock().unwrap().insert(block.id.clone(), block);
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(
        &path,
        format!(
            ":PROPERTIES:\n:ID: {DOC_ID}\n:END:\n#+TITLE: Notes\n\
             * Alpha edited outside\n:PROPERTIES:\n:ID: {ALPHA}\n:END:\n\
             * Beta\n:PROPERTIES:\n:ID: {BETA}\n:END:\n"
        ),
    )
    .unwrap();

    let overruled = Arc::new(Overruled::default());
    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root,
        Arc::new(MovedBetaTree { store }),
        Arc::new(RealFileSystem),
    )
    .with_writeback_disclosure(overruled.clone());
    let outcome = controller.on_file_changed(&path).await;

    assert_eq!(
        overruled.0.lock().unwrap().clone(),
        Vec::<String>::new(),
        "Beta, untouched by the editor, was disclosed as an overruled file edit: a field \
         only the store holds read as the editor's change (ingest outcome {outcome:?})"
    );
}
