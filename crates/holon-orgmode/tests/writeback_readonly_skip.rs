//! Directed regression for the EROFS write-back storm (BugFunnel 2026-07-22
//! `Read-only file system (os error 30)`).
//!
//! Real-vault boot symptom: a doc whose resolved `.org` path has NO writable
//! backing file (a relay/synthetic doc, or a read-only vault mount) re-issued
//! the SAME doomed `write` syscall on EVERY CDC event — hundreds of identical
//! `os error 30` lines. This is an ENVIRONMENT/COVERAGE gap: no test exercised
//! write-back for a doc lacking a writable backing file.
//!
//! Contract (skip-with-one-loud-error): the FIRST failure is disclosed loudly
//! (doc_id + path + cause) and the path is marked; every subsequent CDC event
//! SKIPS the write (no retry). A doc that later gains a writable alias resumes
//! write-back. This test pins the storm: N content edits => exactly ONE write
//! syscall, and no event propagates the error (the sync loop survives).

#![cfg(feature = "di")]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
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
use holon_filesystem::fs_port::FileSystem;
use holon_filesystem::fs_port::RealFileSystem;
use holon_filesystem::fs_port::ScannedEntries;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

/// Authoritative block store stub (stands in for `block_raw`).
struct StubReader {
    doc_id: EntityUri,
    blocks: Mutex<Vec<Block>>,
}

impl StubReader {
    fn set_block(&self, block: &Block) {
        let mut blocks = self.blocks.lock().unwrap();
        if let Some(existing) = blocks.iter_mut().find(|b| b.id == block.id) {
            *existing = block.clone();
        }
    }
}

#[async_trait]
impl BlockReader for StubReader {
    async fn get_blocks(&self, _doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self.blocks.lock().unwrap().clone())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == *id)
            .cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(vec![(
            self.doc_id.clone(),
            self.blocks.lock().unwrap().clone(),
        )])
    }
}

/// Minimal document manager: `name_chain` gives the doc a single-segment path
/// so `doc_id_to_path` resolves to `<root>/doc.org`.
struct StubDocManager {
    doc: Block,
}

#[async_trait]
impl DocumentManager for StubDocManager {
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

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok((self.doc.id == *id).then(|| self.doc.clone()))
    }

    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }

    async fn name_chain(&self, _: &EntityUri) -> anyhow::Result<Vec<String>> {
        Ok(vec!["doc".to_string()])
    }
}

/// Ordering stub whose `children` mirrors the authoritative store (matches
/// production: `children` and `get_blocks` read the same underlying order).
struct StubOrdering {
    reader: Arc<StubReader>,
}

#[async_trait]
impl BlockOrdering for StubOrdering {
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
            .reader
            .blocks
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

/// Filesystem whose `write` ALWAYS fails with EROFS (os error 30) — simulates a
/// relay/synthetic doc's path with no writable backing file, or a read-only
/// vault mount. Reads and dir ops delegate to the real fs (the temp root
/// exists and is readable). Every write attempt is counted so the test can
/// assert one-shot vs. per-CDC-event storm.
struct ReadOnlyWriteFs {
    inner: RealFileSystem,
    write_attempts: AtomicUsize,
    /// When true, `write` fails with EROFS; flip to false to simulate the path
    /// becoming writable again (e.g. a read-only mount remounted rw), so the
    /// resume test can prove edits resume after a re-ingest clears the mark.
    readonly: AtomicBool,
}

impl ReadOnlyWriteFs {
    fn new() -> Self {
        Self {
            inner: RealFileSystem,
            write_attempts: AtomicUsize::new(0),
            readonly: AtomicBool::new(true),
        }
    }
    fn write_attempts(&self) -> usize {
        self.write_attempts.load(Ordering::SeqCst)
    }
    fn set_writable(&self) {
        self.readonly.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl FileSystem for ReadOnlyWriteFs {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.inner.read_to_string(path).await
    }
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.inner.read(path).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        if self.readonly.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ReadOnlyFilesystem,
                format!("Read-only file system (os error 30): {}", path.display()),
            ));
        }
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

fn make_doc_and_blocks() -> (Block, Vec<Block>) {
    let doc_id = EntityUri::block("relay0000001");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "Relay Doc");
    doc.set_page(true);
    let b1 = Block::new_text(EntityUri::block("b1"), doc_id.clone(), "First heading");
    let b2 = Block::new_text(EntityUri::block("b2"), doc_id, "Second heading");
    (doc, vec![b1, b2])
}

