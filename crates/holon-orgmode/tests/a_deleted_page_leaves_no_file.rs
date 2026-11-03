//! A page Holon deletes leaves no file behind and no path record of it,
//! whichever writer created its file, in whatever order the deletion's changes
//! arrive, and while the page registry still lists the deleted page. A page
//! that is not deleted keeps its file, and the undo of the delete writes the
//! page's file again at the same path.
//!
//! The removal refuses a file outside the vault, a file another page owns, and
//! a read-only file. These refusals are fixed scenarios, each pinned to one
//! guard, so they are examples rather than properties.
//!
//! Production's page registry is a view with its own CDC stream, apart from
//! the block feed that drives write-back, so it can still list a page the
//! store has already deleted.

use std::sync::Arc;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockDelta;
use holon_filesystem::sync_ports::AliasRegistrar;
use org_vault_double::DOC_ID;
use org_vault_double::MapAliasRegistrar;
use org_vault_double::Vault;
use org_vault_double::vault;
use proptest::prelude::*;

use crate::org_vault_double;

#[derive(Clone, Copy, Debug)]
enum Writer {
    /// A change of a block inside the page renders the page's file.
    Render,
    /// An upsert of the page block itself runs the identity-file materializer.
    Materialize,
}

#[derive(Clone, Copy, Debug)]
enum Deletion {
    /// The undo of "turn into page": the page goes, its block returns to the
    /// source document.
    UndoConvert,
    /// The page and its block both go.
    Delete,
}

#[derive(Clone, Debug)]
struct Case {
    creation: Vec<Writer>,
    deletion: Deletion,
    /// The order the deletion's changes reach the controller, as indexes into
    /// `deletion_changes`.
    order: Vec<usize>,
    /// How many of those changes reach the controller while the registry
    /// still lists the deleted page.
    registry_lag: usize,
    /// Whether the vault has an alias registry. Production runs without one
    /// when the CRDT layer is off.
    aliases: bool,
}

struct MintedPage {
    page: Block,
    kid: Block,
}

impl MintedPage {
    fn new(id: &str, kid_id: &str, title: &str) -> Self {
        let mut page = Block::new_text(EntityUri::block(id), EntityUri::no_parent(), title);
        page.set_page(true);
        let kid = Block::new_text(EntityUri::block(kid_id), page.id.clone(), "a task");
        Self { page, kid }
    }
}

/// The changes a deletion sends, each with the document it is homed in.
fn deletion_changes(minted: &MintedPage, deletion: Deletion) -> Vec<(EntityUri, BlockDelta)> {
    let page = &minted.page.id;
    let mut changes = vec![
        (page.clone(), BlockDelta::Remove(minted.kid.id.clone())),
        (page.clone(), BlockDelta::Remove(page.clone())),
    ];
    if let Deletion::UndoConvert = deletion {
        changes.push((EntityUri::block(DOC_ID), upsert(&kid_back_home(minted))));
    }
    changes
}

fn kid_back_home(minted: &MintedPage) -> Block {
    let mut back = minted.kid.clone();
    back.parent_id = EntityUri::block(DOC_ID);
    back
}

fn upsert(block: &Block) -> BlockDelta {
    BlockDelta::Upsert {
        block: block.clone(),
        prev: None,
    }
}

async fn change(v: &mut Vault, doc: &EntityUri, delta: &BlockDelta) -> bool {
    v.controller
        .on_block_changed(doc, delta)
        .await
        .unwrap_or_else(|e| panic!("on_block_changed({doc}): {e:#}"))
}

async fn create(v: &mut Vault, minted: &MintedPage, creation: &[Writer]) {
    v.put(minted.page.clone());
    v.put(minted.kid.clone());
    for writer in creation {
        let block = match writer {
            Writer::Render => &minted.kid,
            Writer::Materialize => &minted.page,
        };
        change(v, &minted.page.id, &upsert(block)).await;
    }
}

