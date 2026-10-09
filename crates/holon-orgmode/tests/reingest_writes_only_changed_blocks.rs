//! A re-ingest writes only the blocks the file changed.
//!
//! Drives ONE real `FileSyncController` through two ingests of the same file,
//! so the second diffs against the first's projection exactly as a live
//! re-ingest does. Removing or inserting one headline moves every later
//! headline's position in the file, and position is not an edit: the blocks
//! whose text is untouched must reach the store with zero writes, while the
//! file's order still reaches it through `place_all`.
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

/// The store side of the SQL order owner: `place_all` sets a parent's total
/// sibling order, as `SqlBlockOperations::place_all` does, and `children`
/// answers from it.
#[derive(Clone, Default)]
struct FakeStore {
    blocks: Arc<Mutex<Vec<StorageEntity>>>,
    siblings: Arc<Mutex<HashMap<EntityUri, Vec<EntityUri>>>>,
    docs: Arc<Mutex<HashMap<EntityUri, Block>>>,
    writes: Arc<Mutex<Vec<String>>>,
}

impl FakeStore {
    fn take_writes(&self) -> Vec<String> {
        let mut writes: Vec<String> = std::mem::take(&mut *self.writes.lock().unwrap());
        writes.sort();
        writes
    }

    fn unlink(&self, id: &EntityUri) {
        for kids in self.siblings.lock().unwrap().values_mut() {
            kids.retain(|k| k != id);
        }
    }

    fn sibling_order(&self, parent_id: &EntityUri) -> Vec<String> {
        self.siblings.lock().unwrap()[parent_id]
            .iter()
            .map(|id| id.to_string())
            .collect()
    }

    fn parent_of(&self, id: &str) -> EntityUri {
        let blocks = self.blocks.lock().unwrap();
        let row = blocks
            .iter()
            .find(|row| row_field(row, "id") == id)
            .unwrap_or_else(|| panic!("no store row for {id}"));
        row_uri(row_field(row, "parent_id"))
    }
}

#[async_trait]
impl BlockOrdering for FakeStore {
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        self.unlink(uri);
        let mut siblings = self.siblings.lock().unwrap();
        let kids = siblings.entry(parent_id.clone()).or_default();
        let at = match after_id {
            None => 0,
            Some(after) => {
                kids.iter()
                    .position(|k| k == after)
                    .unwrap_or_else(|| panic!("place after {after}, absent under {parent_id}"))
                    + 1
            }
        };
        kids.insert(at, uri.clone());
        Ok(())
    }
    async fn place_all(
        &self,
        parent_id: &EntityUri,
        ordered_ids: &[EntityUri],
    ) -> OrderingResult<()> {
        let mut siblings = self.siblings.lock().unwrap();
        let kids = siblings.entry(parent_id.clone()).or_default();
        for id in ordered_ids {
            assert!(
                kids.contains(id),
                "place_all of {id}, not a child of {parent_id}"
            );
        }
        let rest: Vec<EntityUri> = kids
            .iter()
            .filter(|k| !ordered_ids.contains(k))
            .cloned()
            .collect();
        *kids = ordered_ids.iter().cloned().chain(rest).collect();
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
            .siblings
            .lock()
            .unwrap()
            .get(parent_id)
            .cloned()
            .unwrap_or_default())
    }
    async fn update_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        let uri = row_uri(&id);
        let parent_id = row_uri(row_field(&params, "parent_id"));
        self.writes.lock().unwrap().push(format!("upsert {id}"));
        let moved = {
            let mut blocks = self.blocks.lock().unwrap();
            match blocks.iter_mut().find(|row| row_field(row, "id") == id) {
                Some(row) => {
                    let moved = row_uri(row_field(row, "parent_id")) != parent_id;
                    *row = params;
                    moved
                }
                None => {
                    blocks.push(params);
                    true
                }
            }
        };
        if moved {
            self.unlink(&uri);
            self.siblings
                .lock()
                .unwrap()
                .entry(parent_id)
                .or_default()
                .push(uri);
        }
        Ok(())
    }
    async fn delete_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        self.writes.lock().unwrap().push(format!("delete {id}"));
        self.unlink(&row_uri(&id));
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
        holon_filesystem::page_at_position(self.docs.lock().unwrap().values(), parent_id, title)
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

const HEADER: &str = "#+ID: shift-doc\n#+TITLE: Shift\n";

fn document(bares: &[&str]) -> String {
    let mut text = String::from(HEADER);
    for bare in bares {
        text.push_str(&headline(bare));
    }
    text
}

/// One headline with the given stars, id `bare`, and no body.
fn stars(stars: &str, bare: &str) -> String {
    format!("{stars} {bare} text\n:PROPERTIES:\n:ID: {bare}\n:END:\n")
}

struct Vault {
    _tmp: tempfile::TempDir,
    path: std::path::PathBuf,
    store: FakeStore,
    controller: FileSyncController,
}

impl Vault {
    async fn ingested(bares: &[&str]) -> Self {
        Self::ingested_text(&document(bares)).await
    }

    async fn ingested_text(text: &str) -> Self {
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
        vault.reingest_text(text).await;
        vault
    }

    async fn reingest(&mut self, bares: &[&str]) -> Vec<String> {
        self.reingest_text(&document(bares)).await
    }

    async fn reingest_text(&mut self, text: &str) -> Vec<String> {
        std::fs::write(&self.path, text).unwrap();
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

#[tokio::test]
async fn reordering_siblings_writes_nothing_and_places_the_new_order() {
    let mut vault = Vault::ingested(&["a", "b", "c"]).await;
    let top = vault.store.parent_of("block:a");
    assert_eq!(
        vault.store.sibling_order(&top),
        vec!["block:a", "block:b", "block:c"]
    );
    let writes = vault.reingest(&["b", "a", "c"]).await;
    assert_eq!(
        writes,
        Vec::<String>::new(),
        "a pure reorder edits no block"
    );
    assert_eq!(
        vault.store.sibling_order(&top),
        vec!["block:b", "block:a", "block:c"],
        "the file's new order reaches the store through place_all"
    );
}

#[tokio::test]
async fn indenting_a_subtree_writes_only_the_block_whose_parent_changed() {
    let before = format!(
        "{HEADER}{}{}{}",
        stars("*", "a"),
        stars("*", "b"),
        stars("**", "b-child")
    );
    let after = format!(
        "{HEADER}{}{}{}",
        stars("*", "a"),
        stars("**", "b"),
        stars("***", "b-child")
    );
    let mut vault = Vault::ingested_text(&before).await;
    let writes = vault.reingest_text(&after).await;
    assert_eq!(
        writes,
        vec!["upsert block:b"],
        "b-child keeps its parent; its level is positional"
    );
}

#[tokio::test]
async fn changing_an_authored_star_jump_is_a_write() {
    let before = format!("{HEADER}{}{}", stars("*", "a"), stars("***", "a-child"));
    let after = format!("{HEADER}{}{}", stars("*", "a"), stars("****", "a-child"));
    let mut vault = Vault::ingested_text(&before).await;
    let writes = vault.reingest_text(&after).await;
    assert_eq!(
        writes,
        vec!["upsert block:a-child"],
        "a star count org cannot derive from the parent is file content"
    );
}
