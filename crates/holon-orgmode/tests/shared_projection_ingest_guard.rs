//! Inc 3 MUST-FIX regression — the shared-subtree ingest guard must key on
//! AUTHORITATIVE mount state, never on parsed drawer content.
//!
//! `:share-role: mount:` / `:shared-tree-id:` drawer properties are lifted
//! verbatim from ANY user file by the org parser, so a guard that skips ingest
//! on content alone would silently drop a hand-authored / imported / templated
//! file carrying such a drawer — a page that never loads, edits that vanish.
//!
//! The observable is STORE STATE, not collaborator traffic: after
//! `on_file_changed` the file's blocks are either in the store (ingested) or
//! absent (skipped). Counting `DocumentManager` calls instead makes the tests
//! hostage to every unrelated pre-ingest step that happens to touch the same
//! collaborator.
//!
//! These tests drive the real ingest path (via `on_file_changed`) against a
//! store fake and assert:
//!   * content-looks-like-mount but NOT registered  → INGESTED (no false skip);
//!   * content-looks-like-mount AND registered       → SKIPPED  (guard works);
//!   * no registry seam at all                        → INGESTED (safe
//!     default);
//!   * a registered mount triggers neither ingest NOR the title-less doc-root
//!     heal — for a mount, Loro is truth, so healing FROM the file path is the
//!     direction invariant 11 forbids.

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering as AtomicOrdering;

use async_trait::async_trait;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::FileFormatAdapter;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::MountRegistry;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_format::OrgFormatAdapter;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

const MOUNT_DOC_ID: &str = "mount-xyz";
const MOUNT_CHILD_ID: &str = "child-1";

/// The store the controller writes through. `update_in_tree` /
/// `delete_in_tree` is the single org→block write seam, so the rows applied
/// there are the store's block state; `DocumentManager` holds the doc-root
/// rows. Both live behind one handle so a test reads ONE store.
#[derive(Clone, Default)]
struct FakeStore {
    blocks: Arc<Mutex<HashMap<String, StorageEntity>>>,
    docs: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

impl FakeStore {
    /// Seed an already-broken doc-root the title-less heal WOULD repair: a
    /// `Page` with EMPTY content, orphaned under the root sentinel.
    fn with_empty_orphan_page(id: &str) -> Self {
        let uri = EntityUri::block(id);
        let mut broken = Block::new_text(uri.clone(), EntityUri::no_parent(), String::new());
        broken.set_page(true);
        let store = Self::default();
        store.docs.lock().unwrap().insert(uri, broken);
        store
    }

    fn block_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.blocks.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    fn has_block(&self, id: &str) -> bool {
        self.blocks
            .lock()
            .unwrap()
            .contains_key(&EntityUri::block(id).to_string())
    }

    fn doc_content(&self, id: &str) -> Option<String> {
        self.docs
            .lock()
            .unwrap()
            .get(&EntityUri::block(id))
            .map(|d| d.content.clone())
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
            .values()
            .map(row_to_block)
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id)
            .collect())
    }
    async fn update_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("every store write carries an `id`")
            .to_string();
        self.blocks.lock().unwrap().insert(id, params);
        Ok(())
    }
    async fn delete_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("every store delete carries an `id`")
            .to_string();
        self.blocks.lock().unwrap().remove(&id);
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
    /// Page-only, mirroring `LiveDocumentManager`'s `WHERE tag='Page'` matview.
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

