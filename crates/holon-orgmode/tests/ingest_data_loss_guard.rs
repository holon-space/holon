//! The ingest write-back guard must still refuse a GENUINELY lossy ingest.
//!
//! The guard grounds an absent block against the AUTHORITY (which file owns it
//! now), not against the file's own projection. That is what lets a legitimate
//! de-inline through. This pins the other half: when a block the file carries
//! never lands in the store — the create-txn FK-rollback shape of BugFunnel row
//! 28 — the authority holds nothing, nothing grounds it, and `on_file_changed`
//! MUST return `Err` so the caller quarantines the file instead of rendering
//! the truncated projection over the user's lines.
//!
//! The pair is what convicts: the SAME file through the SAME controller
//! ingests cleanly once the store stops dropping the block.
//!
//! @pbt kind harness
//! @pbt covers ingest-data-loss-still-refused — a block that never landed in
//!   the store still refuses + quarantines the write-back

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

/// The child page whose store write is swallowed. `:Page:`-tagged on purpose:
/// a page child is EXCLUDED from the post-ingest doc-walk gate (the walk stops
/// at page boundaries) and is legitimately absent from the host's render, so
/// the write-back guard is the ONLY thing standing between a lost child page
/// and its lines being deleted from the host file.
const SWALLOWED: &str = "middle-page";

const SOURCE: &str = "\
#+ID: lossy-doc
#+TITLE: Lossy Doc
* First
:PROPERTIES:
:ID: first-block
:END:
first body
* Middle :Page:
:PROPERTIES:
:ID: middle-page
:END:
middle body that must never be silently deleted
* Last
:PROPERTIES:
:ID: last-block
:END:
last body
";

/// A store that can SWALLOW one block id on write — the observable shape of a
/// create-txn that FK-rolled back without surfacing an error.
///
/// The swallowed id is still reported by `children()`, so the two upstream
/// landing gates (ordering visibility, post-ingest doc walk) are SATISFIED and
/// the write-back guard is what the test actually exercises. Without that, the
/// gates fire first and the guard is never reached.
#[derive(Clone, Default)]
struct FakeStore {
    blocks: Arc<Mutex<HashMap<String, StorageEntity>>>,
    docs: Arc<Mutex<HashMap<EntityUri, Block>>>,
    swallow: Option<(EntityUri, EntityUri)>,
    /// What production stamps through `persist_file_hash`, replayed by
    /// `load_file_hashes` so a later controller boots the cold-boot fast path
    /// on the hash an earlier one actually wrote.
    hashes: Arc<Mutex<Vec<(EntityUri, String)>>>,
}

impl FakeStore {
    /// `id` is swallowed on write; `parent` is where `children()` keeps
    /// pretending it landed.
    fn swallowing(id: &str, parent: &str) -> Self {
        Self {
            swallow: Some((EntityUri::block(id), EntityUri::block(parent))),
            ..Self::default()
        }
    }

    fn swallowed_id(&self) -> Option<&EntityUri> {
        self.swallow.as_ref().map(|(id, _)| id)
    }

    /// Whether a block row with this bare id is in the store.
    fn holds_block(&self, bare: &str) -> bool {
        self.blocks
            .lock()
            .unwrap()
            .contains_key(&EntityUri::block(bare).to_string())
    }

    /// A doc-root can live in either half of this store — as a block row once
    /// ingest has written it, or only in the page map — so both are consulted,
    /// blocks first, exactly as `get_by_id` resolves it.
    fn block_content(&self, bare: &str) -> Option<String> {
        let id = EntityUri::block(bare);
        let row = self.blocks.lock().unwrap().get(&id.to_string()).cloned();
        if let Some(row) = row {
            return Some(row_to_block(&row).content.clone());
        }
        self.docs
            .lock()
            .unwrap()
            .get(&id)
            .map(|d| d.content.clone())
    }

