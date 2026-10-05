//! A user edit or delete that lands while Holon writes a vault file back
//! survives: every write-back compares the file with what it read, and on a
//! mismatch drops the write and re-ingests the file instead.
//!
//! The FileSystem double edits or deletes the target as the controller hands
//! it the bytes to write — after the read the write is based on, before the
//! replacement. One test per write-back path.
//!
//! @pbt kind harness
//! @pbt covers writeback-compare-and-rename — a write-back never overwrites or
//! recreates a file the user changed after Holon read it

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockChangeVerdict;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::RealFileSystem;
use holon_filesystem::RefusedWritebacks;
use holon_filesystem::WritebackDisclosure;
use holon_filesystem::fs_port::FileMeta;
use holon_filesystem::fs_port::FileStamp;
use holon_filesystem::fs_port::FileSystem;
use holon_filesystem::fs_port::ScannedEntries;
use holon_filesystem::fs_port::StampedRead;
use holon_filesystem::fs_port::WriteBack;
use holon_orgmode::file_sync_controller::new_org_sync_controller;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

const DOC_ID: &str = "20261005T120000";

/// Un-normalized: the headline has no `:ID:`, so the first ingest writes the
/// file back.
const CONTENT: &str = ":PROPERTIES:\n:ID: 20261005T120000\n:END:\n#+TITLE: Notes\n* Buy milk\n";

type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

fn params_id(params: &holon_api::StorageEntity) -> EntityUri {
    let id = params
        .get("id")
        .and_then(|v| v.as_string())
        .expect("every ingest op names the block it acts on");
    EntityUri::parse(id).expect("the ingest names blocks by uri")
}

/// The block store: pages and their blocks in one map. Applies creates,
/// updates and deletes, so a re-ingest and a cascade are observable.
#[derive(Clone, Default)]
struct World {
    store: Store,
}

impl World {
    fn with_page(title: &str) -> Self {
        let world = Self::default();
        let mut page = Block::new_text(EntityUri::block(DOC_ID), EntityUri::no_parent(), title);
        page.set_page(true);
        world.put(page);
        world
    }

    fn put(&self, block: Block) {
        self.store.lock().unwrap().insert(block.id.clone(), block);
    }

    fn page(&self) -> Block {
        self.store.lock().unwrap()[&EntityUri::block(DOC_ID)].clone()
    }

    fn block_with_text(&self, text: &str) -> Option<Block> {
        self.store
            .lock()
            .unwrap()
            .values()
            .find(|b| !b.is_page() && b.content == text)
            .cloned()
    }

    fn texts(&self) -> Vec<String> {
        self.store
            .lock()
            .unwrap()
            .values()
            .filter(|b| !b.is_page())
            .map(|b| b.content.clone())
            .collect()
    }

    fn descendants(&self, root: &EntityUri) -> Vec<Block> {
        let store = self.store.lock().unwrap();
        let mut out = Vec::new();
        let mut frontier = vec![root.clone()];
        while let Some(parent) = frontier.pop() {
            for block in store.values().filter(|b| b.parent_id == parent) {
                frontier.push(block.id.clone());
                out.push(block.clone());
            }
        }
        out
    }
}

#[async_trait]
impl BlockOrdering for World {
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
        self.put(block);
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

#[async_trait]
impl BlockReader for World {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self.descendants(doc_id))
    }
    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .descendants(doc_id)
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.store.lock().unwrap().get(id).cloned())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        let pages: Vec<EntityUri> = self
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.is_page())
            .map(|b| b.id.clone())
            .collect();
        Ok(pages
            .into_iter()
            .map(|id| {
                let blocks = self.descendants(&id);
                (id, blocks)
            })
            .collect())
    }
}

#[async_trait]
impl DocumentManager for World {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.put(doc.clone());
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
    }
    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.put(doc.clone());
        Ok(())
    }
}

/// What the user does to the target while Holon writes it back.
enum Interference {
    Edit(String),
    Delete,
    /// Atomically replace the file with its own bytes: a new stamp, the same
    /// content.
    RewriteSame,
    /// [`Interference::RewriteSame`] before every write, never disarmed.
    KeepRewritingSame,
}

fn rewrite_same(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let temp = path.with_extension("org.rewrite");
    std::fs::write(&temp, bytes).unwrap();
    std::fs::rename(&temp, path).unwrap();
}

