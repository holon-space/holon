//! Inc 3b: per-file error containment in the NEW-file discovery pump.
//!
//! `FileSyncController::poll_new_files` walks the vault to discover files not
//! yet tracked and ingests each one. Before this increment it ingested with
//! `on_file_changed(&path).await?` — a SINGLE poisoned org file (one whose
//! ingest returns `Err`) propagated its error and ABORTED the entire walk. Any
//! file iterated AFTER the poison was never ingested; the next discovery tick
//! re-walked and re-hit the same poison FIRST, so healthy files were never
//! ingested for as long as the poison persisted.
//!
//! These tests pin the required containment at the `poll_new_files` boundary:
//!   (i)   one poisoned file does NOT abort the walk — `poll_new_files` returns
//!         `Ok`, and every HEALTHY file discovered after it is still ingested;
//!   (ii)  the poison is disclosed LOUDLY — exactly one information-rich ERROR
//!         (path + error chain) per failure episode, not a per-tick storm;
//!   (iii) the poison is quarantined from immediate re-attempts (unchanged
//!         bytes are skipped, no re-log) and re-attempted only when its
//!         (mtime, content) changes — the user fixing the file un-quarantines
//!         it, and a clean re-ingest clears the quarantine loudly.
//!
//! A REFUSAL — a file whose `#+ID:` another file on disk already claims — is
//! gated by the same quarantine: refusing is correct and permanent for those
//! bytes, so the work must stop until the file changes, not just the log
//! (bugfunnel
//! `2026-08-31-refused-duplicate-id-file-re-ingests-every-poll-forever`:
//! one file re-read and re-parsed 61,654 times in 18 hours behind a log
//! dedupe).
//!
//! `InMemoryFileSystem` is used deliberately: its `scan_directory` returns
//! files in sorted (`BTreeMap`) order, so the poison (`a_poison.org`) is
//! GUARANTEED to be iterated before the healthy file (`z_healthy.org`) — the
//! exact ordering that made the old `?`-abort starve the healthy file, and the
//! ordering that makes `a_owner.org` the claimant of the shared `#+ID:`.

#![cfg(feature = "di")]

use std::collections::HashMap;
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
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileMeta;
use holon_filesystem::FileSystem;
use holon_filesystem::InMemoryFileSystem;
use holon_filesystem::ScannedEntries;
use holon_orgmode::file_sync_controller::new_org_sync_controller;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

// ── ERROR-level tracing capture (dependency-free). ─────────────────────────

#[derive(Clone, Default)]
struct ErrorCapture(Arc<Mutex<Vec<String>>>);

impl ErrorCapture {
    fn errors(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct MsgVisitor<'a>(&'a mut String);
impl Visit for MsgVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

impl<S: tracing::Subscriber> Layer<S> for ErrorCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            let mut buf = String::new();
            event.record(&mut MsgVisitor(&mut buf));
            self.0.lock().unwrap().push(buf);
        }
    }
}

// ── Test doubles. ──────────────────────────────────────────────────────────

#[derive(Default)]
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

struct EmptyReader;

