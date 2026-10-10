//! A block edited in the app and, before the write-back, in its org file
//! merges both edits: text the app deleted stays deleted.
//!
//! Direct mode (no Loro store): the file-sync controller 3-way merges the
//! block's text with the merger the app wires (`TransientLoroTextMerge`).
//!
//! @pbt kind harness
//! @pbt covers file-sync-3way-text-merge — an external org edit and an in-app
//! edit of one block merge without resurrecting deleted text

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
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
use holon_filesystem::FileSystem;
use holon_filesystem::IngestOutcome;
use holon_filesystem::RealFileSystem;
use holon_filesystem::fs_port::FileMeta;
use holon_filesystem::fs_port::FileStamp;
use holon_filesystem::fs_port::ScannedEntries;
use holon_filesystem::fs_port::StampedRead;
use holon_filesystem::fs_port::WriteBack;
use holon_loro::TransientLoroTextMerge;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

const DOC_ID: &str = "20261009T090000";
const ALPHA: &str = "20261009T090001";

type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

/// The SQL store of Direct mode: it is the consolidator, and an ingest's
/// content update lands in it.
#[derive(Clone)]
struct StoreTree {
    store: Store,
}

#[async_trait]
impl BlockOrdering for StoreTree {
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
    async fn ever_seen(&self, _: &EntityUri) -> OrderingResult<Seen> {
        Ok(Seen::Live)
    }
    fn has_upstream_consolidator(&self) -> bool {
        false
    }
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("every ingest op names the block it acts on");
        let id = EntityUri::parse(id).expect("the ingest names blocks by uri");
        let mut store = self.store.lock().unwrap();
        let block = store
            .get_mut(&id)
            .unwrap_or_else(|| panic!("the ingest updates {id}, which the store does not hold"));
        if let Some(text) = params.get("content").and_then(|v| v.as_string()) {
            block.content = text.to_string();
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

fn org_file(alpha: &str) -> String {
    format!(
        ":PROPERTIES:\n:ID: {DOC_ID}\n:END:\n#+TITLE: Notes\n\
         * {alpha}\n:PROPERTIES:\n:ID: {ALPHA}\n:END:\n"
    )
}

#[tokio::test]
async fn a_file_edit_does_not_bring_back_text_the_app_deleted() {
    let doc = EntityUri::block(DOC_ID);
    let alpha = EntityUri::block(ALPHA);
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);
    let store: Store = Arc::default();
    store.lock().unwrap().insert(
        alpha.clone(),
        Block::new_text(alpha.clone(), doc.clone(), "abcdef"),
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, org_file("abcdef")).unwrap();

    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root,
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()));
    let first = controller
        .on_file_changed(&path)
        .await
        .expect("first ingest of the file");
    assert_eq!(
        store.lock().unwrap()[&alpha].content,
        "abcdef",
        "first ingest: {first:?}"
    );

    // The app deletes "cde"; before the write-back, an editor replaces "d"
    // with "ABC" and inserts "D" before "f".
    store.lock().unwrap().get_mut(&alpha).unwrap().content = "abf".to_string();
    std::fs::write(&path, org_file("abcABCeDf")).unwrap();
    let outcome = controller.on_file_changed(&path).await;

    assert_eq!(
        store.lock().unwrap()[&alpha].content,
        "abABCDf",
        "the merge of the app's delete of \"cde\" and the file's edit must keep \
         \"cde\" deleted and keep the file's \"ABC\" and \"D\" (ingest outcome {outcome:?})"
    );
}

/// The text of `ALPHA` that an overruled file edit discloses, and Holon's
/// change that overrules it.
fn overruled_text(bus: &holon_api::ConditionBus) -> Option<(String, holon_api::HolonChange)> {
    bus.current().into_iter().find_map(|c| match c.reason {
        holon_api::ConditionKind::FileEditOverruled {
            file_text, change, ..
        } if c.subject == EntityUri::block(ALPHA).as_str() => Some((file_text, change)),
        _ => None,
    })
}

/// Every conflict copy in `dir`, with its bytes.
fn conflict_copies(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut copies: Vec<(PathBuf, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| holon_core::conflict_copy::is_conflict_copy(path))
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            (path, text)
        })
        .collect();
    copies.sort();
    copies
}

/// A base text and two edits of it too large for the 3-way merge: the app
/// deletes one char, the editor reverses the whole line.
fn too_large_edits() -> (String, String, String) {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let base: String = (0..20_000)
        .map(|_| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            b"abcd"[(seed >> 62) as usize] as char
        })
        .collect();
    let mine = format!("{}{}", &base[..100], &base[101..]);
    let theirs: String = base.chars().rev().collect();
    (base, mine, theirs)
}