/// Applies the armed interference to `target` when the controller hands it
/// bytes for that path, then lets the write proceed.
struct InterferesBeforeWrite {
    inner: RealFileSystem,
    target: PathBuf,
    armed: Mutex<Option<Interference>>,
    fired: AtomicBool,
}

impl InterferesBeforeWrite {
    fn new(target: PathBuf) -> Self {
        Self {
            inner: RealFileSystem,
            target,
            armed: Mutex::new(None),
            fired: AtomicBool::new(false),
        }
    }

    fn arm(&self, interference: Interference) {
        *self.armed.lock().unwrap() = Some(interference);
    }

    fn disarm(&self) {
        *self.armed.lock().unwrap() = None;
    }

    fn interfere(&self, path: &Path) {
        if path != self.target {
            return;
        }
        let mut armed = self.armed.lock().unwrap();
        match armed.take() {
            Some(Interference::Edit(bytes)) => std::fs::write(path, bytes).unwrap(),
            Some(Interference::Delete) => std::fs::remove_file(path).unwrap(),
            Some(Interference::RewriteSame) => rewrite_same(path),
            Some(Interference::KeepRewritingSame) => {
                rewrite_same(path);
                *armed = Some(Interference::KeepRewritingSame);
            }
            None => return,
        }
        self.fired.store(true, Ordering::SeqCst);
    }

    fn assert_fired(&self) {
        assert!(
            self.fired.load(Ordering::SeqCst),
            "the controller never wrote {} back, so this test proves nothing",
            self.target.display(),
        );
    }
}

#[async_trait]
impl FileSystem for InterferesBeforeWrite {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.inner.read_to_string(path).await
    }
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.inner.read(path).await
    }
    async fn read_stamped(&self, path: &Path) -> std::io::Result<StampedRead> {
        self.inner.read_stamped(path).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        self.interfere(path);
        self.inner.write(path, contents).await
    }
    async fn write_if_unchanged(
        &self,
        path: &Path,
        expected: &FileStamp,
        contents: &[u8],
    ) -> std::io::Result<WriteBack> {
        self.interfere(path);
        self.inner
            .write_if_unchanged(path, expected, contents)
            .await
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

/// Records the write-back and ingest conditions as `<signal> <path>`.
#[derive(Default)]
struct Disclosures(Mutex<Vec<String>>);

impl Disclosures {
    fn record(&self, signal: &str, path: &Path) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{signal} {}", path.display()));
    }

    fn of(&self, signal: &str, path: &Path) -> usize {
        let wanted = format!("{signal} {}", path.display());
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|s| **s == wanted)
            .count()
    }
}

impl WritebackDisclosure for Disclosures {
    fn writeback_degraded(&self, _: &str) {}
    fn writeback_stalled(&self, path: &Path, _: &str) {
        self.record("stalled", path);
    }
    fn writeback_resumed(&self, path: &Path) {
        self.record("resumed", path);
    }
    fn ingest_refused(&self, path: &Path, _: &str, _: &str) {
        self.record("ingest_refused", path);
    }
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
    fn vault_sync_not_started(&self, _: &Path, _: &str) {}
    fn vault_state_unreadable(&self, _: &Path, _: &Path, _: &str) {}
    fn vault_start_incomplete(&self, _: &Path, _: &str, _: &str) {}
    fn written_files_unrecorded(&self, _: &Path, _: &[&Path], _: &str) {}
    fn written_files_recorded(&self, _: &Path) {}
    fn deleted_block_gone_from_file(&self, _: &EntityUri) {}
    fn block_in_one_file_again(&self, _: &EntityUri) {}
}

/// Every WARN event's fields, rendered as `name=value` pairs.
#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<String>>>);

struct FieldsVisitor<'a>(&'a mut String);

impl Visit for FieldsVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        write!(self.0, "{}={:?} ", field.name(), value).unwrap();
    }
}

impl<S: tracing::Subscriber> Layer<S> for Warnings {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::WARN {
            let mut fields = String::new();
            event.record(&mut FieldsVisitor(&mut fields));
            self.0.lock().unwrap().push(fields);
        }
    }
}

impl Warnings {
    fn naming(&self, path: &Path) -> usize {
        let path = path.display().to_string();
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.contains(&path))
            .count()
    }
}

struct Harness {
    _tmp: tempfile::TempDir,
    path: PathBuf,
    world: World,
    fs: Arc<InterferesBeforeWrite>,
    disclosed: Arc<Disclosures>,
    refused: Arc<RefusedWritebacks>,
    controller: FileSyncController,
}