#[async_trait]
impl BlockReader for EmptyReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(Vec::new())
    }
    /// Delegates to `get_blocks`: this double has no cheaper projection.
    /// Never an empty stub — an empty shape would let the write-back
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

    async fn get_block_authoritative(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// Storing doc manager that FAILS `create_forcing_id` for one chosen page title
/// while `poison_active` is set — a prod-faithful ingest failure (the DB / Loro
/// refusing the create, e.g. Inc 1's held-id collision guard). Records how many
/// times the poison title's create was ATTEMPTED, so a test can prove a
/// quarantined file is NOT re-attempted every tick.
#[derive(Clone)]
struct FaultDocManager {
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
    poison_title: String,
    poison_active: Arc<AtomicBool>,
    poison_attempts: Arc<AtomicUsize>,
}

impl FaultDocManager {
    fn new(poison_title: &str) -> Self {
        Self {
            by_id: Arc::new(Mutex::new(HashMap::new())),
            poison_title: poison_title.to_string(),
            poison_active: Arc::new(AtomicBool::new(true)),
            poison_attempts: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn pages_titled(&self, title: &str) -> Vec<Block> {
        self.by_id
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.is_page() && b.title() == title)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl DocumentManager for FaultDocManager {
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
    async fn create_forcing_id(&self, doc: Block) -> anyhow::Result<Block> {
        if doc.title() == self.poison_title {
            self.poison_attempts.fetch_add(1, Ordering::SeqCst);
            if self.poison_active.load(Ordering::SeqCst) {
                anyhow::bail!(
                    "simulated DB refusal (Inc 3b poison): page store refused to create \
                     '{}'",
                    self.poison_title
                );
            }
        }
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
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Filesystem decorator counting `read_to_string` per path — the side effect a
/// storm-gate has to stop. `metadata` is deliberately NOT counted: the poller
/// stats every candidate on every tick to decide freshness, and that is the
/// cheap part it is supposed to keep doing.
struct CountingFs {
    inner: Arc<InMemoryFileSystem>,
    reads: Mutex<HashMap<PathBuf, usize>>,
}

impl CountingFs {
    fn new(inner: Arc<InMemoryFileSystem>) -> Self {
        Self {
            inner,
            reads: Mutex::new(HashMap::new()),
        }
    }

    fn reads_of(&self, rel: &str) -> usize {
        let path = std::path::Path::new(ROOT).join(rel);
        *self.reads.lock().unwrap().get(&path).unwrap_or(&0)
    }
}

#[async_trait]
impl FileSystem for CountingFs {
    async fn read_to_string(&self, path: &std::path::Path) -> std::io::Result<String> {
        *self
            .reads
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_insert(0) += 1;
        self.inner.read_to_string(path).await
    }
    async fn read(&self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        self.inner.read(path).await
    }
    async fn write(&self, path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
        self.inner.write(path, contents).await
    }
    async fn remove(&self, path: &std::path::Path) -> std::io::Result<()> {
        self.inner.remove(path).await
    }
    async fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        self.inner.rename(from, to).await
    }
    async fn create_dir_all(&self, path: &std::path::Path) -> std::io::Result<()> {
        self.inner.create_dir_all(path).await
    }
    async fn scan_directory(&self, root: &std::path::Path) -> std::io::Result<ScannedEntries> {
        self.inner.scan_directory(root).await
    }
    async fn metadata(&self, path: &std::path::Path) -> std::io::Result<FileMeta> {
        self.inner.metadata(path).await
    }
    fn exists(&self, path: &std::path::Path) -> bool {
        self.inner.exists(path)
    }
    fn canonicalize(&self, path: &std::path::Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
}

const ROOT: &str = "/holon-virtual/inc3b";

async fn write_file(fs: &InMemoryFileSystem, rel: &str, content: &str) {
    let path = std::path::Path::new(ROOT).join(rel);
    fs.write(&path, content.as_bytes()).await.unwrap();
}

fn build(fs: Arc<InMemoryFileSystem>, dm: FaultDocManager) -> holon_filesystem::FileSyncController {
    build_with_fs(fs, dm)
}

fn build_with_fs(
    fs: Arc<dyn FileSystem>,
    dm: FaultDocManager,
) -> holon_filesystem::FileSyncController {
    new_org_sync_controller(
        Arc::new(EmptyReader),
        Arc::new(dm),
        std::path::PathBuf::from(ROOT),
        Arc::new(NoopOrdering),
        fs,
    )
}

fn poison_containment_errors(errs: &[String]) -> Vec<&String> {
    errs.iter()
        .filter(|e| e.contains("CONTAINED a failed ingest") && e.contains("a_poison"))
        .collect()
}

// ── Tests. ─────────────────────────────────────────────────────────────────

/// THE BUG (red-for-the-right-reason before Inc 3b): the poison
/// (`a_poison.org`, iterated first) aborts `poll_new_files`, so the healthy
/// file (`z_healthy.org`) is NEVER ingested. After containment:
/// `poll_new_files` returns `Ok`, the healthy page is created, and the poison
/// is disclosed by exactly one loud ERROR carrying its path + error chain.
#[tokio::test]
async fn one_poison_does_not_starve_healthy_files() {
    let cap = ErrorCapture::default();
    let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let fs = Arc::new(InMemoryFileSystem::new());
    fs.mkdir_all(std::path::Path::new(ROOT));
    write_file(&fs, "a_poison.org", "#+TITLE: A Poison\n").await;
    write_file(&fs, "z_healthy.org", "#+TITLE: Z Healthy\n").await;

    let dm = FaultDocManager::new("a_poison");
    let mut controller = build(fs.clone(), dm.clone());

    // Before Inc 3b this returns Err (the poison's `?` aborts the walk) and the
    // healthy file is never reached — the red.
    let ingested = controller
        .poll_new_files()
        .await
        .expect("poll_new_files must CONTAIN the poisoned file, not abort the whole walk");

    // The healthy file, iterated AFTER the poison, must still be ingested.
    assert_eq!(
        dm.pages_titled("z_healthy").len(),
        1,
        "the healthy file discovered AFTER the poison must still ingest — one poisoned \
         file must not starve the rest of the walk"
    );
    assert!(
        ingested >= 1,
        "at least the healthy file must count as ingested"
    );

    // The poison itself created no page (its ingest genuinely failed).
    assert!(
        dm.pages_titled("a_poison").is_empty(),
        "the poisoned file must NOT have produced a page"
    );

    // Exactly one loud, information-rich containment ERROR for the poison.
    let errs = cap.errors();
    let contained = poison_containment_errors(&errs);
    assert_eq!(
        contained.len(),
        1,
        "expected exactly ONE loud containment ERROR for the poison; got {errs:?}"
    );
    assert!(
        contained[0].contains("simulated DB refusal"),
        "the containment ERROR must carry the underlying error chain; got {}",
        contained[0]
    );
}

/// The quarantine storm-gate + un-quarantine trigger: a poisoned new file is
/// not re-attempted (nor re-logged) on subsequent discovery ticks while its
/// bytes are unchanged, and IS re-attempted — clearing the quarantine — once
/// its content changes (the user fixing the file).
#[tokio::test]
async fn quarantined_file_reattempted_only_after_it_changes() {
    let cap = ErrorCapture::default();
    let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let fs = Arc::new(InMemoryFileSystem::new());
    fs.mkdir_all(std::path::Path::new(ROOT));
    write_file(&fs, "a_poison.org", "#+TITLE: A Poison\n").await;

    let dm = FaultDocManager::new("a_poison");
    let mut controller = build(fs.clone(), dm.clone());

    // Tick 1: poison fails, is contained + quarantined. One create attempt.
    controller
        .poll_new_files()
        .await
        .expect("tick 1 must contain, not abort");
    assert_eq!(dm.poison_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(poison_containment_errors(&cap.errors()).len(), 1);

    // Tick 2: file UNCHANGED — quarantine must skip it. No new attempt, no new
    // loud error (no per-tick storm).
    controller
        .poll_new_files()
        .await
        .expect("tick 2 must not abort");
    assert_eq!(
        dm.poison_attempts.load(Ordering::SeqCst),
        1,
        "a quarantined, unchanged file must NOT be re-ingested every tick"
    );
    assert_eq!(
        poison_containment_errors(&cap.errors()).len(),
        1,
        "an unchanged quarantined file must not re-log a loud error every tick"
    );

    // The user fixes the file: lift the fault AND rewrite the file (bumps mtime
    // + size → signature changes → quarantine re-attempts it).
    dm.poison_active.store(false, Ordering::SeqCst);
    write_file(
        &fs,
        "a_poison.org",
        "#+TITLE: A Poison now fixed and longer\n",
    )
    .await;

    // Tick 3: signature changed → re-attempted → succeeds → quarantine cleared.
    let ingested = controller
        .poll_new_files()
        .await
        .expect("tick 3 must not abort");
    assert_eq!(
        dm.poison_attempts.load(Ordering::SeqCst),
        2,
        "a changed file must be re-attempted"
    );
    assert_eq!(
        dm.pages_titled("a_poison").len(),
        1,
        "after the fix the formerly-poisoned file must ingest"
    );
    assert!(ingested >= 1);
}

// ── Duplicate-`#+ID:` refusal: the storm-gate for REFUSED files. ────────────

const SHARED_ID: &str = "3f1c9b52-6f7a-4d21-9b0e-2a8c4d5e6f70";

fn duplicate_id_errors(errs: &[String]) -> Vec<&String> {
    errs.iter()
        .filter(|e| e.contains("DUPLICATE DOCUMENT ID") && e.contains("z_dupe"))
        .collect()
}

/// Two files carry the same `#+ID:`. The one scanned first claims it; the
/// second is REFUSED — correctly, and permanently for those bytes. THE BUG: the
/// refusal deduped its LOG but not its WORK, so every discovery tick re-read
/// and re-parsed the refused file forever (61,654 times in one 18h session).
///
/// The refusal must be recorded at the file's freshness signature, so a second
/// poll over an unchanged tree does no ingest work at all, and an EDIT — the
/// user giving the file a fresh id — re-ingests it.
#[tokio::test]
async fn refused_duplicate_id_file_is_not_re_ingested_while_unchanged() {
    let cap = ErrorCapture::default();
    let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let disk = Arc::new(InMemoryFileSystem::new());
    disk.mkdir_all(std::path::Path::new(ROOT));
    write_file(
        &disk,
        "a_owner.org",
        &format!("#+TITLE: A Owner\n#+ID: {SHARED_ID}\n"),
    )
    .await;
    write_file(
        &disk,
        "z_dupe.org",
        &format!("#+TITLE: Z Dupe\n#+ID: {SHARED_ID}\n"),
    )
    .await;

    let fs = Arc::new(CountingFs::new(disk.clone()));
    let dm = FaultDocManager::new("no-poison-here");
    let mut controller = build_with_fs(fs.clone(), dm.clone());

    // Tick 1: the owner ingests and claims the id; the dupe is read, refused,
    // and disclosed by exactly one loud ERROR.
    controller.poll_new_files().await.expect("tick 1");
    assert_eq!(
        dm.pages_titled("a_owner").len(),
        1,
        "the first file to present the id owns it"
    );
    assert!(
        dm.pages_titled("z_dupe").is_empty(),
        "the duplicate must NOT be ingested — its blocks would merge into the claimant"
    );
    assert_eq!(
        duplicate_id_errors(&cap.errors()).len(),
        1,
        "the refusal is disclosed loudly ONCE; got {:?}",
        cap.errors()
    );
    let reads_after_refusal = fs.reads_of("z_dupe.org");
    assert!(
        reads_after_refusal >= 1,
        "deciding the refusal requires reading the file once"
    );

    // Tick 2: nothing changed on disk, so re-deciding cannot change the outcome.
    // No read, no parse, no new disclosure — the storm-gate.
    controller.poll_new_files().await.expect("tick 2");
    assert_eq!(
        fs.reads_of("z_dupe.org"),
        reads_after_refusal,
        "a refused file whose bytes have not changed must NOT be re-read on the next \
         discovery tick — the refusal is permanent until the file changes"
    );
    assert_eq!(
        duplicate_id_errors(&cap.errors()).len(),
        1,
        "no re-disclosure for an unchanged refused file"
    );

    // The user fixes it: a fresh id. The signature changes, so the file is
    // re-attempted and now ingests.
    write_file(
        &disk,
        "z_dupe.org",
        "#+TITLE: Z Dupe\n#+ID: 8b2d0e41-77aa-4c33-a1d5-99e0f1c2b3a4\n",
    )
    .await;
    controller.poll_new_files().await.expect("tick 3");
    assert!(
        fs.reads_of("z_dupe.org") > reads_after_refusal,
        "an edited file must be re-attempted"
    );
    assert_eq!(
        dm.pages_titled("z_dupe").len(),
        1,
        "with a fresh id the formerly-refused file ingests"
    );
}

/// The refusal stays loud once per CONTENT VERSION, not once per session: a
/// user who edits the file without fixing the collision gets told again, and
/// the gate re-arms at the new bytes.
#[tokio::test]
async fn an_edited_still_duplicate_file_is_re_disclosed_then_gated_again() {
    let cap = ErrorCapture::default();
    let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let disk = Arc::new(InMemoryFileSystem::new());
    disk.mkdir_all(std::path::Path::new(ROOT));
    write_file(
        &disk,
        "a_owner.org",
        &format!("#+TITLE: A Owner\n#+ID: {SHARED_ID}\n"),
    )
    .await;
    write_file(
        &disk,
        "z_dupe.org",
        &format!("#+TITLE: Z Dupe\n#+ID: {SHARED_ID}\n"),
    )
    .await;

    let fs = Arc::new(CountingFs::new(disk.clone()));
    let dm = FaultDocManager::new("no-poison-here");
    let mut controller = build_with_fs(fs.clone(), dm.clone());

    controller.poll_new_files().await.expect("tick 1");
    assert_eq!(duplicate_id_errors(&cap.errors()).len(), 1);

    // Edited, but the id still collides.
    write_file(
        &disk,
        "z_dupe.org",
        &format!("#+TITLE: Z Dupe\n#+ID: {SHARED_ID}\n\n* an edit that does not fix the id\n"),
    )
    .await;
    controller.poll_new_files().await.expect("tick 2");
    assert_eq!(
        duplicate_id_errors(&cap.errors()).len(),
        2,
        "a NEW version of a still-colliding file is disclosed loudly again"
    );

    let reads = fs.reads_of("z_dupe.org");
    controller.poll_new_files().await.expect("tick 3");
    assert_eq!(
        fs.reads_of("z_dupe.org"),
        reads,
        "the gate re-arms at the new bytes"
    );
    assert_eq!(duplicate_id_errors(&cap.errors()).len(), 2);
}

/// The refusal is NOT a property of the refused file's bytes: it holds only
/// while ANOTHER file on disk claims the id. `disclose_duplicate_doc_id` tells
/// the user the winner is whichever file the session happened to ingest first,
/// which invites them to delete the stray copy that won — so deleting the
/// claimant must hand the id over. Nothing about the refused file changes when
/// that happens, so a gate keyed on its signature alone would make the remedy a
/// silent dead end for the rest of the session.
#[tokio::test]
async fn refusal_lifts_when_the_claimant_leaves_disk() {
    let cap = ErrorCapture::default();
    let _g = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let disk = Arc::new(InMemoryFileSystem::new());
    disk.mkdir_all(std::path::Path::new(ROOT));
    write_file(
        &disk,
        "a_owner.org",
        &format!("#+TITLE: A Owner\n#+ID: {SHARED_ID}\n"),
    )
    .await;
    write_file(
        &disk,
        "z_dupe.org",
        &format!("#+TITLE: Z Dupe\n#+ID: {SHARED_ID}\n"),
    )
    .await;

    let fs = Arc::new(CountingFs::new(disk.clone()));
    let dm = FaultDocManager::new("no-poison-here");
    let mut controller = build_with_fs(fs.clone(), dm.clone());

    controller.poll_new_files().await.expect("tick 1");
    let refused_reads = fs.reads_of("z_dupe.org");
    assert_eq!(duplicate_id_errors(&cap.errors()).len(), 1);

    // The user resolves the collision the way the ERROR suggests: delete the
    // copy that won. The refused file itself is untouched — same mtime, same
    // size — so only the claimant's absence can lift the gate.
    disk.remove_file(&std::path::Path::new(ROOT).join("a_owner.org"))
        .unwrap();

    controller.poll_new_files().await.expect("tick 2");
    assert!(
        fs.reads_of("z_dupe.org") > refused_reads,
        "with the claimant gone the id is free: the refused file must be re-read and \
         adopted, not gated on its unchanged bytes for the rest of the session"
    );

    // Adopted means TRACKED: the next tick no longer rediscovers it, which is
    // what separates a real adoption from a re-read that refuses again.
    let adopted_reads = fs.reads_of("z_dupe.org");
    controller.poll_new_files().await.expect("tick 3");
    assert_eq!(
        fs.reads_of("z_dupe.org"),
        adopted_reads,
        "an adopted file is tracked, so the discovery walk leaves it alone"
    );
    assert_eq!(
        duplicate_id_errors(&cap.errors()).len(),
        1,
        "adopting the id is not a new fault to report"
    );
}