    /// Empty a doc-root's content, standing in for the broken convert/delete
    /// that leaves one title-less.
    fn set_block_content(&self, bare: &str, content: &str) {
        let id = EntityUri::block(bare);
        {
            let mut rows = self.blocks.lock().unwrap();
            if let Some(row) = rows.get_mut(&id.to_string()) {
                row.insert("content".into(), holon_api::Value::from(content));
                return;
            }
        }
        let mut docs = self.docs.lock().unwrap();
        let doc = docs
            .get_mut(&id)
            .expect("the doc-root must exist before its content is emptied");
        doc.content = content.to_string();
    }

    fn stamped_hashes(&self) -> usize {
        self.hashes.lock().unwrap().len()
    }

    /// Drop every block and page row, keeping the persisted content hashes —
    /// a DB that has moved on while the hashes an earlier session stamped
    /// remain.
    fn clear_rows(&self) {
        self.blocks.lock().unwrap().clear();
        self.docs.lock().unwrap().clear();
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
        let mut ids: Vec<EntityUri> = self
            .blocks
            .lock()
            .unwrap()
            .values()
            .map(row_to_block)
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id)
            .collect();
        if let Some((id, parent)) = &self.swallow {
            if parent == parent_id {
                ids.push(id.clone());
            }
        }
        Ok(ids)
    }
    async fn update_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        if self.swallowed_id().map(|u| u.to_string()) == Some(id.clone()) {
            return Ok(());
        }
        self.blocks.lock().unwrap().insert(id, params);
        Ok(())
    }
    async fn delete_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
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
    /// Page-only, and DERIVED from the block rows first — mirroring
    /// `LiveDocumentManager`, whose `WHERE tag='Page'` matview sits over the
    /// same `block_raw` the writes land in. A page store fed only by `create`
    /// would never know about a `:Page:` child this very ingest wrote, so every
    /// child page would resolve to no file and the de-inline topology these
    /// tests are about could not exist.
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let row = self.blocks.lock().unwrap().get(&id.to_string()).cloned();
        if let Some(row) = row {
            let block = row_to_block(&row);
            if block.is_page() {
                return Ok(Some(block));
            }
        }
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

#[async_trait]
impl BlockReader for FakeStore {
    /// Stops at `Page` boundaries, like the real doc walk: a page-tagged child
    /// and everything under it belongs to that page's own document, never to
    /// this one. Without this the render would keep inlining child pages and no
    /// de-inline absence — the whole subject of these tests — would exist.
    async fn get_blocks(&self, document_uri: &EntityUri) -> anyhow::Result<Vec<Block>> {
        let rows: Vec<Block> = self
            .blocks
            .lock()
            .unwrap()
            .values()
            .filter(|row| row_doc_uri(row) == *document_uri)
            .map(row_to_block)
            .collect();
        let docs = self.docs.lock().unwrap();
        let pages: std::collections::HashSet<EntityUri> = rows
            .iter()
            .filter(|b| b.is_page())
            .map(|b| b.id.clone())
            .chain(docs.values().filter(|d| d.is_page()).map(|d| d.id.clone()))
            .collect();
        let parent_of: HashMap<EntityUri, EntityUri> = rows
            .iter()
            .map(|b| (b.id.clone(), b.parent_id.clone()))
            .collect();
        Ok(rows
            .iter()
            .filter(|b| b.id != *document_uri)
            .filter(|b| {
                let mut cursor = b.id.clone();
                loop {
                    if cursor != *document_uri && pages.contains(&cursor) {
                        return false;
                    }
                    match parent_of.get(&cursor) {
                        Some(parent) if *parent != cursor => cursor = parent.clone(),
                        _ => return true,
                    }
                }
            })
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
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .get(&id.to_string())
            .map(row_to_block))
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }

    async fn load_file_hashes(&self) -> anyhow::Result<Vec<(EntityUri, String)>> {
        Ok(self.hashes.lock().unwrap().clone())
    }