impl Harness {
    fn new(world: World) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let path = root.join("Notes.org");
        let fs = Arc::new(InterferesBeforeWrite::new(path.clone()));
        let disclosed = Arc::new(Disclosures::default());
        let refused = Arc::new(RefusedWritebacks::default());
        let controller = new_org_sync_controller(
            Arc::new(world.clone()),
            Arc::new(world.clone()),
            root,
            Arc::new(world.clone()),
            fs.clone(),
        )
        .with_writeback_disclosure(disclosed.clone())
        .with_refused_writebacks(refused.clone());
        Self {
            _tmp: tmp,
            path,
            world,
            fs,
            disclosed,
            refused,
            controller,
        }
    }

    /// A tracked `Notes.org` whose first ingest and normalization write-back
    /// already ran.
    async fn ingested() -> Self {
        let mut h = Self::new(World::with_page("Notes"));
        std::fs::write(&h.path, CONTENT).unwrap();
        let _ = h
            .controller
            .on_file_changed(&h.path)
            .await
            .expect("the first ingest completes");
        assert!(
            h.world.block_with_text("Buy milk").is_some(),
            "the fixture ingests its block"
        );
        h
    }

    fn disk(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    /// The store edit a block-driven write-back renders.
    fn edit_store(&self) -> Block {
        let mut block = self
            .world
            .block_with_text("Buy milk")
            .expect("the fixture block");
        block.content = "Buy oat milk".to_string();
        self.world.put(block.clone());
        block
    }

    async fn block_changed(&mut self, block: &Block) {
        let doc = EntityUri::block(DOC_ID);
        self.controller
            .seed_holder_from_authority(&doc)
            .await
            .expect("seed the holder");
        self.controller
            .on_block_changed(
                &doc,
                &BlockDelta::Upsert {
                    block: block.clone(),
                    prev: None,
                },
            )
            .await
            .expect("the block-driven write-back runs");
    }

    /// One poll tick: the tracked-file backstop, then new-file discovery.
    async fn poll(&mut self) {
        self.controller
            .poll_tracked_files()
            .await
            .expect("the poll backstop runs");
        self.controller
            .poll_new_files()
            .await
            .expect("the new-file discovery runs");
    }

    fn sorted_texts(&self) -> Vec<String> {
        let mut texts = self.world.texts();
        texts.sort();
        texts
    }
}

#[tokio::test]
async fn ingest_normalization_keeps_an_edit_made_during_the_write() {
    let mut h = Harness::new(World::with_page("Notes"));
    std::fs::write(&h.path, CONTENT).unwrap();
    let edit = format!("{CONTENT}* Call mom\n");
    h.fs.arm(Interference::Edit(edit.clone()));

    let _ = h
        .controller
        .on_file_changed(&h.path)
        .await
        .expect("the ingest completes");
    h.fs.assert_fired();
    assert_eq!(
        h.disk(),
        edit,
        "the normalization write-back overwrote the user's edit"
    );

    h.poll().await;
    assert!(
        h.world.block_with_text("Call mom").is_some(),
        "the edit was never re-ingested: {:?}",
        h.world.texts(),
    );
}

#[tokio::test]
async fn ingest_normalization_keeps_a_delete_made_during_the_write() {
    let mut h = Harness::new(World::with_page("Notes"));
    std::fs::write(&h.path, CONTENT).unwrap();
    h.fs.arm(Interference::Delete);

    let _ = h
        .controller
        .on_file_changed(&h.path)
        .await
        .expect("the ingest completes");
    h.fs.assert_fired();
    assert!(
        !h.path.exists(),
        "the normalization write-back recreated the deleted file"
    );

    h.poll().await;
    assert!(
        h.world.block_with_text("Buy milk").is_none(),
        "the delete never cascaded: {:?}",
        h.world.texts(),
    );
}

#[tokio::test]
async fn block_write_back_keeps_an_edit_made_during_the_write() {
    let mut h = Harness::ingested().await;
    let edit = format!("{}* Call mom\n", h.disk());
    h.fs.arm(Interference::Edit(edit.clone()));

    let block = h.edit_store();
    h.block_changed(&block).await;
    h.fs.assert_fired();
    assert_eq!(
        h.disk(),
        edit,
        "the block write-back overwrote the user's edit"
    );

    h.poll().await;
    assert!(
        h.world.block_with_text("Call mom").is_some(),
        "the edit was never re-ingested: {:?}",
        h.world.texts(),
    );
}

