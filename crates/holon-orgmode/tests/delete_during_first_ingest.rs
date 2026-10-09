//! A file the user deletes while its first ingest is still running stays
//! deleted.
//!
//! When the file is gone by the time `ingest_file`'s normalization write-back
//! runs, the ingest has already put the document's blocks in the store, so
//! the deletion must still cascade: the path has to be tracked, or the poll
//! backstop and the watcher's Remove have nothing to delete and a later page
//! upsert re-materializes the file.
//!
//! The FileSystem double deletes the file as the write-back hands it the
//! normalized bytes. The store doubles APPLY deletes.
//!
//! @pbt kind harness
//! @pbt covers delete-during-first-ingest — a vault file deleted while its
//! first ingest runs is cascaded and not recreated

#![cfg(feature = "di")]

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

const DOC_ID: &str = "20260908T090000";

type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

/// Applies creates, updates AND deletes. The delete leg is the point: it is the
/// operation a 0-byte parse emits for every block of the document.
#[derive(Clone, Default)]
struct ApplyingOrdering {
    store: Store,
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
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.parent_id == *parent)
            .map(|b| b.id.clone())
            .collect())
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
        self.store.lock().unwrap().insert(id.clone(), block);
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
        self.store.lock().unwrap().remove(&params_id(&params));
        Ok(())
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

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use holon_filesystem::FileMeta;
use holon_filesystem::FileStamp;
use holon_filesystem::FileSystem;
use holon_filesystem::ScannedEntries;
use holon_filesystem::StampedRead;
use holon_filesystem::WriteBack;

/// Needs an un-normalized file: a headline with no `:ID:` makes the
/// rendered projection differ from the disk bytes, which is what reaches the
/// write-back.
const CONTENT: &str = ":PROPERTIES:\n:ID: 20260908T090000\n:END:\n#+TITLE: Notes\n* Buy milk\n";

/// Deletes `target` when the first write-back of it starts.
struct DeletesBeforeWriteBack {
    inner: RealFileSystem,
    target: PathBuf,
    deleted: AtomicBool,
}

#[async_trait]
impl FileSystem for DeletesBeforeWriteBack {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.inner.read_to_string(path).await
    }
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.inner.read(path).await
    }
    async fn read_stamped(&self, path: &Path) -> std::io::Result<StampedRead> {
        self.inner.read_stamped(path).await
    }
    async fn write_if_unchanged(
        &self,
        path: &Path,
        expected: &FileStamp,
        contents: &[u8],
    ) -> std::io::Result<WriteBack> {
        if path == self.target && !self.deleted.swap(true, Ordering::SeqCst) {
            std::fs::remove_file(path)?;
        }
        self.inner
            .write_if_unchanged(path, expected, contents)
            .await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        self.inner.write(path, contents).await
    }
    async fn remove(&self, path: &Path) -> std::io::Result<()> {
        self.inner.remove(path).await
    }
    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.create_dir_all(path).await
    }
    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries> {
        self.inner.scan_directory(root).await
    }
    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        self.inner.metadata(path).await
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
}

#[tokio::test]
async fn a_file_deleted_during_its_first_ingest_is_cascaded_and_stays_deleted() {
    let uri = EntityUri::block(DOC_ID);
    let mut page = Block::new_text(uri.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(uri, page);
    let ordering = ApplyingOrdering::default();
    let store = ordering.store.clone();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, CONTENT).unwrap();

    let fs = Arc::new(DeletesBeforeWriteBack {
        inner: RealFileSystem,
        target: path.clone(),
        deleted: AtomicBool::new(false),
    });
    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root,
        Arc::new(ordering),
        fs.clone(),
    );

    let _ = controller
        .on_file_changed(&path)
        .await
        .expect("the ingest completes");
    assert!(
        fs.deleted.load(Ordering::SeqCst),
        "the double must have deleted the file at the ingest's write-back, else this test proves \
         nothing",
    );
    assert!(!path.exists(), "the double deleted the file");
    assert!(
        !store.lock().unwrap().is_empty(),
        "the fixture must land blocks before the delete cascades",
    );

    controller.poll_tracked_files().await.expect("poll runs");

    let left: Vec<String> = store
        .lock()
        .unwrap()
        .keys()
        .map(|k| k.as_str().to_string())
        .collect();
    assert!(
        left.is_empty(),
        "the file was deleted during its first ingest, yet its blocks stayed in the store: \
         {left:?}",
    );
    assert!(!path.exists(), "the deleted file was recreated");
}