/// N CDC content edits against a doc whose backing path is on a read-only
/// filesystem must issue EXACTLY ONE write syscall (skip-with-one-loud-error),
/// not one per event, and no event may propagate the EROFS error (the sync
/// loop keeps serving every other document).
#[tokio::test]
async fn readonly_writeback_logs_once_then_skips_no_per_cdc_retry() {
    let (doc, blocks) = make_doc_and_blocks();
    let reader = Arc::new(StubReader {
        doc_id: doc.id.clone(),
        blocks: Mutex::new(blocks),
    });
    let doc_manager = Arc::new(StubDocManager { doc: doc.clone() });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let fs = Arc::new(ReadOnlyWriteFs::new());
    let mut controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root,
        Arc::new(StubOrdering {
            reader: reader.clone(),
        }),
        fs.clone(),
    );

    const EVENTS: usize = 4;
    let mut all_ok = true;
    for i in 0..EVENTS {
        // Distinct content each event so the render differs and a write is
        // attempted (a no-op render short-circuits before the write).
        let mut edited = reader.blocks.lock().unwrap()[0].clone();
        edited.content = format!("First heading edit {i}");
        reader.set_block(&edited);
        let r = controller
            .on_block_changed(&doc.id, &BlockDelta::Upsert(edited))
            .await;
        all_ok &= r.is_ok();
    }

    assert_eq!(
        fs.write_attempts(),
        1,
        "EROFS write-back must be attempted ONCE then skipped, not retried on \
         every CDC event (got {} attempts across {} events — the os-error-30 \
         boot storm)",
        fs.write_attempts(),
        EVENTS,
    );
    assert!(
        all_ok,
        "a CDC event propagated the EROFS error instead of the disclosed \
         skip — that would kill the sync loop for every other document",
    );
}

/// RESUME regression (coverage for the LANDED clear path, not red-first): after
/// the EROFS storm marks a path, a successful re-ingest of the file — the SOLE
/// resume trigger, `ingest_file` (reached via `on_file_changed`) — lifts the
/// mark, so the NEXT CDC edit issues a REAL write to disk again. Mutation-check:
/// removing the clear in `ingest_file` leaves the mark set and this test goes
/// red (assert (4) fails — the resumed edit never reaches disk).
#[tokio::test]
async fn readonly_writeback_resumes_after_reingest_clears_the_mark() {
    let (doc, blocks) = make_doc_and_blocks();
    let reader = Arc::new(StubReader {
        doc_id: doc.id.clone(),
        blocks: Mutex::new(blocks),
    });
    let doc_manager = Arc::new(StubDocManager { doc: doc.clone() });
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize so the controller's strip_prefix(root) sees the same
    // /private/var shape it derives on macOS (where /var is a symlink).
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("doc.org");
    let fs = Arc::new(ReadOnlyWriteFs::new());
    let mut controller = new_org_sync_controller(
        reader.clone(),
        doc_manager,
        root.clone(),
        Arc::new(StubOrdering {
            reader: reader.clone(),
        }),
        fs.clone(),
    );

    // (1) Storm on a read-only path: the mark is set with exactly one attempt.
    let mut edited = reader.blocks.lock().unwrap()[0].clone();
    edited.content = "First heading edit A".to_string();
    reader.set_block(&edited);
    controller
        .on_block_changed(&doc.id, &BlockDelta::Upsert(edited))
        .await
        .unwrap();
    assert_eq!(
        fs.write_attempts(),
        1,
        "storm must set the read-only mark with exactly one write attempt",
    );

    // (2) The path becomes writable AND the file now exists on disk (a real
    //     re-ingest reads it). Write a matching bare-`#+ID:` file directly.
    std::fs::write(&path, format!("#+ID: {}\n", doc.id.id())).unwrap();
    fs.set_writable();

    // (3) Re-ingest via on_file_changed — the SOLE resume trigger. Clears the
    //     mark (ingest_file) before any reconciliation/write-back.
    controller
        .on_file_changed(&path)
        .await
        .expect("re-ingest of a writable-backed file must not error");

    // (4) The next CDC edit must issue a REAL write that reaches disk.
    let base_attempts = fs.write_attempts();
    let mut edited2 = reader.blocks.lock().unwrap()[0].clone();
    edited2.content = "RESUMED-MARKER-XYZ".to_string();
    reader.set_block(&edited2);
    let r = controller
        .on_block_changed(&doc.id, &BlockDelta::Upsert(edited2))
        .await;
    assert!(r.is_ok(), "resumed write-back must return Ok");
    assert!(
        fs.write_attempts() > base_attempts,
        "a REAL write must be attempted after resume — the mark did not clear",
    );
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("RESUMED-MARKER-XYZ"),
        "the resumed edit must reach disk — the read-only mark did NOT clear on \
         re-ingest (resume regression; on-disk = {on_disk:?})",
    );

    // (5) last_projection was stamped by the resumed write: an IDENTICAL re-edit
    //     renders == last and issues NO further write.
    let after_resume = fs.write_attempts();
    let same = reader.blocks.lock().unwrap()[0].clone();
    controller
        .on_block_changed(&doc.id, &BlockDelta::Upsert(same))
        .await
        .unwrap();
    assert_eq!(
        fs.write_attempts(),
        after_resume,
        "an idempotent re-edit must not write — last_projection was not stamped \
         by the resumed write",
    );
}
