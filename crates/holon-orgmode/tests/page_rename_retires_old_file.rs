//! A renamed page must keep owning exactly ONE file — in EVERY storage mode.
//!
//! `dyn AliasRegistrar` is registered only inside the `loro_enabled` arm of the
//! composition root, so in SqlOnly the controller holds
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

    /// Add a SECOND live page under the same directory page, carrying `title`
    /// and owning NO file — a fileless owner of that name chain. Its one child
    /// block stands in for the user content only the store holds.
    fn add_fileless_page(&self, id: &str, title: &str) -> (EntityUri, EntityUri) {
        let dir_id = EntityUri::block(DIR_TITLE);
        let page = page(id, dir_id.clone(), title);
        let child = non_page(
            &format!("{id}child"),
            page.id.clone(),
            "a line only the store holds",
        );

        let mut store = self.0.lock().unwrap();
        store.by_id.insert(page.id.clone(), page.clone());
        store.by_id.insert(child.id.clone(), child.clone());
        store.children.entry(dir_id).or_default().push(page.clone());
        store.children.insert(page.id.clone(), vec![child.clone()]);
        (page.id, child.id)
    }

    /// Rewrite the seeded page's one child block, so a later write-back
    /// carries content the file does not have yet.
    fn set_child_text(&self, text: &str) -> Block {
        let target = EntityUri::block("pgorigin");
        let child = non_page("pgchild", target.clone(), text);

        let mut store = self.0.lock().unwrap();
        store.by_id.insert(child.id.clone(), child.clone());
        store.children.insert(target, vec![child.clone()]);
        child
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

/// Records every `delete_in_tree` so a test can assert which blocks a cascade
/// removed — the store-side twin of asserting which files survived on disk.
#[derive(Default)]
struct RecordingOrdering {
    deleted: Mutex<Vec<String>>,
}

impl RecordingOrdering {
    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }
}

#[async_trait]
impl BlockOrdering for RecordingOrdering {
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
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("delete_in_tree is always called with an `id`")
            .to_string();
        self.deleted.lock().unwrap().push(id);
        Ok(())
    }
}

/// Collects the ERROR-level tracing output of one test, so a refusal can
/// be asserted on the surface an operator actually sees.
#[derive(Clone, Default)]
struct CapturedErrors(Arc<Mutex<Vec<u8>>>);

impl CapturedErrors {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedErrors {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedErrors {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Every `.org` file under `root`, so a test can prove a document's
/// content reached NO file rather than only the one it checked.
fn all_org_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "org") {
                out.push(path);
            }
        }
    }
    out
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

