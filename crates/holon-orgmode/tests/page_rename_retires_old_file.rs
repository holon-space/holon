//! A renamed page must keep owning exactly ONE file — in EVERY storage mode.
//!
//! `dyn AliasRegistrar` is registered only inside the `loro_enabled` arm of the
//! composition root, so in the shipped SqlOnly default the controller holds
//! `None` and the rename cleanup in `materialize_page_identity_file` had no
//! record of the page's previous home: the new title's file was written, the
//! old title's file survived, and the page was DOUBLE-HOMED
//! (`inv-every-page-has-its-own-file`).
//!
//! The two tests drive the SAME rename through the two wirings, so the
//! invariant is pinned as mode-independent rather than as a property of the
//! Loro seam.

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
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

const OLD_TITLE: &str = "pagea";
const NEW_TITLE: &str = "Renamed";
const DIR_TITLE: &str = "structural-page";

// ---------------------------------------------------------------------------
// A mutable block store, so the page can be RETITLED under the same controller
// (a rename is one store write, not a new session).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Store {
    by_id: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<Block>>,
    /// Whatever production stamped through `persist_file_hash`. Replayed by
    /// `load_file_hashes` so a second controller boots the cold-boot fast path
    /// on the hash the first one actually wrote — no reimplementation of
    /// `projection_hash` in the test.
    file_hashes: Vec<(EntityUri, String)>,
}

#[derive(Clone, Default)]
struct Fixtures(Arc<Mutex<Store>>);

impl Fixtures {
    fn page_id() -> EntityUri {
        EntityUri::block("pgorigin")
    }

    fn seeded(title: &str) -> Self {
        let this = Self::default();
        this.retitle(title);
        this
    }

    /// (Re)build the whole fixture with the page carrying `title`. A rename in
    /// production rewrites the page block's content in place; here the block's
    /// id is stable and only its title moves.
    fn retitle(&self, title: &str) {
        let dir = page(DIR_TITLE, EntityUri::no_parent(), DIR_TITLE);
        let target = page("pgorigin", dir.id.clone(), title);
        let leaf = non_page("pgchild", target.id.clone(), "child");

        let mut store = self.0.lock().unwrap();
        store.by_id.clear();
        store.children.clear();
        for b in [&dir, &target, &leaf] {
            store.by_id.insert(b.id.clone(), b.clone());
        }
        store.children.insert(dir.id.clone(), vec![target.clone()]);
        store.children.insert(target.id.clone(), vec![leaf]);
    }

    fn block(&self, id: &EntityUri) -> Block {
        self.0.lock().unwrap().by_id[id].clone()
    }
}

#[async_trait]
impl BlockReader for Fixtures {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .children
            .get(doc_id)
            .cloned()
            .unwrap_or_default())
    }

    /// Delegates to `get_blocks`: this double has no cheaper projection. Never
    /// an empty stub — an empty shape would let the write-back
    /// fold-completeness gate PASS on an incomplete document.
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
        Ok(self.0.lock().unwrap().by_id.get(id).cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }

    async fn load_file_hashes(&self) -> anyhow::Result<Vec<(EntityUri, String)>> {
        Ok(self.0.lock().unwrap().file_hashes.clone())
    }

    async fn persist_file_hash(&self, uri: &EntityUri, hash: &str) -> anyhow::Result<()> {
        let mut store = self.0.lock().unwrap();
        store.file_hashes.retain(|(u, _)| u != uri);
        store.file_hashes.push((uri.clone(), hash.to_string()));
        Ok(())
    }
}

#[async_trait]
impl DocumentManager for Fixtures {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .by_id
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .by_id
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
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

#[derive(Default)]
struct MapAliasRegistrar(Mutex<HashMap<EntityUri, PathBuf>>);

#[async_trait]
impl holon_filesystem::sync_ports::AliasRegistrar for MapAliasRegistrar {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path) {
        self.0
            .lock()
            .unwrap()
            .insert(doc_id.clone(), path.to_path_buf());
    }
    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        self.0.lock().unwrap().get(doc_id).cloned()
    }
}