/// Reads serve what the write seam stored, so a successful ingest actually
/// converges here — the post-ingest doc walk finds the blocks it just wrote and
/// `on_file_changed` returns `Ok`. Without that, every outcome assertion would
/// be masked by a stub-induced ingest error.
#[async_trait]
impl BlockReader for FakeStore {
    async fn get_blocks(&self, document_uri: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .values()
            .filter(|row| row_doc_uri(row) == *document_uri)
            .map(row_to_block)
            .filter(|b| b.id != *document_uri)
            .collect())
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .get(&id.to_string())
            .map(row_to_block))
    }
    /// No junction to resolve against — this double stores marks as given.
    async fn resolve_link_marks(&self, _: &mut [Block]) -> anyhow::Result<()> {
        Ok(())
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

/// Stub registry with a fixed answer — models "this id IS / IS NOT a real
/// mount". Counts consultations: the guard is this seam's ONLY caller, so the
/// count is exactly "how many times the guard decided".
struct StubMountRegistry {
    registered: bool,
    consultations: AtomicUsize,
}

impl StubMountRegistry {
    fn new(registered: bool) -> Arc<Self> {
        Arc::new(Self {
            registered,
            consultations: AtomicUsize::new(0),
        })
    }

    fn consultations(&self) -> usize {
        self.consultations.load(AtomicOrdering::SeqCst)
    }
}

#[async_trait]
impl MountRegistry for StubMountRegistry {
    async fn is_registered_mount(&self, _: &EntityUri) -> anyhow::Result<bool> {
        self.consultations.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(self.registered)
    }
}

/// Render a mount-page file exactly as the write-back would, so its content
/// carries the share markers and is guaranteed parseable.
fn mount_page_org(path: &std::path::Path) -> String {
    let doc_uri = EntityUri::block(MOUNT_DOC_ID);
    let mut mount = Block::new_text(doc_uri.clone(), EntityUri::no_parent(), "My Shared Page");
    mount.set_page(true);
    mount.set_property("share-role", "mount");
    mount.set_property("shared-tree-id", "stid-abc");
    mount.set_property("ID", MOUNT_DOC_ID);
    let mut child = Block::new_text(
        EntityUri::block(MOUNT_CHILD_ID),
        doc_uri.clone(),
        "Child under P",
    );
    child.set_property("shared-tree-id", "stid-abc");
    child.set_property("ID", MOUNT_CHILD_ID);
    OrgFormatAdapter::new().render_document(&mount, &[child], path, &doc_uri)
}

/// A temp vault holding exactly one mount-shaped org file, with a controller
/// wired to `store`. The `TempDir` is held so the vault outlives the test body.
struct MountVault {
    _tmp: tempfile::TempDir,
    path: std::path::PathBuf,
    controller: FileSyncController,
    store: FakeStore,
}

fn mount_vault(store: FakeStore, registry: Option<Arc<StubMountRegistry>>) -> MountVault {
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize so the controller's `strip_prefix(root)` matches (macOS
    // /var/folders is a symlink to /private/var/folders).
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("My Shared Page.org");
    std::fs::write(&path, mount_page_org(&path)).unwrap();

    let mut controller = new_org_sync_controller(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        root,
        Arc::new(store.clone()),
        Arc::new(RealFileSystem),
    );
    if let Some(reg) = registry {
        controller = controller.with_mount_registry(reg);
    }
    MountVault {
        _tmp: tmp,
        path,
        controller,
        store,
    }
}

/// Drive one `on_file_changed` over a mount-shaped file and hand back the
/// store so the test can read the OUTCOME. The result is returned too — a
/// skip must be a clean `Ok`.
async fn ingest_mount_file(
    store: FakeStore,
    registry: Option<Arc<StubMountRegistry>>,
) -> (FakeStore, anyhow::Result<()>) {
    let mut vault = mount_vault(store, registry);
    let result = vault.controller.on_file_changed(&vault.path).await;
    (vault.store.clone(), result)
}

fn registry(registered: bool) -> Option<Arc<StubMountRegistry>> {
    Some(StubMountRegistry::new(registered))
}

// MUST-FIX: content looks like a mount but the id is NOT a registered mount →
// the file is INGESTED (the guard must not skip on drawer content alone).
#[tokio::test]
async fn unregistered_share_role_file_is_ingested_not_skipped() {
    let (store, result) = ingest_mount_file(FakeStore::default(), registry(false)).await;
    result.expect("ingesting an unregistered `share-role` file must succeed");
    assert!(
        store.has_block(MOUNT_CHILD_ID),
        "a `share-role` file whose id is NOT a registered mount must be ingested into the store \
         (no false skip) — store blocks = {:?}",
        store.block_ids()
    );
}

// The guard still works for a REAL mount: registered → SKIPPED (nothing about
// the file reaches the store).
#[tokio::test]
async fn registered_mount_file_is_skipped() {
    let (store, result) = ingest_mount_file(FakeStore::default(), registry(true)).await;
    result.expect("skipping a registered mount file is a clean no-op, not an error");
    assert!(
        store.block_ids().is_empty(),
        "a file whose id IS a registered mount must be skipped (projection sink) — the store \
         must hold none of its blocks, but holds {:?}",
        store.block_ids()
    );
    assert_eq!(
        store.doc_content(MOUNT_DOC_ID),
        None,
        "the skipped mount file must not create its doc-root either"
    );
}

// Safe default: with no registry seam wired, never skip on content alone.
#[tokio::test]
async fn no_registry_seam_ingests() {
    let (store, result) = ingest_mount_file(FakeStore::default(), None).await;
    result.expect("ingesting a `share-role` file with no registry wired must succeed");
    assert!(
        store.has_block(MOUNT_CHILD_ID),
        "without a mount registry the guard must never skip a share-role file — store blocks = \
         {:?}",
        store.block_ids()
    );
}

// Model.md invariant 11 covers the WHOLE file-change path, not just ingest: a
// registered mount's truth is the shared Loro doc, so the title-less doc-root
// heal — which re-derives content from the FILE PATH — must not run on it
// either. Seeded with exactly the store row that heal would rewrite.
#[tokio::test]
async fn registered_mount_file_is_not_healed() {
    let store = FakeStore::with_empty_orphan_page(MOUNT_DOC_ID);
    let (store, result) = ingest_mount_file(store, registry(true)).await;
    result.expect("skipping a registered mount file is a clean no-op, not an error");
    assert!(
        store.block_ids().is_empty(),
        "a registered mount file must trigger NEITHER the heal NOR ingest — the heal re-derives \
         the doc-root's content from the FILE NAME, i.e. from the projection sink, exactly the \
         direction invariant 11 forbids. Store rows written: {:?}",
        store.block_ids()
    );
    assert_eq!(
        store.doc_content(MOUNT_DOC_ID).as_deref(),
        Some(""),
        "the seeded mount doc-root must keep its store content untouched"
    );
}

// The BOOT route to the same heal. `heal_title_less_doc_roots` (the
// store-health sweep the boot driver runs unconditionally) reaches the heal
// without going through `on_file_changed`, so guarding only the file-watch path
// leaves a registered mount's doc-root rewritten from its file name on every
// boot.
#[tokio::test]
async fn registered_mount_file_is_not_healed_by_boot_sweep() {
    let store = FakeStore::with_empty_orphan_page(MOUNT_DOC_ID);
    let mut vault = mount_vault(store, registry(true));
    vault
        .controller
        .heal_title_less_doc_roots()
        .await
        .expect("the store-health sweep must not error on a registered mount file");
    assert!(
        vault.store.block_ids().is_empty(),
        "the boot store-health sweep must skip a registered mount file for the SAME reason the \
         file-watch path does — its truth is the shared Loro doc, not its file name. Store rows \
         written: {:?}",
        vault.store.block_ids()
    );
    assert_eq!(
        vault.store.doc_content(MOUNT_DOC_ID).as_deref(),
        Some(""),
        "the seeded mount doc-root must survive the boot sweep untouched"
    );
}

// The guard decides on genuine disk changes only. A write-back echo — the file
// watcher re-reporting bytes we ourselves just projected — must short-circuit
// BEFORE the guard: re-deciding it re-discloses the skip on every echo and
// re-parses content that changed by definition not at all.
#[tokio::test]
async fn write_back_echo_does_not_re_run_the_guard() {
    let reg = StubMountRegistry::new(true);
    let mut vault = mount_vault(FakeStore::default(), Some(reg.clone()));

    vault
        .controller
        .on_file_changed(&vault.path)
        .await
        .expect("skipping a registered mount file is a clean no-op");
    assert_eq!(
        reg.consultations(),
        1,
        "the first change of a mount file must consult the registry (and disclose the skip)"
    );

    // Byte-identical re-notification: our own projection output coming back.
    vault
        .controller
        .on_file_changed(&vault.path)
        .await
        .expect("an echo is a clean no-op");
    assert_eq!(
        reg.consultations(),
        1,
        "a byte-identical write-back echo must be short-circuited before the guard — deciding it \
         again re-discloses the skip on every echo of our own output"
    );

    // A genuine external edit is NOT an echo — the guard must decide again.
    let edited = format!("{}\n", std::fs::read_to_string(&vault.path).unwrap());
    std::fs::write(&vault.path, edited).unwrap();
    vault
        .controller
        .on_file_changed(&vault.path)
        .await
        .expect("skipping a registered mount file is a clean no-op");
    assert_eq!(
        reg.consultations(),
        2,
        "a genuine external edit of a mount file must re-run the guard"
    );
}