#[tokio::test]
async fn edits_too_large_to_merge_keep_both_texts_and_disclose_the_files() {
    let doc = EntityUri::block(DOC_ID);
    let alpha = EntityUri::block(ALPHA);
    let (base, mine, theirs) = too_large_edits();
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);
    let store: Store = Arc::default();
    store.lock().unwrap().insert(
        alpha.clone(),
        Block::new_text(alpha.clone(), doc.clone(), base.clone()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, org_file(&base)).unwrap();

    let bus = Arc::new(holon_api::ConditionBus::new());
    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs.clone()),
        root.clone(),
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()))
    .with_writeback_disclosure(Arc::new(
        holon_app::loro_seams::WritebackDegradedDisclosure { bus: bus.clone() },
    ));
    let first = controller
        .on_file_changed(&path)
        .await
        .expect("first ingest of the file");
    assert_eq!(first, IngestOutcome::Ingested);

    store.lock().unwrap().get_mut(&alpha).unwrap().content = mine.clone();
    std::fs::write(&path, org_file(&theirs)).unwrap();
    let outcome = controller.on_file_changed(&path).await;

    assert!(
        store.lock().unwrap()[&alpha].content == mine,
        "the app's text must stand (ingest outcome {outcome:?})"
    );
    let (disclosed, change) =
        overruled_text(&bus).expect("the file's edit is disclosed as overruled");
    assert_eq!(change, holon_api::HolonChange::Edited);
    assert!(
        disclosed.contains(&theirs),
        "the disclosure must hold the file's text whole"
    );

    let copies = conflict_copies(&root);
    let [(copy, copy_text)] = copies.as_slice() else {
        panic!("the overruling write-back must leave exactly one conflict copy, found {copies:?}");
    };
    assert_eq!(
        copy_text,
        &org_file(&theirs),
        "the conflict copy must hold the editor's file byte-equal"
    );
    let named: Vec<String> = bus.current().iter().map(|c| format!("{c:?}")).collect();
    assert!(
        named
            .iter()
            .any(|c| c.contains("FileEditOverruled") && c.contains(&copy.display().to_string())),
        "the overruled-edit condition must name the conflict copy {}: {named:?}",
        copy.display()
    );

    drop(controller);
    let mut restarted = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs.clone()),
        root.clone(),
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()));
    let ingested = restarted
        .poll_new_files()
        .await
        .expect("the restart's scan of the vault");
    assert_eq!(
        ingested, 1,
        "the restart ingests Notes.org and never its conflict copy"
    );
    assert_eq!(
        conflict_copies(&root),
        copies,
        "the conflict copy must survive a restart unchanged"
    );
    assert!(
        store.lock().unwrap()[&alpha].content == mine,
        "after a restart the app's text must still stand"
    );
    assert_eq!(
        docs.by_id.lock().unwrap().len(),
        1,
        "the conflict copy must not become a document"
    );
}

/// The real file system, except that writing a conflict copy fails.
struct ConflictCopiesUnwritable;

#[async_trait]
impl FileSystem for ConflictCopiesUnwritable {
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
        RealFileSystem
            .write_if_unchanged(path, expected, contents)
            .await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        if holon_core::conflict_copy::is_conflict_copy(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "conflict copies are unwritable here",
            ));
        }
        RealFileSystem.write(path, contents).await
    }
    async fn remove(&self, path: &Path) -> std::io::Result<()> {
        RealFileSystem.remove(path).await
    }
    async fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        RealFileSystem.rename(from, to).await
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

#[tokio::test]
async fn an_unwritable_conflict_copy_blocks_the_overruling_write_back_and_is_disclosed() {
    let doc = EntityUri::block(DOC_ID);
    let alpha = EntityUri::block(ALPHA);
    let (base, mine, theirs) = too_large_edits();
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);
    let store: Store = Arc::default();
    store.lock().unwrap().insert(
        alpha.clone(),
        Block::new_text(alpha.clone(), doc.clone(), base.clone()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, org_file(&base)).unwrap();

    let bus = Arc::new(holon_api::ConditionBus::new());
    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root.clone(),
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        Arc::new(ConflictCopiesUnwritable),
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()))
    .with_writeback_disclosure(Arc::new(
        holon_app::loro_seams::WritebackDegradedDisclosure { bus: bus.clone() },
    ));
    let first = controller
        .on_file_changed(&path)
        .await
        .expect("first ingest of the file");
    assert_eq!(first, IngestOutcome::Ingested);

    store.lock().unwrap().get_mut(&alpha).unwrap().content = mine;
    std::fs::write(&path, org_file(&theirs)).unwrap();
    let outcome = controller.on_file_changed(&path).await;

    let error = format!(
        "{:#}",
        outcome.expect_err("an unsaved conflict copy fails the ingest")
    );
    assert!(
        error.contains(".conflict-"),
        "the error must name the conflict copy it could not write: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        org_file(&theirs),
        "without its conflict copy the editor's file must not be overwritten"
    );
    let named: Vec<String> = bus.current().iter().map(|c| format!("{c:?}")).collect();
    assert!(
        named.iter().any(|c| c.contains(".conflict-")),
        "the unwritable conflict copy must be disclosed: {named:?}"
    );
}