#[tokio::test]
async fn block_write_back_keeps_a_delete_made_during_the_write() {
    let mut h = Harness::ingested().await;
    h.fs.arm(Interference::Delete);

    let block = h.edit_store();
    h.block_changed(&block).await;
    h.fs.assert_fired();
    assert!(
        !h.path.exists(),
        "the block write-back recreated the deleted file"
    );

    h.poll().await;
    assert!(
        h.world.block_with_text("Buy oat milk").is_none(),
        "the delete never cascaded: {:?}",
        h.world.texts(),
    );
}

#[tokio::test]
async fn re_render_keeps_an_edit_made_during_the_write() {
    let mut h = Harness::ingested().await;
    let edit = format!("{}* Call mom\n", h.disk());
    h.fs.arm(Interference::Edit(edit.clone()));

    h.edit_store();
    h.controller
        .re_render_all_tracked(&HashSet::new())
        .await
        .expect("the re-render runs");
    h.fs.assert_fired();
    assert_eq!(h.disk(), edit, "the re-render overwrote the user's edit");

    h.poll().await;
    assert!(
        h.world.block_with_text("Call mom").is_some(),
        "the edit was never re-ingested: {:?}",
        h.world.texts(),
    );
}

const USER_FILE: &str = "#+TITLE: Notes\n* Written by the user\n";

#[tokio::test]
async fn fileless_page_sweep_keeps_a_file_the_user_created_during_the_write() {
    let world = World::with_page("Notes");
    let mut child = Block::new_text(
        EntityUri::block("child-1"),
        EntityUri::block(DOC_ID),
        "Buy milk",
    );
    child.set_page(false);
    world.put(child);
    let mut h = Harness::new(world);
    h.fs.arm(Interference::Edit(USER_FILE.to_string()));

    h.controller
        .materialize_missing_page_files()
        .await
        .expect("the sweep runs");
    h.fs.assert_fired();
    assert_eq!(
        h.disk(),
        USER_FILE,
        "the fileless-page sweep overwrote the file the user created"
    );
}

#[tokio::test]
async fn page_identity_file_keeps_a_file_the_user_created_during_the_write() {
    let mut h = Harness::new(World::with_page("Notes"));
    h.fs.arm(Interference::Edit(USER_FILE.to_string()));

    let page = h.world.page();
    h.block_changed(&page).await;
    h.fs.assert_fired();
    assert!(
        h.disk().contains("Written by the user"),
        "the page identity file overwrote the file the user created: {:?}",
        h.disk(),
    );
}

#[tokio::test]
async fn ingest_normalization_lands_over_a_rewrite_of_the_same_bytes() {
    let mut h = Harness::new(World::with_page("Notes"));
    std::fs::write(&h.path, CONTENT).unwrap();
    h.fs.arm(Interference::RewriteSame);

    let _ = h
        .controller
        .on_file_changed(&h.path)
        .await
        .expect("the ingest completes");
    h.fs.assert_fired();
    assert_ne!(
        h.disk(),
        CONTENT,
        "the normalization write-back was dropped although the file kept the bytes it was \
         rendered from"
    );
}

#[tokio::test]
async fn block_write_back_lands_over_a_rewrite_of_the_same_bytes() {
    let mut h = Harness::ingested().await;
    h.fs.arm(Interference::RewriteSame);

    let block = h.edit_store();
    h.block_changed(&block).await;
    h.fs.assert_fired();
    assert!(
        h.disk().contains("Buy oat milk"),
        "the block write-back was dropped although the file kept the bytes it was rendered \
         from: {:?}",
        h.disk(),
    );

    h.poll().await;
    assert!(
        h.disk().contains("Buy oat milk"),
        "the poll reverted the write-back: {:?}",
        h.disk(),
    );
}

#[tokio::test]
async fn block_write_back_discloses_a_file_that_keeps_rewriting_its_own_bytes() {
    let mut h = Harness::ingested().await;
    let before = h.disk();
    h.fs.arm(Interference::KeepRewritingSame);

    let block = h.edit_store();
    let doc = EntityUri::block(DOC_ID);
    h.controller
        .seed_holder_from_authority(&doc)
        .await
        .expect("seed the holder");
    let err = h
        .controller
        .on_block_changed(&doc, &BlockDelta::Upsert { block, prev: None })
        .await
        .expect_err("a write-back that can never land is an error, not a silent drop");
    h.fs.assert_fired();
    assert!(
        format!("{err:#}").contains("kept changing"),
        "the error does not name the condition: {err:#}"
    );
    assert_eq!(h.disk(), before, "the file is not touched");
}

