//! An org vault of one document on a real directory, driven through the real
//! `FileSyncController`, with store doubles that apply every write.

#![allow(dead_code)] // each test file uses its own subset

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

pub const DOC_ID: &str = "20260908T090000";

pub type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

/// Child ids by parent, so a document's blocks are found without a walk over
/// every stored block of a many-file vault.
type Children = Arc<Mutex<HashMap<EntityUri, Vec<EntityUri>>>>;

/// Applies creates, updates AND deletes. The delete leg is the point: it is the
/// operation a 0-byte parse emits for every block of the document.
#[derive(Clone, Default)]
struct ApplyingOrdering {
    store: Store,
    children: Children,
}

fn params_id(params: &holon_api::StorageEntity) -> EntityUri {
    let id = params
        .get("id")
        .and_then(|v| v.as_string())
        .expect("every ingest op names the block it acts on");
    EntityUri::parse(id).expect("the ingest names blocks by uri")
}

#[async_trait]
impl BlockOrdering for ApplyingOrdering {
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
            .children
            .lock()
            .unwrap()
            .get(parent)
            .cloned()
            .unwrap_or_default())
    }
    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        _: Option<&EntityUri>,
        id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &HashMap<String, holon_api::Value>,
        edges: &holon_api::BlockEdges,
    ) -> OrderingResult<bool> {
        let mut block = Block::new_text(
            id.clone(),
            parent_id.clone(),
            content.as_text().unwrap_or(""),
        );
        block.properties = properties.clone();
        edges.apply_to(&mut block);
        if let Some(old) = self.store.lock().unwrap().insert(id.clone(), block) {
            self.unlink(&old);
        }
        self.children
            .lock()
            .unwrap()
            .entry(parent_id.clone())
            .or_default()
            .push(id.clone());
        Ok(false)
    }
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let id = params_id(&params);
        let mut store = self.store.lock().unwrap();
        if let Some(existing) = store.get_mut(&id) {
            if let Some(text) = params.get("content").and_then(|v| v.as_string()) {
                existing.content = text.to_string();
            }
        }
        Ok(())
    }
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let removed = self.store.lock().unwrap().remove(&params_id(&params));
        if let Some(old) = removed {
            self.unlink(&old);
        }
        Ok(())
    }
}

impl ApplyingOrdering {
    fn unlink(&self, block: &Block) {
        if let Some(siblings) = self.children.lock().unwrap().get_mut(&block.parent_id) {
            siblings.retain(|id| *id != block.id);
        }
    }
}

struct StoreReader(Store, Children);

#[async_trait]
impl BlockReader for StoreReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        let store = self.0.lock().unwrap();
        let children = self.1.lock().unwrap();
        let mut blocks = Vec::new();
        let mut open = vec![doc_id.clone()];
        while let Some(parent) = open.pop() {
            for id in children.get(&parent).into_iter().flatten() {
                blocks.push(store[id].clone());
                open.push(id.clone());
            }
        }
        Ok(blocks)
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
pub struct PageStore {
    pub by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

#[async_trait]
impl DocumentManager for PageStore {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        holon_filesystem::page_at_position(self.by_id.lock().unwrap().values(), parent_id, title)
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

/// A document with two identified headlines, so the blocks the ingest stores
/// keep stable ids across re-ingests and a disappearance is unambiguous.
pub fn full_content() -> String {
    format!(
        ":PROPERTIES:\n:ID: {DOC_ID}\n:END:\n#+TITLE: Notes\n\
         * Buy milk\n:PROPERTIES:\n:ID: 20260908T090001\n:END:\n\
         * Call the dentist\n:PROPERTIES:\n:ID: 20260908T090002\n:END:\n"
    )
}

/// Records the disclosure seam's calls, so a test asserts on the signal the
/// frontend's banner is built from rather than on a log line.
#[derive(Default)]
pub struct Disclosures(Mutex<Vec<String>>);

impl Disclosures {
    pub fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl holon_filesystem::WritebackDisclosure for Disclosures {
    fn writeback_degraded(&self, _: &str) {}
    fn writeback_stalled(&self, _: &std::path::Path, _: &str) {}
    fn writeback_resumed(&self, _: &std::path::Path) {}
    fn ingest_refused(&self, path: &std::path::Path, _: &str, _: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("refused {}", path.display()));
    }
    fn ingest_recovered(&self, path: &std::path::Path) {
        self.0
            .lock()
            .unwrap()
            .push(format!("recovered {}", path.display()));
    }
    fn vault_file_emptied(&self, path: &std::path::Path, _: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("emptied {}", path.display()));
    }
    fn file_condition_cleared(&self, path: &std::path::Path, kind: &'static str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("cleared {} {kind}", path.display()));
    }
    fn writeback_lossy(&self, _: &std::path::Path, _: &str) {}
    fn writeback_faithful(&self, _: &std::path::Path) {}