/// The real file system, except that the first write-back over a vault file
/// fails and writes nothing.
#[derive(Default)]
struct FirstWriteBackFails {
    refused: Mutex<bool>,
}

#[async_trait]
impl FileSystem for FirstWriteBackFails {
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
        let first = !std::mem::replace(&mut *self.refused.lock().unwrap(), true);
        if first {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "the first write-back fails here",
            ));
        }
        RealFileSystem
            .write_if_unchanged(path, expected, contents)
            .await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        RealFileSystem.write(path, contents).await
    }
    async fn remove(&self, path: &Path) -> std::io::Result<()> {
        RealFileSystem.remove(path).await
    }
    async fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        RealFileSystem.rename(from, to).await
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

/// A vault of one file `file_name` whose block the app and an editor edited
/// too far apart to merge; `fs` is the controller's file system. Returns the
/// vault (kept alive by the `TempDir`), the file, the condition bus, the
/// editor's file text, and the controller.
async fn edited_too_far_apart(
    file_name: &str,
    fs: Arc<dyn FileSystem>,
) -> (
    tempfile::TempDir,
    PathBuf,
    Arc<holon_api::ConditionBus>,
    String,
    holon_filesystem::file_sync_controller::FileSyncController,
) {
    let doc = EntityUri::block(DOC_ID);
    let alpha = EntityUri::block(ALPHA);
    let (base, mine, theirs) = too_large_edits();
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);
    let store: Store = Arc::default();
    store.lock().unwrap().insert(
        alpha.clone(),
        Block::new_text(alpha.clone(), doc.clone(), base.clone()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join(file_name);
    std::fs::write(&path, org_file(&base)).unwrap();

    let bus = Arc::new(holon_api::ConditionBus::new());
    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root,
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        fs,
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()))
    .with_writeback_disclosure(Arc::new(
        holon_app::loro_seams::WritebackDegradedDisclosure { bus: bus.clone() },
    ));
    let first = controller
        .on_file_changed(&path)
        .await
        .expect("first ingest of the file");
    assert_eq!(first, IngestOutcome::Ingested);

    store.lock().unwrap().get_mut(&alpha).unwrap().content = mine;
    let edited = org_file(&theirs);
    std::fs::write(&path, &edited).unwrap();
    (tmp, path, bus, edited, controller)
}

/// A file name long enough that the copy's suffix would push the copy's
/// name past the file system's limit still gets its conflict copy.
#[tokio::test]
async fn a_long_file_name_still_gets_its_conflict_copy() {
    let name = format!("{}.org", "n".repeat(226));
    let (_tmp, path, bus, edited, mut controller) =
        edited_too_far_apart(&name, Arc::new(RealFileSystem)).await;

    let outcome = controller
        .on_file_changed(&path)
        .await
        .expect("the overruling ingest of a file with a long name");
    assert_eq!(outcome, IngestOutcome::Ingested);

    let copies = conflict_copies(path.parent().unwrap());
    let [(copy, copy_text)] = copies.as_slice() else {
        panic!("the overruling write-back must leave exactly one conflict copy, found {copies:?}");
    };
    assert_eq!(copy_text, &edited, "the copy holds the editor's file");
    let named: Vec<String> = bus.current().iter().map(|c| format!("{c:?}")).collect();
    assert!(
        named
            .iter()
            .any(|c| c.contains("FileEditOverruled") && c.contains(&copy.display().to_string())),
        "the overruled-edit condition must name the conflict copy {}: {named:?}",
        copy.display()
    );
}

/// The write-back that overrules the file fails, so the next ingest
/// overrules the same text again: it saves no second copy of that text, and
/// still names the copy.
#[tokio::test]
async fn the_same_overruled_text_is_saved_once() {
    let (_tmp, path, bus, edited, mut controller) =
        edited_too_far_apart("Notes.org", Arc::new(FirstWriteBackFails::default())).await;

    let failed = controller.on_file_changed(&path).await;
    assert!(
        failed.is_err(),
        "premise: the first write-back fails: {failed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        edited,
        "premise: the first write-back writes nothing"
    );
    let outcome = controller
        .on_file_changed(&path)
        .await
        .expect("the ingest of the same text again");
    assert_eq!(outcome, IngestOutcome::Ingested);

    let copies = conflict_copies(path.parent().unwrap());
    let [(copy, copy_text)] = copies.as_slice() else {
        panic!("one overruled text must leave exactly one conflict copy, found {copies:?}");
    };
    assert_eq!(copy_text, &edited, "the copy holds the editor's file");
    let named: Vec<String> = bus.current().iter().map(|c| format!("{c:?}")).collect();
    assert!(
        named
            .iter()
            .any(|c| c.contains("FileEditOverruled") && c.contains(&copy.display().to_string())),
        "the overruled-edit condition must name the conflict copy {}: {named:?}",
        copy.display()
    );
}