/// A stalled write-back is disclosed for its file and waits: the poll does not
/// retry it, the next block edit writes it.
#[tokio::test]
async fn ingest_normalization_discloses_a_file_that_keeps_rewriting_its_own_bytes() {
    let warnings = Warnings::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(warnings.clone()));
    let mut h = Harness::new(World::with_page("Notes"));
    std::fs::write(&h.path, CONTENT).unwrap();
    h.fs.arm(Interference::KeepRewritingSame);

    let _ =
        h.controller.on_file_changed(&h.path).await.expect(
            "the store holds the whole file, so a stalled write-back is not an ingest failure",
        );
    h.fs.assert_fired();
    assert_eq!(h.disk(), CONTENT, "the file is not touched");
    assert!(
        h.world.block_with_text("Buy milk").is_some(),
        "the ingest landed in the store"
    );
    assert_eq!(
        h.disclosed.of("ingest_refused", &h.path),
        0,
        "a stalled write-back is reported as a refused ingest"
    );
    assert_eq!(
        h.disclosed.of("stalled", &h.path),
        1,
        "the stalled write-back raises no write-back condition"
    );
    assert_eq!(
        h.refused.documents().len(),
        1,
        "the shutdown guard does not learn of the stalled write-back: {:?}",
        h.refused.documents()
    );
    assert!(
        warnings.naming(&h.path) > 0,
        "no WARN names the stalled file: {:?}",
        warnings.0.lock().unwrap()
    );

    h.fs.disarm();
    h.poll().await;
    h.poll().await;
    assert_eq!(
        h.disk(),
        CONTENT,
        "the poll retried the stalled write-back; only the next event writes it"
    );
    assert_eq!(
        h.disclosed.of("resumed", &h.path),
        0,
        "the condition was lifted although nothing was written"
    );
    assert_eq!(h.refused.documents().len(), 1);

    let block = h.edit_store();
    h.block_changed(&block).await;
    assert!(
        h.disk().contains("Buy oat milk"),
        "the stall quarantined the file, so the next block edit never reached it: {:?}",
        h.disk()
    );
    assert_eq!(h.disclosed.of("resumed", &h.path), 1);
    assert!(h.refused.documents().is_empty());
}

/// `CONTENT` plus a second id-less headline.
const TWO: &str =
    ":PROPERTIES:\n:ID: 20261005T120000\n:END:\n#+TITLE: Notes\n* Buy milk\n* Call mom\n";

/// A first ingest of `content` whose normalization write-back stalls on a file
/// that keeps rewriting its own bytes. Returns the id the ingest gave
/// "Buy milk".
async fn stalled_ingest_of(h: &mut Harness, content: &str) -> EntityUri {
    std::fs::write(&h.path, content).unwrap();
    h.fs.arm(Interference::KeepRewritingSame);
    let _ = h
        .controller
        .on_file_changed(&h.path)
        .await
        .expect("a stalled normalization is not an ingest failure");
    h.fs.assert_fired();
    assert_eq!(
        h.disk(),
        content,
        "the stalled normalization touched the file"
    );
    h.world
        .block_with_text("Buy milk")
        .expect("the ingest landed in the store")
        .id
}

async fn stalled_ingest(h: &mut Harness) -> EntityUri {
    stalled_ingest_of(h, CONTENT).await
}

/// A first ingest of `content` whose normalization write-back lands.
async fn ingest_of(h: &mut Harness, content: &str) -> EntityUri {
    std::fs::write(&h.path, content).unwrap();
    let _ = h
        .controller
        .on_file_changed(&h.path)
        .await
        .expect("the first ingest completes");
    assert_ne!(h.disk(), content, "the normalization did not write");
    h.world
        .block_with_text("Buy milk")
        .expect("the ingest landed in the store")
        .id
}