    fn block_in_two_files(
        &self,
        block_id: &holon_api::EntityUri,
        _: &holon_api::EntityUri,
        _: Option<&std::path::Path>,
        copy_files: &[&std::path::Path],
        _: bool,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(format!("two files {block_id} {copy_files:?}"));
    }

    fn deleted_block_kept_in_file(&self, _: &holon_api::EntityUri, _: &std::path::Path) {}

    fn deletion_undone(
        &self,
        _: &holon_api::EntityUri,
        _: &std::path::Path,
        _: &[&std::path::Path],
    ) {
    }

    fn undone_deletion_resolved(&self, _: &holon_api::EntityUri) {}
    fn deletion_ended_by_edit(&self, _: &holon_api::EntityUri, _: &std::path::Path) {}
    fn companion_page_id_overruled(
        &self,
        _: &str,
        _: &holon_api::EntityUri,
        _: &holon_api::EntityUri,
    ) {
    }
    fn file_edit_overruled(
        &self,
        _: &holon_api::EntityUri,
        _: &std::path::Path,
        _: &str,
        _: holon_api::HolonChange,
        _: &std::path::Path,
    ) {
    }
    fn holon_edit_overruled(
        &self,
        _: &holon_api::EntityUri,
        _: &std::path::Path,
        _: &str,
        _: &[String],
        _: &std::path::Path,
    ) {
    }
    fn vault_sync_not_started(&self, _: &std::path::Path, _: &str) {}
    fn vault_state_unreadable(&self, _: &std::path::Path, _: &std::path::Path, _: &str) {}
    fn vault_start_incomplete(&self, _: &std::path::Path, _: &str, _: &str) {}
    fn vault_backlog_ingesting(&self, _: &std::path::Path, _: usize, _: usize) {}
    fn vault_backlog_ingested(&self, _: &std::path::Path) {}
    fn written_files_unrecorded(&self, _: &std::path::Path, _: &[&std::path::Path], _: &str) {}
    fn written_files_recorded(&self, _: &std::path::Path) {}

    fn deleted_block_gone_from_file(&self, _: &holon_api::EntityUri) {}

    fn block_in_one_file_again(&self, block_id: &holon_api::EntityUri) {
        self.0.lock().unwrap().push(format!("one file {block_id}"));
    }
}

pub struct Vault {
    pub controller: holon_filesystem::FileSyncController,
    pub store: Store,
    pub docs: PageStore,
    pub path: std::path::PathBuf,
    pub disclosures: Arc<Disclosures>,
    pub net: holon_net_shell::Shell,
    _tmp: tempfile::TempDir,
}

impl Vault {
    /// The stored blocks of the document, by id — the doc-root excluded.
    pub fn block_ids(&self) -> Vec<String> {
        let doc = EntityUri::block(DOC_ID);
        let mut ids: Vec<String> = self
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.id != doc)
            .map(|b| b.id.as_str().to_string())
            .collect();
        ids.sort();
        ids
    }
}

pub fn vault() -> Vault {
    let uri = EntityUri::block(DOC_ID);
    let mut page = Block::new_text(uri.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(uri, page);

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, full_content()).unwrap();
    let (controller, store, disclosures, net) = controller_at(root, &docs);
    Vault {
        controller,
        store,
        docs,
        path,
        disclosures,
        net,
        _tmp: tmp,
    }
}

/// The real controller over `root`, with store doubles that apply every write.
pub fn controller_at(
    root: std::path::PathBuf,
    docs: &PageStore,
) -> (
    holon_filesystem::FileSyncController,
    Store,
    Arc<Disclosures>,
    holon_net_shell::Shell,
) {
    let ordering = ApplyingOrdering::default();
    let store = ordering.store.clone();
    let children = ordering.children.clone();
    let disclosures = Arc::new(Disclosures::default());
    let net = holon_net_shell::Shell::new(holon_filesystem::writeback_net::net())
        .expect("the write-back net loads");
    let controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone(), children)),
        Arc::new(docs.clone()),
        root,
        Arc::new(ordering),
        Arc::new(RealFileSystem),
    )
    .with_writeback_disclosure(disclosures.clone())
    .with_writeback_net(net.clone());
    (controller, store, disclosures, net)
}