    async fn persist_file_hash(&self, uri: &EntityUri, hash: &str) -> anyhow::Result<()> {
        let mut hashes = self.hashes.lock().unwrap();
        hashes.retain(|(u, _)| u != uri);
        hashes.push((uri.clone(), hash.to_string()));
        Ok(())
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

/// Restores the row's `tags`, so a block written as a `Page` reads back as one.
/// Page-ness is what the doc walk and the ancestor walk both key on; dropping
/// it here would make every store round trip silently demote child pages.
fn row_to_block(row: &StorageEntity) -> Block {
    let mut block = Block::new_text(
        row_uri(row_field(row, "id")),
        row_uri(row_field(row, "parent_id")),
        row_field(row, "content"),
    );
    if let Some(holon_api::Value::Array(tags)) = row.get("tags") {
        block.tags = tags
            .iter()
            .filter_map(|t| t.as_string().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .into();
    }
    block
}

/// Ingest `source` once through the real controller over `store`, and hand back
/// the outcome plus the bytes still on disk. `preexisting` seeds vault files
/// the ingest does not write — the sibling whose bytes the grounding may
/// consult.
async fn ingest_file(
    store: FakeStore,
    source: &str,
    preexisting: &[(&str, &str)],
) -> (anyhow::Result<()>, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Lossy Doc.org");
    std::fs::write(&path, source).unwrap();
    for (rel, content) in preexisting {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
    }

    let mut controller = new_org_sync_controller(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        root,
        Arc::new(store.clone()),
        Arc::new(RealFileSystem),
    );
    let result = controller.on_file_changed(&path).await.map(|_| ());
    let on_disk = std::fs::read_to_string(&path).unwrap();
    (result, on_disk)
}

async fn ingest(store: FakeStore) -> (anyhow::Result<()>, String) {
    ingest_file(store, SOURCE, &[]).await
}

#[tokio::test]
async fn a_block_that_never_landed_refuses_the_write_back() {
    let (result, on_disk) = ingest(FakeStore::swallowing(SWALLOWED, "lossy-doc")).await;
    let err = result.expect_err("an ingest that lost a block must refuse the write-back");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("INGEST DATA LOSS") && msg.contains(SWALLOWED),
        "the refusal must be loud and name the lost block; got: {msg}"
    );
    assert!(
        !msg.contains("UNRESOLVABLE"),
        "a plain loss must not be dressed up as a topology failure; got: {msg}"
    );
    assert!(
        on_disk.contains("middle body that must never be silently deleted"),
        "the user's lines must survive on disk; got:\n{on_disk}"
    );
}

/// The twin that convicts the store, not the fixture: the SAME file through the
/// SAME controller ingests cleanly once nothing is swallowed — and the child
/// page is de-inlined out of the host, which is the relocation the guard now
/// recognises rather than the loss it used to report.
#[tokio::test]
async fn the_same_file_ingests_cleanly_when_nothing_is_swallowed() {
    let (result, on_disk) = ingest(FakeStore::default()).await;
    result.expect("a lossless ingest of the same file must succeed");
    assert!(
        !on_disk.contains("middle body that must never be silently deleted"),
        "the child page must be de-inlined out of the host file; got:\n{on_disk}"
    );
    assert!(
        on_disk.contains("first body") && on_disk.contains("last body"),
        "the host's own blocks must survive the de-inline; got:\n{on_disk}"
    );
}

// ---------------------------------------------------------------------------
// A sibling file's bytes must not rescue a block the AUTHORITY lost.
//
// `Sub` is a child page that legitimately de-inlines into `Lossy Doc/Sub.org`,
// so the grounding reads that sibling's bytes into the surviving union. Those
// bytes are STALE: they still carry `stranded`, a block whose store write this
// ingest swallowed. Bytes written before the loss prove nothing about what the
// store holds now, so the write must still be refused.
// ---------------------------------------------------------------------------

const STRANDED: &str = "stranded";

const SOURCE_WITH_CHILD_PAGE: &str = "\
#+ID: lossy-doc
#+TITLE: Lossy Doc
* First
:PROPERTIES:
:ID: first-block
:END:
first body
* Sub :Page:
:PROPERTIES:
:ID: sub-page
:END:
sub body
** Stranded
:PROPERTIES:
:ID: stranded
:END:
stranded body only the stale sibling still mentions
";

/// The sibling as a PREVIOUS session left it: it still carries `stranded`.
const STALE_SIBLING: &str = "\
#+ID: sub-page
#+TITLE: Sub
* Stranded
:PROPERTIES:
:ID: stranded
:END:
stranded body only the stale sibling still mentions
";

#[tokio::test]
async fn a_siblings_stale_bytes_do_not_rescue_a_block_the_authority_lost() {
    let (result, on_disk) = ingest_file(
        FakeStore::swallowing(STRANDED, "sub-page"),
        SOURCE_WITH_CHILD_PAGE,
        &[("Lossy Doc/Sub.org", STALE_SIBLING)],
    )
    .await;
    let err = result.expect_err(
        "a block the authority lost must be refused even though a sibling file still names it",
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains(STRANDED) && msg.contains("the authority no longer holds this block"),
        "the refusal must name the lost block and why nothing grounds it; got: {msg}"
    );
    assert!(
        on_disk.contains("stranded body only the stale sibling still mentions"),
        "the user's lines must survive on disk; got:\n{on_disk}"
    );
}

// ---------------------------------------------------------------------------
// A prohibited topology is disclosed AS a topology failure, not as data loss.
// `Nested` is a `:Page:` under a PLAIN heading, so `name_chain` fails loud and
// the block's own-file path cannot be derived. The block is in BOTH the drop
// set and the unresolvable set; leading with the loss headline would count it
// twice and point the reader at a truncated ingest that never happened.
// ---------------------------------------------------------------------------

const SOURCE_PAGE_UNDER_PLAIN: &str = "\
#+ID: lossy-doc
#+TITLE: Lossy Doc
* Plain Heading
:PROPERTIES:
:ID: plain-heading
:END:
plain body
** Nested :Page:
:PROPERTIES:
:ID: nested-page
:END:
nested page body
";

#[tokio::test]
async fn a_prohibited_topology_is_refused_as_unresolvable_not_as_data_loss() {
    let (result, on_disk) = ingest_file(FakeStore::default(), SOURCE_PAGE_UNDER_PLAIN, &[]).await;
    let err = result.expect_err("a page under a non-page owns no derivable file — refuse");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("UNRESOLVABLE INGEST DROP") && msg.contains("nested-page"),
        "the refusal must lead with the prohibited topology and name the block; got: {msg}"
    );
    assert!(
        !msg.contains("INGEST DATA LOSS"),
        "the topology failure must not also be reported as data loss (one block, one count); got: \
         {msg}"
    );
    assert!(
        !msg.contains("Dropped blocks:"),
        "an empty/duplicate drop section must not be printed on this path; got: {msg}"
    );
    assert!(
        on_disk.contains("nested page body"),
        "the user's lines must survive on disk; got:\n{on_disk}"
    );
}

// ---------------------------------------------------------------------------
// D40: a SECOND file presenting an `#+ID:` a live file already claims.
//
// `#+ID:` is the authoritative document identity, so two files carrying one id
// resolve to one document node and the later file's blocks merge into it —
// silently collapsing two vault files into one document. Ingest must refuse the
// later file instead, and say which two paths collide.
// ---------------------------------------------------------------------------

/// Every test below builds its OWN doc id and its own tempdir, and asserts on
/// tempdir-qualified paths. The capture sink is process-wide, so a needle any
/// sibling could also emit would let one test pass on another's output — a
/// green that proves nothing.
fn claimant_src(id: &str) -> String {
    format!(
        "#+ID: {id}\n#+TITLE: Original\n* Original heading\n:PROPERTIES:\n:ID: \
         {id}-original\n:END:\noriginal body\n"
    )
}

fn stray_src(id: &str) -> String {
    format!(
        "#+ID: {id}\n#+TITLE: Impostor\n* Impostor heading\n:PROPERTIES:\n:ID: \
         {id}-impostor\n:END:\nimpostor body that must not merge into the original \
         document\n"
    )
}

/// Only the captured lines that name THIS test's vault root. Both directions
/// die: a sibling's line cannot contain this tempdir, and this test's negative
/// assertions can no longer be tripped by a sibling's.
fn lines_under(captured: &str, root: &std::path::Path) -> String {
    let root = root.display().to_string();
    captured
        .lines()
        .filter(|line| line.contains(&root))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collects ERROR-level tracing for one span, so a refusal can be asserted on
/// the surface an operator actually sees.
#[derive(Clone, Default)]
struct CapturedErrors(Arc<Mutex<Vec<u8>>>);

impl CapturedErrors {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }

    fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
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

/// Serializes the capture window. The sink below is process-wide, so two
/// tests reading it at once would each see the other's events.
static CAPTURE: Mutex<()> = Mutex::new(());
static SINK: std::sync::OnceLock<CapturedErrors> = std::sync::OnceLock::new();

/// One ERROR-level subscriber for the whole binary, installed GLOBALLY rather
/// than per-thread: the test harness may drive a test's future on a thread
/// other than the one that installed a thread-local default, and events
/// emitted there would be lost.
fn sink() -> &'static CapturedErrors {
    SINK.get_or_init(|| {
        let errors = CapturedErrors::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(errors.clone())
            .with_max_level(tracing::Level::ERROR)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("this binary installs exactly one global subscriber");
        errors
    })
}

/// Run `f` with ERROR-level tracing captured.
async fn capturing_errors<F, T>(f: F) -> (T, String)
where
    F: std::future::Future<Output = T>,
{
    // A panicking test poisons this; the lock orders captures, it guards no
    // invariant, so an earlier panic must not cascade into every later test.
    let _serialized = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    let errors = sink();
    errors.clear();
    let out = f.await;
    (out, errors.text())
}

#[tokio::test]
async fn a_second_file_claiming_a_live_id_is_refused_not_merged() {
    let id = "dup-merge-id";
    let store = FakeStore::default();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let original = root.join("Original.org");
    let second = root.join("Impostor.org");
    std::fs::write(&original, claimant_src(id)).unwrap();
    std::fs::write(&second, stray_src(id)).unwrap();

    let mut controller = controller_over(&store, &root);
    controller
        .on_file_changed(&original)
        .await
        .expect("the first file to claim the id ingests normally");
    assert!(
        store.holds_block(&format!("{id}-original")),
        "precondition: the first claimant's blocks must have landed"
    );

    let (result, captured) = capturing_errors(controller.on_file_changed(&second)).await;
    result.expect("a duplicate-id file must be SKIPPED, not fail the whole ingest loop");
    let disclosed = lines_under(&captured, &root);

    assert!(
        !store.holds_block(&format!("{id}-impostor")),
        "the second claimant's blocks MERGED into the first file's document — two vault \
         files collapsed into one document node"
    );
    for needle in [
        original.display().to_string(),
        second.display().to_string(),
        id.to_string(),
    ] {
        assert!(
            disclosed.contains(&needle),
            "the refusal must name both colliding paths and the shared id; {needle:?} missing \
             from:\n{disclosed}"
        );
    }

    // The SAME file re-ingested is an ordinary re-ingest, never a collision.
    let (again, quiet) = capturing_errors(controller.on_file_changed(&original)).await;
    again.expect("re-ingesting the file that owns the id must not be refused");
    assert!(
        store.holds_block(&format!("{id}-original")),
        "the owner's blocks must survive its own re-ingest"
    );
    assert!(
        !lines_under(&quiet, &root).contains(id),
        "re-ingesting the id's OWN file must not be reported as a collision; got:\n{}",
        lines_under(&quiet, &root)
    );
}

/// Build a controller over `store` rooted at `root`.
fn controller_over(
    store: &FakeStore,
    root: &std::path::Path,
) -> holon_filesystem::FileSyncController {
    new_org_sync_controller(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        root.to_path_buf(),
        Arc::new(store.clone()),
        Arc::new(RealFileSystem),
    )
}

/// The cold-boot fast path is the OTHER door into a home/alias record: an
/// unchanged file whose stored hash matches skips identity resolution
/// entirely. A stray must be refused there too — and the file that legitimately
/// owns the id must stay writable afterwards, which is the half that loses user
/// edits when the stray wins the home.
#[tokio::test]
async fn a_cold_boot_fast_path_refuses_a_stray_and_keeps_the_claimant_writable() {
    let id = "dup-boot-id";
    let store = FakeStore::default();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let original = root.join("Original.org");
    let second = root.join("Impostor.org");
    std::fs::write(&original, claimant_src(id)).unwrap();
    std::fs::write(&second, stray_src(id)).unwrap();

    // The shape the vault's existing duplicate pairs are in, built the way it
    // actually arose: a past session ingested the stray and stamped its
    // content hash, from before any duplicate-id refusal existed.
    {
        let mut past = controller_over(&store, &root);
        past.on_file_changed(&second)
            .await
            .expect("a past session ingests the stray it found");
    }
    // The DB has since moved on to reflect the claimant. The stray's rows are
    // gone; its persisted hash is not, and that stale hash is what lets it
    // take the byte-identity door on the next boot.
    store.clear_rows();
    {
        let mut past = controller_over(&store, &root);
        past.on_file_changed(&original)
            .await
            .expect("the claimant ingests");
    }
    assert!(
        store.stamped_hashes() >= 2,
        "precondition: both files must carry a persisted content hash to boot from"
    );

    let mut boot = controller_over(&store, &root);
    boot.initialize().await.expect("cold boot");
    boot.on_file_changed(&original)
        .await
        .expect("the claimant ingests");

    let (refused, captured) = capturing_errors(boot.on_file_changed(&second)).await;
    refused.expect("a stray must be skipped, not fail the boot scan");
    let disclosed = lines_under(&captured, &root);
    for needle in [
        original.display().to_string(),
        second.display().to_string(),
        id.to_string(),
    ] {
        assert!(
            disclosed.contains(&needle),
            "the fast path let a stray through unreported; {needle:?} missing from:\n{disclosed}"
        );
    }

    // The half that costs a user their work: the file that OWNS the id must
    // still ingest. If the stray took the home, this edit is refused instead.
    std::fs::write(
        &original,
        format!(
            "{}* Later\n:PROPERTIES:\n:ID: {id}-later\n:END:\nlater body\n",
            claimant_src(id)
        ),
    )
    .unwrap();
    boot.on_file_changed(&original)
        .await
        .expect("an edit to the id's own file must ingest");
    assert!(
        store.holds_block(&format!("{id}-later")),
        "the LEGITIMATE file's later edit was dropped — the stray had taken its home record"
    );
}

/// The pre-ingest heal resolves a doc by `#+ID:` and re-derives its title and
/// parent from the FILE's path. Reached from a stray, that rewrites the
/// claimant's document to the stray's name chain — a store write by a file that
/// is about to be refused.
#[tokio::test]
async fn a_stray_does_not_retitle_the_claimants_document() {
    let id = "dup-heal-id";
    let store = FakeStore::default();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let original = root.join("Original.org");
    let second = root.join("Impostor.org");
    std::fs::write(&original, claimant_src(id)).unwrap();
    std::fs::write(&second, stray_src(id)).unwrap();

    let mut controller = controller_over(&store, &root);
    controller
        .on_file_changed(&original)
        .await
        .expect("the claimant ingests and takes the id");

    // A broken convert/delete leaves the doc-root title-less — the one state
    // the heal acts on.
    store.set_block_content(id, "");

    controller
        .on_file_changed(&second)
        .await
        .expect("a stray must be skipped, not fail the sync loop");

    assert_eq!(
        store.block_content(id).as_deref(),
        Some(""),
        "the stray retitled the CLAIMANT's document from its own filename before being refused"
    );
}