/// The poll neither retries the stalled write nor re-ingests the unchanged
/// file over the store edit.
#[tokio::test]
async fn a_store_edit_made_during_an_ingest_stall_survives_and_reaches_disk() {
    let mut h = Harness::new(World::with_page("Notes"));
    let id = stalled_ingest(&mut h).await;
    h.edit_store();

    h.poll().await;
    h.fs.disarm();
    h.poll().await;
    h.poll().await;
    assert_eq!(h.disk(), CONTENT, "a poll wrote the unchanged file");
    assert_eq!(
        h.world.block_with_text("Buy oat milk").map(|b| b.id),
        Some(id.clone()),
        "a poll re-ingested the unchanged file over the store edit: {:?}",
        h.world.texts(),
    );

    let block = h.world.block_with_text("Buy oat milk").unwrap();
    h.block_changed(&block).await;
    assert!(
        h.disk().contains("Buy oat milk"),
        "the next block edit never reached disk: {:?}",
        h.disk()
    );
    assert_eq!(
        h.world.block_with_text("Buy oat milk").map(|b| b.id),
        Some(id)
    );
    assert_eq!(h.disclosed.of("resumed", &h.path), 1);
    assert!(h.refused.documents().is_empty());
}

#[tokio::test]
async fn an_edit_on_disk_during_an_ingest_stall_ingests_over_the_parsed_base() {
    let mut h = Harness::new(World::with_page("Notes"));
    let id = stalled_ingest(&mut h).await;
    h.edit_store();
    let edit = format!("{CONTENT}* Call mom\n");
    std::fs::write(&h.path, &edit).unwrap();

    h.poll().await;
    assert!(
        h.world.block_with_text("Call mom").is_some(),
        "the edit on disk was not ingested: {:?}",
        h.world.texts(),
    );
    assert_eq!(
        h.world.block_with_text("Buy oat milk").map(|b| b.id),
        Some(id.clone()),
        "the re-ingest replaced the block the store edited: {:?}",
        h.world.texts(),
    );

    h.fs.disarm();
    let block = h.world.block_with_text("Buy oat milk").unwrap();
    h.block_changed(&block).await;
    let disk = h.disk();
    assert!(
        disk.contains("Buy oat milk") && disk.contains("Call mom"),
        "the next block edit never reached disk: {disk:?}"
    );
    assert!(h.refused.documents().is_empty());
}

/// Without a stall, a line the user deletes on disk leaves the store.
#[tokio::test]
async fn a_line_deleted_on_disk_after_the_first_ingest_stays_deleted() {
    let mut h = Harness::new(World::with_page("Notes"));
    ingest_of(&mut h, TWO).await;
    let disk = h.disk();
    // The render may put either headline first.
    let start = disk
        .find("* Call mom")
        .expect("the normalized file holds Call mom");
    let end = disk[start + 1..]
        .find("\n* ")
        .map_or(disk.len(), |i| start + 2 + i);
    std::fs::write(&h.path, format!("{}{}", &disk[..start], &disk[end..])).unwrap();

    h.poll().await;
    assert_eq!(h.sorted_texts(), ["Buy milk"]);
    let block = h.world.block_with_text("Buy milk").unwrap();
    h.block_changed(&block).await;
    assert!(!h.disk().contains("Call mom"), "{:?}", h.disk());
}

#[tokio::test]
#[ignore = "D97 state machine Inc 2: the stalled ingest leaves an id-less diff base, so the re-ingest cannot bind the deleted line to its block"]
async fn a_line_deleted_on_disk_during_an_ingest_stall_stays_deleted() {
    let mut h = Harness::new(World::with_page("Notes"));
    stalled_ingest_of(&mut h, TWO).await;
    std::fs::write(&h.path, CONTENT).unwrap();

    h.poll().await;
    h.fs.disarm();
    h.poll().await;
    assert_eq!(
        h.sorted_texts(),
        ["Buy milk"],
        "the line the user deleted is still in the store"
    );
    assert!(!h.disk().contains("Call mom"), "{:?}", h.disk());

    let block = h.world.block_with_text("Buy milk").unwrap();
    h.block_changed(&block).await;
    assert!(
        !h.disk().contains("Call mom"),
        "the next write-back put the deleted line back: {:?}",
        h.disk()
    );
}

/// Without a stall, one line edited in the store and on disk stays one block
/// under its id.
#[tokio::test]
async fn a_line_edited_in_the_store_and_on_disk_after_the_first_ingest_stays_one_block() {
    let mut h = Harness::new(World::with_page("Notes"));
    let id = ingest_of(&mut h, CONTENT).await;
    h.edit_store();
    std::fs::write(&h.path, h.disk().replace("Buy milk", "Buy soy milk")).unwrap();

    h.poll().await;
    assert_eq!(h.sorted_texts(), ["Buy soy milk"]);
    assert_eq!(
        h.world.block_with_text("Buy soy milk").map(|b| b.id),
        Some(id)
    );
}