/// Deletes the page in the store at once and in the registry after
/// `case.registry_lag` changes. Returns whether any change asked for the bulk
/// pass.
async fn delete(v: &mut Vault, minted: &MintedPage, case: &Case) -> bool {
    let page = &minted.page.id;
    let listed = v.docs.by_id.lock().unwrap()[page].clone();
    v.forget(page);
    match case.deletion {
        Deletion::UndoConvert => v.put(kid_back_home(minted)),
        Deletion::Delete => v.forget(&minted.kid.id),
    }
    v.docs.by_id.lock().unwrap().insert(page.clone(), listed);

    let changes = deletion_changes(minted, case.deletion);
    let mut handled = true;
    for (sent, &i) in case.order.iter().enumerate() {
        if sent == case.registry_lag {
            v.docs.by_id.lock().unwrap().remove(page);
        }
        let (doc, delta) = &changes[i];
        handled &= change(v, doc, delta).await;
    }
    v.docs.by_id.lock().unwrap().remove(page);
    !handled
}

/// The vault files that name `page` in their bytes. `.holon/` holds
/// content-addressed merge bases, which a sweep removes, not page files.
fn files_naming(v: &Vault, page: &EntityUri) -> Vec<String> {
    let root = v.path.parent().unwrap();
    let mut found = Vec::new();
    let mut open = vec![root.to_path_buf()];
    while let Some(dir) = open.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != ".holon" {
                    open.push(path);
                }
            } else if std::fs::read_to_string(&path).unwrap().contains(page.id()) {
                found.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    found
}

/// Undoes the deletion: the page and its block return to the store, and
/// their changes reach the controller.
async fn restore(v: &mut Vault, minted: &MintedPage, deletion: Deletion) {
    let page = &minted.page.id;
    let mut changes = Vec::new();
    if let Deletion::UndoConvert = deletion {
        v.forget(&minted.kid.id);
        changes.push((
            EntityUri::block(DOC_ID),
            BlockDelta::Remove(minted.kid.id.clone()),
        ));
    }
    v.put(minted.page.clone());
    v.put(minted.kid.clone());
    changes.push((page.clone(), upsert(&minted.page)));
    changes.push((page.clone(), upsert(&minted.kid)));
    let mut handled = true;
    for (doc, delta) in &changes {
        handled &= change(v, doc, delta).await;
    }
    if !handled {
        v.controller
            .re_render_all_tracked(&Default::default())
            .await
            .unwrap_or_else(|e| panic!("bulk pass: {e:#}"));
    }
}

/// What the case left on disk or in the path records that it should not
/// have, or `None`.
async fn run(case: &Case) -> Option<String> {
    let aliases = Arc::new(MapAliasRegistrar::default());
    let mut v = match case.aliases {
        true => vault().with_alias_registrar(aliases.clone()),
        false => vault(),
    };
    let notes = v.path.clone();
    let _ = v.controller.on_file_changed(&notes).await.unwrap();
    let root = v.path.parent().unwrap().to_path_buf();

    let doomed = MintedPage::new("20261102T100000", "20261102T100001", "Origin");
    let keeper = MintedPage::new("20261102T110000", "20261102T110001", "Keeper");
    create(&mut v, &doomed, &case.creation).await;
    create(&mut v, &keeper, &case.creation).await;
    let before = files_naming(&v, &doomed.page.id);
    assert_eq!(before.len(), 1, "the new page owns one file: {before:?}");
    let home = root.join(&before[0]);
    let bytes = std::fs::read_to_string(&home).unwrap();

    if delete(&mut v, &doomed, case).await {
        v.controller
            .re_render_all_tracked(&Default::default())
            .await
            .unwrap_or_else(|e| panic!("bulk pass: {e:#}"));
    }

    if home.exists() {
        return Some(format!(
            "the deleted page's file {} still exists, holding {:?}",
            before[0],
            std::fs::read_to_string(&home).unwrap()
        ));
    }
    let doomed_files = files_naming(&v, &doomed.page.id);
    let keeper_files = files_naming(&v, &keeper.page.id);
    if !doomed_files.is_empty() || keeper_files.len() != 1 {
        return Some(format!(
            "deleted page still in {doomed_files:?}; live page in {keeper_files:?}"
        ));
    }
    let alias = aliases.resolve_alias_to_path(&doomed.page.id).await;
    if alias.is_some() || v.controller.tracks_file(&home) {
        return Some(format!(
            "the records keep the removed file {}: alias {alias:?}, home or last projection {}",
            before[0],
            v.controller.tracks_file(&home)
        ));
    }

    restore(&mut v, &doomed, case.deletion).await;
    let again = std::fs::read_to_string(&home).ok();
    (again.as_deref() != Some(bytes.as_str())).then(|| {
        format!(
            "the undo did not write {} again: {again:?}, expected {bytes:?}; the page is in {:?}",
            before[0],
            files_naming(&v, &doomed.page.id)
        )
    })
}