fn page(id: &str, parent: EntityUri, title: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent, title.to_string());
    b.set_page(true);
    b
}

fn non_page(id: &str, parent: EntityUri, content: &str) -> Block {
    Block::new_text(EntityUri::block(id), parent, content.to_string())
}

/// macOS hands `tempdir()` a `/var/…` path that resolves to `/private/var/…`;
/// the controller's containment proof compares the two spellings literally, so
/// the vault root and every path a test hands it must be the SAME spelling.
fn vault_root(tmp: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(tmp.path()).unwrap()
}

/// Drive one page upsert the way production does.
async fn write_back(
    controller: &mut holon_filesystem::FileSyncController,
    block: &Block,
) -> anyhow::Result<bool> {
    let doc = Fixtures::page_id();
    controller.seed_holder_from_authority(&doc).await?;
    controller
        .on_block_changed(
            &doc,
            &BlockDelta::Upsert {
                block: block.clone(),
                prev: None,
            },
        )
        .await
}

fn build_controller(
    f: &Fixtures,
    root: &Path,
    registrar: Option<Arc<MapAliasRegistrar>>,
) -> holon_filesystem::FileSyncController {
    let controller = new_org_sync_controller(
        Arc::new(f.clone()),
        Arc::new(f.clone()),
        root.to_path_buf(),
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    );
    match registrar {
        Some(r) => controller.with_alias_registrar(r),
        None => controller,
    }
}

/// `create page -> rename page` through ONE controller, in the wiring the
/// caller picks. Asserts the page ends up homed to exactly one file.
async fn a_rename_leaves_exactly_one_home(registrar: Option<Arc<MapAliasRegistrar>>) {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let mut controller = build_controller(&f, &root, registrar);

    let original = f.block(&Fixtures::page_id());
    write_back(&mut controller, &original)
        .await
        .expect("the page's first write-back must land");

    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));
    assert!(
        old_file.exists(),
        "precondition: the page must own {old_file:?} before the rename"
    );

    // The rename: the SAME page id, a new authoritative title. Production
    // drives it as `set_field("content")` on the page block, which reaches the
    // controller as an upsert of the retitled block.
    f.retitle(NEW_TITLE);
    let renamed = f.block(&Fixtures::page_id());
    write_back(&mut controller, &renamed)
        .await
        .expect("renaming a page must not crash the sync loop");

    let new_file = root.join(DIR_TITLE).join(format!("{NEW_TITLE}.org"));
    assert!(
        new_file.exists(),
        "the renamed page never got its new file {new_file:?}"
    );
    assert!(
        !old_file.exists(),
        "the page stayed DOUBLE-HOMED — the pre-rename file {old_file:?} survived the rename to \
         {new_file:?}"
    );
}

/// Loro wiring (`dyn AliasRegistrar` present) — the path that already worked.
#[tokio::test]
async fn a_renamed_page_retires_its_old_file_with_an_alias_registrar() {
    a_rename_leaves_exactly_one_home(Some(Arc::new(MapAliasRegistrar::default()))).await;
}

/// SqlOnly wiring (`dyn AliasRegistrar` ABSENT — the shipped default). The
/// controller must retire the old home from its OWN record of what it wrote.
#[tokio::test]
async fn a_renamed_page_retires_its_old_file_without_an_alias_registrar() {
    a_rename_leaves_exactly_one_home(None).await;
}

// ---------------------------------------------------------------------------
// Deletion safety. A home record says where the page USED to live — never who
// owns those bytes now. Between our write and the rename anything can have
// replaced the file, and the watcher's re-ingest need not have landed, so the
// retire must re-read the disk and prove ownership before removing anything.
// ---------------------------------------------------------------------------