/// Drive one upsert for a doc the caller names, so a test can write
/// back a doc other than the seeded page.
async fn write_back_doc(
    controller: &mut holon_filesystem::FileSyncController,
    doc: &EntityUri,
    block: &Block,
) -> anyhow::Result<bool> {
    controller.seed_holder_from_authority(doc).await?;
    controller
        .on_block_changed(
            doc,
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
    build_controller_with_ordering(f, root, registrar, Arc::new(RecordingOrdering::default()))
}

fn build_controller_with_ordering(
    f: &Fixtures,
    root: &Path,
    registrar: Option<Arc<MapAliasRegistrar>>,
    ordering: Arc<RecordingOrdering>,
) -> holon_filesystem::FileSyncController {
    let controller = new_org_sync_controller(
        Arc::new(f.clone()),
        Arc::new(f.clone()),
        root.to_path_buf(),
        ordering,
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

/// SqlOnly wiring (`dyn AliasRegistrar` ABSENT). The
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

// ---------------------------------------------------------------------------
// Cascade safety. The retire above deletes a file, so the watcher delivers a
// delete event for a path Holon itself vacated. Nothing about that event says
// which document — if any — the vanished bytes belonged to, and resolving it by
// the path's NAME finds whatever page answers to that name today.
// ---------------------------------------------------------------------------

/// The retired home's own delete event must not cascade-delete a DIFFERENT live
/// page that merely answers to the vacated name.
///
/// After the retire, `forget_file_state` has dropped the path's
/// `last_projection`, so `on_file_deleted` cannot read the vanished file's
/// `#+ID:` and falls back to a name-chain lookup on the now-vacated chain. A
/// FILELESS page carrying the old title (a page the store holds but no file
/// backs — a rule-minted page, a `convert_block_to_page` result, a page whose
/// materialize has not run) answers that lookup, and the id-based reunification
/// scan cannot save it: it owns no tracked file for the scan to find.
#[tokio::test]
async fn retiring_a_stale_home_does_not_cascade_delete_a_fileless_namesake_page() {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let ordering = Arc::new(RecordingOrdering::default());
    let mut controller = build_controller_with_ordering(&f, &root, None, ordering.clone());

    let original = f.block(&Fixtures::page_id());
    write_back(&mut controller, &original)
        .await
        .expect("the page's first write-back must land");

    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));
    assert!(old_file.exists(), "precondition: {old_file:?} must exist");

    // The rename frees the old title, and an unrelated live page holds it.
    f.retitle(NEW_TITLE);
    let (namesake, namesake_child) = f.add_fileless_page("pgnamesake", OLD_TITLE);
    let renamed = f.block(&Fixtures::page_id());
    write_back(&mut controller, &renamed)
        .await
        .expect("renaming a page must not crash the sync loop");
    assert!(
        !old_file.exists(),
        "precondition: the retire must have removed {old_file:?}"
    );

    // The watcher delivers the delete event for the file Holon itself removed.
    controller
        .on_file_changed(&old_file)
        .await
        .expect("the retire's own delete event must not fail the sync loop");

    assert_eq!(
        ordering.deleted(),
        Vec::<String>::new(),
        "the retire's own delete event cascade-deleted the FILELESS page {namesake} (child \
         {namesake_child}) — a live document that never lived at {old_file:?}"
    );
}

/// The discriminating control: a file the user genuinely removes still
/// cascades. The proof gate has to refuse the NAME GUESS, not every deletion.
#[tokio::test]
async fn a_users_deletion_of_a_tracked_file_still_cascades() {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let ordering = Arc::new(RecordingOrdering::default());
    let mut controller = build_controller_with_ordering(&f, &root, None, ordering.clone());

    let original = f.block(&Fixtures::page_id());
    write_back(&mut controller, &original)
        .await
        .expect("the page's first write-back must land");

    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));
    std::fs::remove_file(&old_file).expect("the user removes the page's file");
    controller
        .on_file_changed(&old_file)
        .await
        .expect("an external deletion must not fail the sync loop");

    assert!(
        ordering
            .deleted()
            .contains(&Fixtures::page_id().to_string()),
        "the user's deletion of {old_file:?} did NOT cascade — deleted: {:?}",
        ordering.deleted()
    );
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

/// The cold-boot fast path must record the doc's home in the ALIAS
/// registry, not only in the controller's own map. Path derivation reads
/// the alias first and otherwise falls back to a name chain, and a name
/// chain is not unique: two docs sharing one can both answer it, so the
/// loser's write-back is aimed at the winner's file.
///
/// See `docs/Testing/bugfunnel/entries/
/// 2026-08-30-cold-boot-alias-gap-misroutes-writeback.md`.
#[tokio::test]
async fn a_cold_boot_fast_path_still_registers_the_docs_alias() {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));

    {
        let mut controller = build_controller(&f, &root, None);
        let original = f.block(&Fixtures::page_id());
        write_back(&mut controller, &original)
            .await
            .expect("the page's first write-back must land");
    }
    {
        let mut controller = build_controller(&f, &root, None);
        controller
            .on_file_changed(&old_file)
            .await
            .expect("ingesting the file must stamp its content hash");
    }

    // A fresh process holds an EMPTY alias registry: the map lives in
    // memory, so only this boot's own ingests can fill it.
    let registrar = Arc::new(MapAliasRegistrar::default());
    let mut controller = build_controller(&f, &root, Some(registrar.clone()));
    controller.initialize().await.expect("cold boot");
    controller
        .on_file_changed(&old_file)
        .await
        .expect("the unchanged file must take the fast path, not error");

    use holon_filesystem::sync_ports::AliasRegistrar;
    assert_eq!(
        registrar.resolve_alias_to_path(&Fixtures::page_id()).await,
        Some(old_file.clone()),
        "the cold-boot fast path left {} with NO alias, so its write-back path is derived \
         from a name chain any namesake doc also answers",
        Fixtures::page_id()
    );
}