fn check(case: &Case) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Some(failure) = rt.block_on(run(case)) {
        panic!("{case:?}: {failure}");
    }
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let writer = prop_oneof![Just(Writer::Render), Just(Writer::Materialize)];
    let deletion = prop_oneof![Just(Deletion::UndoConvert), Just(Deletion::Delete)];
    (
        prop::collection::vec(writer, 1..=3),
        deletion,
        any::<bool>(),
    )
        .prop_flat_map(|(creation, deletion, aliases)| {
            // A delete removes leaves before their parent
            // (holon-loro loro_sync_controller.rs, "Delete leaves first"); a page
            // removed before its block is ungrounded and the removal guard refuses it.
            let (n, order) = match deletion {
                Deletion::UndoConvert => (3usize, Just(vec![0, 1, 2]).prop_shuffle().boxed()),
                Deletion::Delete => (2, Just(vec![0, 1]).boxed()),
            };
            (order, 0..=n).prop_map(move |(order, registry_lag)| Case {
                creation: creation.clone(),
                deletion,
                order,
                registry_lag,
                aliases,
            })
        })
}

/// Cases kept from earlier failures, run through the same property first.
fn fixed_cases() -> Vec<Case> {
    let in_order = |creation: Vec<Writer>, deletion, n, registry_lag| Case {
        creation,
        deletion,
        order: (0..n).collect(),
        registry_lag,
        aliases: true,
    };
    vec![
        // The render path wrote the page's file without recording its alias.
        in_order(vec![Writer::Render], Deletion::UndoConvert, 3, 0),
        in_order(vec![Writer::Render], Deletion::Delete, 2, 0),
        // The registry still listed the deleted page when its removal was
        // rendered, so the file kept the page's header.
        in_order(vec![Writer::Materialize], Deletion::UndoConvert, 3, 3),
        in_order(vec![Writer::Render], Deletion::UndoConvert, 3, 3),
        // With no alias registry, only the home record names the deleted
        // page's file once the registry has dropped the page.
        Case {
            aliases: false,
            ..in_order(vec![Writer::Render], Deletion::UndoConvert, 3, 0)
        },
    ]
}