/// The page's old home is externally replaced by `squatter` before the rename.
/// The retire must REFUSE and leave those bytes untouched — a stale double-home
/// is recoverable, a deleted user document is not.
async fn a_rename_refuses_to_delete(squatter: &str) {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let mut controller = build_controller(&f, &root, None);

    let original = f.block(&Fixtures::page_id());
    write_back(&mut controller, &original)
        .await
        .expect("the page's first write-back must land");

    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));
    assert!(old_file.exists(), "precondition: {old_file:?} must exist");

    // Someone else takes the path over, and the watcher has NOT delivered it.
    std::fs::write(&old_file, squatter).unwrap();

    f.retitle(NEW_TITLE);
    let renamed = f.block(&Fixtures::page_id());
    write_back(&mut controller, &renamed)
        .await
        .expect("a refused retire must not fail the write-back");

    let new_file = root.join(DIR_TITLE).join(format!("{NEW_TITLE}.org"));
    assert!(
        new_file.exists(),
        "the renamed page still needs its new file {new_file:?}"
    );
    assert!(
        old_file.exists(),
        "the rename DELETED bytes the page does not own at {old_file:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&old_file).unwrap(),
        squatter,
        "the squatting document's bytes were modified at {old_file:?}"
    );
}

/// Probe iii — the vacated path now carries a FOREIGN `#+ID:` plus user text
/// the watcher has not yet ingested.
#[tokio::test]
async fn a_rename_does_not_delete_a_file_that_now_roots_another_page() {
    a_rename_refuses_to_delete(
        "#+ID: 11111111-2222-3333-4444-555555555555\n* someone else's headline\nunsynced body\n",
    )
    .await;
}

/// Probe iv — two docs, one path: the second page's rendered document sits at
/// the path the first page is vacating. It must survive intact.
#[tokio::test]
async fn a_rename_does_not_delete_a_second_pages_document_at_the_same_path() {
    let other = page("pgother", EntityUri::block(DIR_TITLE), OLD_TITLE);
    a_rename_refuses_to_delete(&format!(
        "#+ID: {}\n#+TITLE: {OLD_TITLE}\n* other page body\n",
        other.id.id()
    ))
    .await;
}

/// The `None` arm: a hand-authored file with no `#+ID:` root at all, dropped at
/// the name the page is vacating.
#[tokio::test]
async fn a_rename_does_not_delete_a_file_with_no_id_header() {
    a_rename_refuses_to_delete("* a plain org file a user dropped here\n").await;
}

/// Cold boot of an UNCHANGED vault: `initialize` loads the hash the previous
/// session stamped, so `on_file_changed` takes the byte-identity fast path and
/// never runs the ingest that would normally record the page's home. A rename
/// later in that session must still retire the old file.
#[tokio::test]
async fn a_page_renamed_after_a_cold_boot_fast_path_still_retires_its_old_file() {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));

    // Session 1 writes the page's file.
    {
        let mut controller = build_controller(&f, &root, None);
        let original = f.block(&Fixtures::page_id());
        write_back(&mut controller, &original)
            .await
            .expect("the page's first write-back must land");
    }
    // A later session ingests it and stamps `file.content_hash` — the ingest
    // that engages the next boot's fast path. It has to be a FRESH controller:
    // the writing session echo-suppresses its own bytes and never reaches the
    // stamp.
    {
        let mut controller = build_controller(&f, &root, None);
        controller
            .on_file_changed(&old_file)
            .await
            .expect("ingesting the file must stamp its content hash");
    }
    assert!(
        !f.0.lock().unwrap().file_hashes.is_empty(),
        "precondition: session 1 must have stamped a content hash to boot from"
    );

    // Session 2 boots the same vault, unchanged.
    let mut controller = build_controller(&f, &root, None);
    controller.initialize().await.expect("cold boot");
    controller
        .on_file_changed(&old_file)
        .await
        .expect("the unchanged file must take the fast path, not error");

    f.retitle(NEW_TITLE);
    let renamed = f.block(&Fixtures::page_id());
    write_back(&mut controller, &renamed)
        .await
        .expect("renaming after a cold boot must not crash the sync loop");

    let new_file = root.join(DIR_TITLE).join(format!("{NEW_TITLE}.org"));
    assert!(
        new_file.exists(),
        "the renamed page never got its new file {new_file:?}"
    );
    assert!(
        !old_file.exists(),
        "a page renamed after a cold-boot fast path stayed DOUBLE-HOMED — {old_file:?} survived"
    );
}