#[tokio::test]
#[ignore = "D97 state machine Inc 2: the stalled ingest leaves an id-less diff base, so the re-ingest mints a new block for the edited line"]
async fn a_line_edited_in_the_store_and_on_disk_during_an_ingest_stall_stays_one_block() {
    let mut h = Harness::new(World::with_page("Notes"));
    let id = stalled_ingest(&mut h).await;
    h.edit_store();
    std::fs::write(&h.path, CONTENT.replace("Buy milk", "Buy soy milk")).unwrap();

    h.poll().await;
    h.fs.disarm();
    h.poll().await;
    assert_eq!(
        h.sorted_texts(),
        ["Buy soy milk"],
        "the line edited in two places split into two blocks"
    );
    assert_eq!(
        h.world.block_with_text("Buy soy milk").map(|b| b.id),
        Some(id),
        "the edited line lost its id"
    );

    let block = h.world.block_with_text("Buy soy milk").unwrap();
    h.block_changed(&block).await;
    assert_eq!(h.disk().matches("\n* ").count(), 1, "{:?}", h.disk());
}

/// A block-driven write-back of a store edit whose pre-ingest's normalization
/// stalls. Returns the edited block.
async fn pre_ingest_stall(h: &mut Harness) -> Block {
    let block = h.edit_store();
    let doc = EntityUri::block(DOC_ID);
    h.controller
        .seed_holder_from_authority(&doc)
        .await
        .expect("seed the holder");
    // A file without its document id makes the pre-ingest stamp it, i.e. write.
    let without_doc_id = h
        .disk()
        .strip_prefix(":PROPERTIES:\n:ID: 20261005T120000\n:END:\n")
        .expect("the normalized file opens with its document id")
        .to_string();
    std::fs::write(&h.path, without_doc_id).unwrap();
    h.fs.arm(Interference::KeepRewritingSame);

    let verdicts = h
        .controller
        .on_block_changed_coalesced(&[(
            doc,
            BlockDelta::Upsert {
                block: block.clone(),
                prev: None,
            },
        )])
        .await;
    let (_, verdict) = verdicts.into_iter().next().expect("one document");
    assert!(verdict.is_err(), "the stalled write-back was not reported");
    h.fs.assert_fired();
    assert_eq!(h.refused.documents().len(), 1);
    h.fs.disarm();
    h.poll().await;
    h.poll().await;
    block
}

/// The stall stays disclosed until a write carries the edit to disk.
#[tokio::test]
async fn a_block_edit_whose_pre_ingest_stalls_stays_disclosed_until_it_reaches_disk() {
    let mut h = Harness::ingested().await;
    let block = pre_ingest_stall(&mut h).await;
    assert!(
        h.disk().contains("Buy oat milk") || h.refused.documents().len() == 1,
        "the stall was retracted although the edit is not on disk: {:?}",
        h.disk()
    );

    h.block_changed(&block).await;
    assert!(h.disk().contains("Buy oat milk"), "{:?}", h.disk());
    assert!(h.refused.documents().is_empty());
}

#[tokio::test]
#[ignore = "D97 state machine, no increment yet: the pre-ingest takes the file's text over the store edit, also without a stall and on main b2a19917"]
async fn a_block_edit_whose_pre_ingest_stalls_stays_in_the_store() {
    let mut h = Harness::ingested().await;
    pre_ingest_stall(&mut h).await;
    assert_eq!(
        h.sorted_texts(),
        ["Buy oat milk"],
        "the store lost the edit while its write-back stalled"
    );
}

#[tokio::test]
async fn a_file_deleted_during_an_ingest_stall_leaves_no_refused_write_back() {
    let mut h = Harness::new(World::with_page("Notes"));
    stalled_ingest(&mut h).await;
    std::fs::remove_file(&h.path).unwrap();

    h.poll().await;
    h.poll().await;
    assert!(!h.path.exists(), "a poll recreated the deleted file");
    assert!(
        h.world.texts().is_empty(),
        "the deletion did not reach the store: {:?}",
        h.world.texts()
    );
    assert!(
        h.refused.documents().is_empty(),
        "the shutdown guard still reports the deleted file: {:?}",
        h.refused.documents()
    );
    assert_eq!(h.disclosed.of("resumed", &h.path), 1);
}