/// The harm the ambiguity does. A namesake doc answers the owner's
/// name chain, so its write-back is aimed at a file it does not own;
/// the ADR-0025 removal guard then quarantines that file, and the
/// owner's own edits stop reaching disk. Derivation must refuse a
/// path another doc is known to home, so the owner keeps writing.
///
/// Driven through BOTH wirings: the refusal reads `doc_home`, which
/// exists in every storage mode, so SqlOnly must be protected
/// too and not only the Loro arm.
///
/// See `docs/Testing/bugfunnel/entries/
/// 2026-08-30-cold-boot-alias-gap-misroutes-writeback.md`.
async fn a_namesake_never_takes_the_owners_file(registrar: Option<Arc<MapAliasRegistrar>>) {
    let f = Fixtures::seeded(OLD_TITLE);
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let old_file = root.join(DIR_TITLE).join(format!("{OLD_TITLE}.org"));

    {
        let mut controller = build_controller(&f, &root, None);
        let original = f.block(&Fixtures::page_id());
        write_back(&mut controller, &original)
            .await
            .expect("the page's first write-back must land");
    }
    {
        let mut controller = build_controller(&f, &root, None);
        controller
            .on_file_changed(&old_file)
            .await
            .expect("ingesting the file must stamp its content hash");
    }

    let mut controller = build_controller(&f, &root, registrar);
    controller.initialize().await.expect("cold boot");
    controller
        .on_file_changed(&old_file)
        .await
        .expect("the unchanged file must take the fast path, not error");

    // A second live page answers the same name chain and owns no file.
    let (namesake, namesake_child) = f.add_fileless_page("pgnamesake", OLD_TITLE);
    let errors = CapturedErrors::default();
    let wrote = {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(errors.clone())
            .with_max_level(tracing::Level::ERROR)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        write_back_doc(&mut controller, &namesake, &f.block(&namesake_child))
            .await
            .expect("a namesake's write-back must not fail the sync loop")
    };

    assert!(
        !wrote,
        "the namesake {namesake} was allowed to write — a doc that owns no file must not \
         reach one by answering another page's name chain"
    );
    let disclosed = errors.text();
    assert!(
        disclosed.contains("AMBIGUOUS PAGE-FILE PATH"),
        "the namesake's refusal was silent — an operator has nothing to act on. ERROR \
         output was:\n{disclosed}"
    );

    // The owner edits its page. Its own file is the only place that
    // edit can land.
    let edited = f.set_child_text("the owner's later edit");
    write_back(&mut controller, &edited)
        .await
        .expect("the owner's write-back must not fail the sync loop");

    let on_disk = std::fs::read_to_string(&old_file).unwrap();
    assert!(
        on_disk.contains("the owner's later edit"),
        "the namesake {namesake} took {old_file:?} away from its owner — the owner's \
         later edit never reached disk. On disk:\n{on_disk}"
    );
    for path in all_org_files(&root) {
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !body.contains("a line only the store holds"),
            "the refused namesake's content reached {path:?} anyway:\n{body}"
        );
    }
}

/// Loro wiring — the owner's home is also in the alias registry.
#[tokio::test]
async fn a_namesake_docs_writeback_leaves_the_owners_file_writable() {
    a_namesake_never_takes_the_owners_file(Some(Arc::new(MapAliasRegistrar::default()))).await;
}

/// SqlOnly wiring: `doc_home` is the only record
/// of the owner's file, and the refusal has to hold on it alone.
#[tokio::test]
async fn a_namesake_docs_writeback_leaves_the_owners_file_writable_without_a_registrar() {
    a_namesake_never_takes_the_owners_file(None).await;
}
