//! A re-ingest writes only the blocks the file changed.
//!
//! Drives ONE real `FileSyncController` through two ingests of the same file,
//! so the second diffs against the first's projection exactly as a live
//! re-ingest does. Removing or inserting one headline moves every later
//! headline's position in the file, and position is not an edit: the blocks
//! whose text is untouched must reach the store with zero writes.
//!
//! @pbt kind harness
//! @pbt covers reingest-writes-only-changed-blocks — a re-ingest writes the
//!   changed blocks and nothing else

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

#[derive(Clone, Default)]
struct FakeStore {
    /// Insertion order is document order: `place` is a no-op here, so the
    /// write-back renders siblings in the order they were created.
    blocks: Arc<Mutex<Vec<StorageEntity>>>,
    docs: Arc<Mutex<HashMap<EntityUri, Block>>>,
    writes: Arc<Mutex<Vec<String>>>,
}

impl FakeStore {
    fn take_writes(&self) -> Vec<String> {
        let mut writes: Vec<String> = std::mem::take(&mut *self.writes.lock().unwrap());
        writes.sort();
        writes
    }
}

#[async_trait]
impl BlockOrdering for FakeStore {
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
            .blocks
            .lock()
            .unwrap()
            .iter()
            .map(row_to_block)
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id)
            .collect())
    }
    async fn update_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        self.writes.lock().unwrap().push(format!("upsert {id}"));
        let mut blocks = self.blocks.lock().unwrap();
        match blocks.iter_mut().find(|row| row_field(row, "id") == id) {
            Some(row) => *row = params,
            None => blocks.push(params),
        }
        Ok(())
    }
    async fn delete_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        self.writes.lock().unwrap().push(format!("delete {id}"));
        self.blocks
            .lock()
            .unwrap()
            .retain(|row| row_field(row, "id") != id);
        Ok(())
    }
}

#[async_trait]
impl DocumentManager for FakeStore {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .docs
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.docs
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .docs
            .lock()
            .unwrap()
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
    }
    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.docs
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

#[async_trait]
impl BlockReader for FakeStore {
    async fn get_blocks(&self, document_uri: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row_doc_uri(row) == *document_uri)
            .map(row_to_block)
            .filter(|b| b.id != *document_uri)
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
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|row| row_field(row, "id") == id.as_str())
            .map(row_to_block))
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

fn row_uri(raw: &str) -> EntityUri {
    EntityUri::parse(raw)
        .unwrap_or_else(|e| panic!("store row holds an unparseable uri {raw}: {e}"))
}

fn row_field<'a>(row: &'a StorageEntity, key: &str) -> &'a str {
    row.get(key)
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("store row {row:?} has no `{key}`"))
}

fn row_doc_uri(row: &StorageEntity) -> EntityUri {
    row_uri(row_field(row, holon_api::ROUTING_DOC_URI_KEY))
}

fn row_to_block(row: &StorageEntity) -> Block {
    Block::new_text(
        row_uri(row_field(row, "id")),
        row_uri(row_field(row, "parent_id")),
        row_field(row, "content"),
    )
}

fn headline(bare: &str) -> String {
    format!(
        "* {bare} text\n:PROPERTIES:\n:ID: {bare}\n:END:\n{bare} body\n** {bare} child\n:PROPERTIES:\n:ID: {bare}-child\n:END:\n"
    )
}

fn document(bares: &[&str]) -> String {
    let mut text = String::from("#+ID: shift-doc\n#+TITLE: Shift\n");
    for bare in bares {
        text.push_str(&headline(bare));
    }
    text
}

struct Vault {
    _tmp: tempfile::TempDir,
    path: std::path::PathBuf,
    store: FakeStore,
    controller: FileSyncController,
}

impl Vault {
    async fn ingested(bares: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let path = root.join("Shift.org");
        let store = FakeStore::default();
        let controller = new_org_sync_controller(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            root,
            Arc::new(store.clone()),
            Arc::new(RealFileSystem),
        );
        let mut vault = Self {
            _tmp: tmp,
            path,
            store,
            controller,
        };
        vault.reingest(bares).await;
        vault.store.take_writes();
        vault
    }

    async fn reingest(&mut self, bares: &[&str]) -> Vec<String> {
        std::fs::write(&self.path, document(bares)).unwrap();
        self.controller
            .on_file_changed(&self.path)
            .await
            .expect("ingest must succeed");
        self.store.take_writes()
    }
}

#[tokio::test]
async fn removing_the_first_headline_writes_only_its_deletes() {
    let mut vault = Vault::ingested(&["a", "b", "c"]).await;
    let writes = vault.reingest(&["b", "c"]).await;
    assert_eq!(
        writes,
        vec!["delete block:a", "delete block:a-child"],
        "b and c only moved up the file; position is not an edit"
    );
}

#[tokio::test]
async fn inserting_a_first_headline_writes_only_its_creates() {
    let mut vault = Vault::ingested(&["b", "c"]).await;
    let writes = vault.reingest(&["a", "b", "c"]).await;
    assert_eq!(
        writes,
        vec!["upsert block:a", "upsert block:a-child"],
        "b and c only moved down the file; position is not an edit"
    );
}

#[tokio::test]
async fn reingesting_identical_bytes_writes_nothing() {
    let mut vault = Vault::ingested(&["a", "b", "c"]).await;
    let writes = vault.reingest(&["a", "b", "c"]).await;
    assert_eq!(writes, Vec::<String>::new());
}