#[test]
fn a_deleted_page_leaves_no_file_fixed_cases() {
    for case in fixed_cases() {
        check(&case);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_deleted_page_leaves_no_file_and_a_live_page_keeps_its_file(case in case_strategy()) {
        check(&case);
    }
}

/// Deletes `doomed` after `aim` re-points its alias, and returns the vault.
async fn delete_with_alias_at(
    aim: impl FnOnce(&std::path::Path) -> std::path::PathBuf,
    keeper_kid: bool,
) -> (Vault, std::path::PathBuf, String) {
    let aliases = Arc::new(MapAliasRegistrar::default());
    let mut v = vault().with_alias_registrar(aliases.clone());
    let notes = v.path.clone();
    let _ = v.controller.on_file_changed(&notes).await.unwrap();
    let root = v.path.parent().unwrap().to_path_buf();
    let doomed = MintedPage::new("20261102T100000", "20261102T100001", "Origin");
    let keeper = MintedPage::new("20261102T110000", "20261102T110001", "Keeper");
    create(&mut v, &doomed, &[Writer::Render]).await;
    v.put(keeper.page.clone());
    let keeper_writer = if keeper_kid {
        v.put(keeper.kid.clone());
        &keeper.kid
    } else {
        &keeper.page
    };
    change(&mut v, &keeper.page.id, &upsert(keeper_writer)).await;
    let target = aim(&root);
    let bytes = std::fs::read_to_string(&target).unwrap();
    aliases.register_alias(&doomed.page.id, &target).await;
    let case = Case {
        creation: vec![Writer::Render],
        deletion: Deletion::Delete,
        order: vec![0, 1],
        registry_lag: 0,
        aliases: true,
    };
    if delete(&mut v, &doomed, &case).await {
        v.controller
            .re_render_all_tracked(&Default::default())
            .await
            .unwrap_or_else(|e| panic!("bulk pass: {e:#}"));
    }
    (v, target, bytes)
}

#[tokio::test]
async fn a_deleted_page_never_removes_a_file_outside_the_vault() {
    let outside = tempfile::tempdir().unwrap();
    let outside_root = std::fs::canonicalize(outside.path()).unwrap();
    let (_v, target, bytes) = delete_with_alias_at(
        |root| {
            let copy = outside_root.join("Origin.org");
            std::fs::copy(root.join("Origin.org"), &copy).unwrap();
            copy
        },
        true,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(&target).ok().as_deref(),
        Some(bytes.as_str()),
        "the file outside the vault keeps its bytes"
    );
}

#[tokio::test]
async fn a_deleted_page_never_removes_the_file_of_a_live_page() {
    let (v, target, bytes) = delete_with_alias_at(|root| root.join("Keeper.org"), false).await;
    assert_eq!(
        std::fs::read_to_string(&target).ok().as_deref(),
        Some(bytes.as_str()),
        "the live page's file keeps its bytes"
    );
    assert!(
        v.controller.tracks_file(&target),
        "the live page keeps its file's records"
    );
}

mod read_only_home {
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use async_trait::async_trait;
    use holon_api::block::Block;
    use holon_api::entity_uri::EntityUri;
    use holon_core::block_ordering::BlockOrdering;
    use holon_core::traits::Result as OrderingResult;
    use holon_filesystem::BlockDelta;
    use holon_filesystem::BlockReader;
    use holon_filesystem::DocumentManager;
    use holon_filesystem::fs_port::FileMeta;
    use holon_filesystem::fs_port::FileStamp;
    use holon_filesystem::fs_port::FileSystem;
    use holon_filesystem::fs_port::RealFileSystem;
    use holon_filesystem::fs_port::ScannedEntries;
    use holon_filesystem::fs_port::StampedRead;
    use holon_filesystem::fs_port::WriteBack;
    use holon_orgmode::file_sync_controller::new_org_sync_controller;

    #[derive(Default)]
    struct Blocks(Mutex<Vec<Block>>);

    #[async_trait]
    impl BlockReader for Blocks {
        async fn get_blocks(&self, doc: &EntityUri) -> anyhow::Result<Vec<Block>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|b| b.id != *doc)
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
            Ok(self.0.lock().unwrap().iter().find(|b| b.id == *id).cloned())
        }
        async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
            Ok(Vec::new())
        }
    }

    /// Routes every page to `Page.org`.
    struct OnePageFile;

    #[async_trait]
    impl DocumentManager for OnePageFile {
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
        async fn get_by_id(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
            Ok(None)
        }
        async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
            Ok(())
        }
        async fn name_chain(&self, _: &EntityUri) -> anyhow::Result<Vec<String>> {
            Ok(vec!["Page".to_string()])
        }
    }

    struct Ordering_(Arc<Blocks>);

    #[async_trait]
    impl BlockOrdering for Ordering_ {
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
                .0
                .0
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

    /// A mount that counts removals. When read-only, every write and removal
    /// of a page file fails with EROFS; Holon's own `.holon/` stays writable.
    struct Mount {
        read_only: bool,
        removals: AtomicUsize,
    }

    impl Mount {
        fn refuses(&self, path: &Path) -> bool {
            self.read_only && path.extension().is_some_and(|e| e == "org")
        }
    }

    fn erofs(path: &Path) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::ReadOnlyFilesystem,
            format!("Read-only file system (os error 30): {}", path.display()),
        )
    }

    #[async_trait]
    impl FileSystem for Mount {
        async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            RealFileSystem.read_to_string(path).await
        }
        async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
            RealFileSystem.read(path).await
        }
        async fn read_stamped(&self, path: &Path) -> std::io::Result<StampedRead> {
            RealFileSystem.read_stamped(path).await
        }
        async fn write_if_unchanged(
            &self,
            path: &Path,
            expected: &FileStamp,
            contents: &[u8],
        ) -> std::io::Result<WriteBack> {
            if self.refuses(path) {
                return Err(erofs(path));
            }
            RealFileSystem
                .write_if_unchanged(path, expected, contents)
                .await
        }
        async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
            if self.refuses(path) {
                return Err(erofs(path));
            }
            RealFileSystem.write(path, contents).await
        }
        async fn remove(&self, path: &Path) -> std::io::Result<()> {
            self.removals.fetch_add(1, Ordering::SeqCst);
            if self.refuses(path) {
                return Err(erofs(path));
            }
            RealFileSystem.remove(path).await
        }
        async fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
            RealFileSystem.create_dir_all(path).await
        }
        async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries> {
            RealFileSystem.scan_directory(root).await
        }
        async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
            RealFileSystem.metadata(path).await
        }
        fn exists(&self, path: &Path) -> bool {
            RealFileSystem.exists(path)
        }
        fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
            RealFileSystem.canonicalize(path)
        }
    }

    /// Edits a page, then deletes it, on a mount that is read-only or not.
    /// Returns the mount, the vault dir, the page's file and its bytes before
    /// the edit.
    async fn edit_then_delete(read_only: bool) -> (Arc<Mount>, tempfile::TempDir, PathBuf, String) {
        let mut page = Block::new_text(
            EntityUri::block("20261102T120000"),
            EntityUri::no_parent(),
            "Page",
        );
        page.set_page(true);
        let kid = Block::new_text(
            EntityUri::block("20261102T120001"),
            page.id.clone(),
            "a task",
        );
        let blocks = Arc::new(Blocks(Mutex::new(vec![page.clone(), kid.clone()])));
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let path = root.join("Page.org");
        let bytes = format!(
            "#+ID: {}\n* a task\n:PROPERTIES:\n:ID: {}\n:END:\n",
            page.id.id(),
            kid.id.id()
        );
        std::fs::write(&path, &bytes).unwrap();
        let mount = Arc::new(Mount {
            read_only,
            removals: AtomicUsize::new(0),
        });
        let mut controller = new_org_sync_controller(
            blocks.clone(),
            Arc::new(OnePageFile),
            root,
            Arc::new(Ordering_(blocks.clone())),
            mount.clone(),
        );
        controller
            .seed_holder_from_authority(&page.id)
            .await
            .unwrap();

        // On a read-only mount the failed write reports once and marks the
        // file read-only.
        let mut edited = kid.clone();
        edited.content = "an edited task".to_string();
        blocks.0.lock().unwrap()[1] = edited.clone();
        let edit = controller
            .on_block_changed(
                &page.id,
                &BlockDelta::Upsert {
                    block: edited,
                    prev: None,
                },
            )
            .await;
        let mut handled = if read_only {
            let e = format!("{:#}", edit.unwrap_err());
            assert!(e.contains("read-only file system"), "edit: {e}");
            true
        } else {
            edit.unwrap_or_else(|e| panic!("edit: {e:#}"))
        };

        blocks.0.lock().unwrap().clear();
        for id in [&kid.id, &page.id] {
            handled &= controller
                .on_block_changed(&page.id, &BlockDelta::Remove(id.clone()))
                .await
                .unwrap_or_else(|e| panic!("remove {id}: {e:#}"));
        }
        if !handled {
            controller
                .re_render_all_tracked(&Default::default())
                .await
                .unwrap_or_else(|e| panic!("bulk pass: {e:#}"));
        }
        (mount, tmp, path, bytes)
    }

    #[tokio::test]
    async fn a_deleted_page_never_removes_a_read_only_file() {
        let (mount, _vault, path, bytes) = edit_then_delete(true).await;
        assert_eq!(
            mount.removals.load(Ordering::SeqCst),
            0,
            "no removal was attempted"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            bytes,
            "the file keeps its bytes"
        );
    }

    /// The same steps on a writable mount do remove the file, so the
    /// read-only refusal above is the guard that keeps it.
    #[tokio::test]
    async fn the_same_deletion_on_a_writable_mount_removes_the_file() {
        let (mount, _vault, path, _) = edit_then_delete(false).await;
        assert_eq!(mount.removals.load(Ordering::SeqCst), 1, "one removal");
        assert!(!path.exists(), "the deleted page's file is gone");
    }
}