#[tokio::test]
async fn a_block_edit_after_an_ingest_stall_reaches_disk() {
    let mut h = Harness::new(World::with_page("Notes"));
    let id = stalled_ingest(&mut h).await;
    h.fs.disarm();

    let block = h.edit_store();
    h.block_changed(&block).await;
    assert!(
        h.disk().contains("Buy oat milk"),
        "the block edit after the stall never reached disk: {:?}",
        h.disk()
    );
    assert_eq!(
        h.world.block_with_text("Buy oat milk").map(|b| b.id),
        Some(id),
        "the block edit re-ingested the stale file: {:?}",
        h.world.texts(),
    );
    assert!(h.refused.documents().is_empty());
}

/// The holder carries an edit the store render does not, so the block-driven
/// render differs from what the pre-ingest's normalization wrote.
#[tokio::test]
async fn a_block_edit_lands_after_a_pre_ingest_that_normalizes_the_file() {
    let mut h = Harness::ingested().await;
    let doc = EntityUri::block(DOC_ID);
    h.controller
        .seed_holder_from_authority(&doc)
        .await
        .expect("seed the holder");
    let mut block = h
        .world
        .block_with_text("Buy milk")
        .expect("the fixture block");
    block.content = "Buy oat milk".to_string();
    // A file without its document id makes the pre-ingest stamp it, i.e. write.
    let without_doc_id = h
        .disk()
        .strip_prefix(":PROPERTIES:\n:ID: 20261005T120000\n:END:\n")
        .expect("the normalized file opens with its document id")
        .to_string();
    std::fs::write(&h.path, without_doc_id).unwrap();

    let verdicts = h
        .controller
        .on_block_changed_coalesced(&[(doc, BlockDelta::Upsert { block, prev: None })])
        .await;
    let (_, verdict) = verdicts.into_iter().next().expect("one document");
    assert_eq!(
        verdict.expect("the block-driven write-back runs"),
        BlockChangeVerdict::Handled,
        "the write-back was based on the bytes from before the pre-ingest wrote the file"
    );
    assert!(
        h.disk().contains("Buy oat milk"),
        "the render was dropped: {:?}",
        h.disk()
    );
}

const OTHER_ID: &str = "20261005T130000";
const OTHER: &str = ":PROPERTIES:\n:ID: 20261005T130000\n:END:\n#+TITLE: Other\n* Water plants\n";

/// `re_render_all_tracked` walks its tracked files in hash order, so each
/// round can put the churning file before or after the other one.
#[tokio::test]
async fn re_render_writes_the_other_files_while_one_file_keeps_rewriting_its_own_bytes() {
    for round in 0..12 {
        let world = World::with_page("Notes");
        let mut other_page =
            Block::new_text(EntityUri::block(OTHER_ID), EntityUri::no_parent(), "Other");
        other_page.set_page(true);
        world.put(other_page);
        let mut h = Harness::new(world);
        let other_path = h.path.with_file_name("Other.org");
        std::fs::write(&h.path, CONTENT).unwrap();
        std::fs::write(&other_path, OTHER).unwrap();
        for path in [h.path.clone(), other_path.clone()] {
            let _ = h
                .controller
                .on_file_changed(&path)
                .await
                .expect("the first ingest completes");
        }
        let notes_before = h.disk();
        h.fs.arm(Interference::KeepRewritingSame);

        h.edit_store();
        let mut plants = h
            .world
            .block_with_text("Water plants")
            .expect("the other file's block");
        plants.content = "Water plants NOW".to_string();
        h.world.put(plants);
        h.controller
            .re_render_all_tracked(&HashSet::new())
            .await
            .expect("one file that cannot be written does not fail the pass");
        h.fs.assert_fired();

        let other = std::fs::read_to_string(&other_path).unwrap();
        assert!(
            other.contains("Water plants NOW"),
            "round {round}: the churning file stopped the pass before the other file: {other:?}"
        );
        assert_eq!(
            h.disk(),
            notes_before,
            "round {round}: the churning file is not touched"
        );
        assert_eq!(
            h.disclosed.of("stalled", &h.path),
            1,
            "round {round}: the churning file's stalled write-back is not disclosed"
        );
        assert_eq!(
            h.refused.documents().len(),
            1,
            "round {round}: {:?}",
            h.refused.documents()
        );
    }
}
