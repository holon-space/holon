//! Fork B B1 §3.1 Finding A / R11 (BLOCKING): the no-pages-under-non-pages
//! `name_chain` assertion must be **observably fail-loud**, not silently
//! swallowed by `doc_id_to_path`'s callers.
//!
//! Before the fix, `doc_id_to_path` did `Err(_) => None`, collapsing the new
//! assertion into the SAME silent-skip bucket as "legitimately not a page". At
//! all three call sites `None` was an unlogged skip — so a live edit to a
//! prohibited-topology page would silently never write to disk (worse than
//! today, where no prohibition is enforced and the edit writes fine).
//!
//! These tests pin the required behavior at the `FileSyncController` boundary:
//!   (i)   the write does NOT silently no-op;
//!   (ii)  an ERROR-level tracing event fires, naming the offending doc;
//!   (iii) the boot sweep continues processing OTHER documents afterward
//!         (bounded blast radius — never crash the whole sync loop).

#![cfg(feature = "di")]

use std::collections::HashMap;
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
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

// ---------------------------------------------------------------------------
// ERROR-level tracing capture (dependency-free of any test-log crate).
// ---------------------------------------------------------------------------

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
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            let mut buf = String::new();
            event.record(&mut MsgVisitor(&mut buf));
            self.0.lock().unwrap().push(buf);
        }
    }
}

// ---------------------------------------------------------------------------
// Test doubles.
// ---------------------------------------------------------------------------

/// Block reader that serves a whole fixture set and can enumerate a chosen list
/// of `(doc_id, blocks)` for the boot sweep.
struct FixtureReader {
    by_id: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<Block>>,
    documents: Vec<EntityUri>,
}

#[async_trait]
impl BlockReader for FixtureReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self.children.get(doc_id).cloned().unwrap_or_default())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.by_id.get(id).cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(self
            .documents
            .iter()
            .map(|d| (d.clone(), self.children.get(d).cloned().unwrap_or_default()))
            .collect())
    }
}

/// Document manager that uses the DEFAULT `name_chain` — so the real
/// no-pages-under-non-pages assertion runs against whatever topology we seed.
struct FixtureDocManager {
    by_id: HashMap<EntityUri, Block>,
}

#[async_trait]
impl DocumentManager for FixtureDocManager {
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
        Ok(self.by_id.get(id).cloned())
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

fn page(id: &str, parent: EntityUri, title: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent, title.to_string());
    b.set_page(true);
    b
}

fn non_page(id: &str, parent: EntityUri, content: &str) -> Block {
    Block::new_text(EntityUri::block(id), parent, content.to_string())
}

/// Fixture: one LEGIT top-level page `legit` (materializes to `legit.org`) and
/// one PROHIBITED page `prohibited` parented to a non-page `b1` parented to a
/// page `ancestor` — `name_chain(prohibited)` fails loud.
struct Fixtures {
    by_id: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<Block>>,
    legit_id: EntityUri,
    prohibited_id: EntityUri,
}

fn fixtures() -> Fixtures {
    let ancestor = page("ancestor", EntityUri::no_parent(), "Ancestor");
    let b1 = non_page("b1", ancestor.id.clone(), "a plain heading (non-page)");
    let prohibited = page("prohibited", b1.id.clone(), "Prohibited Page");
    let legit = page("legit", EntityUri::no_parent(), "Legit Page");

    // Each page has one leaf child so the sweep renders non-empty content.
    let legit_child = non_page("legit-child", legit.id.clone(), "legit body");
    let prohibited_child = non_page("prohibited-child", prohibited.id.clone(), "prohibited body");

    let mut by_id = HashMap::new();
    for b in [
        &ancestor,
        &b1,
        &prohibited,
        &legit,
        &legit_child,
        &prohibited_child,
    ] {
        by_id.insert(b.id.clone(), b.clone());
    }

    let mut children = HashMap::new();
    children.insert(legit.id.clone(), vec![legit_child]);
    children.insert(prohibited.id.clone(), vec![prohibited_child]);

    Fixtures {
        by_id,
        children,
        legit_id: legit.id.clone(),
        prohibited_id: prohibited.id.clone(),
    }
}

fn build_controller(
    f: &Fixtures,
    documents: Vec<EntityUri>,
    root: std::path::PathBuf,
) -> holon_filesystem::FileSyncController {
    let reader = Arc::new(FixtureReader {
        by_id: f.by_id.clone(),
        children: f.children.clone(),
        documents,
    });
    let doc_manager = Arc::new(FixtureDocManager {
        by_id: f.by_id.clone(),
    });
    new_org_sync_controller(
        reader,
        doc_manager,
        root,
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    )
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// on_block_changed on a prohibited-topology page: MUST NOT silently write,
/// MUST emit an ERROR naming the doc, and returns `Ok(false)` (skip, not
/// crash).
#[tokio::test]
async fn on_block_changed_prohibited_topology_logs_error_and_skips_write() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut controller = build_controller(&f, vec![], root.clone());

    let prohibited_block = f.by_id[&f.prohibited_id].clone();
    let result = controller
        .on_block_changed(&f.prohibited_id, &BlockDelta::Upsert(prohibited_block))
        .await
        .expect(
            "a prohibited topology must be a bounded skip, never propagate an Err that crashes \
             the loop",
        );

    // (i) does not silently write: returns Ok(false) and NO file exists.
    assert!(
        !result,
        "on_block_changed must report 'no file written' (Ok(false)) for a prohibited topology"
    );
    let mut wrote_any = false;
    for entry in walkdir(&root) {
        if entry.extension().and_then(|e| e.to_str()) == Some("org") {
            wrote_any = true;
        }
    }
    assert!(
        !wrote_any,
        "a prohibited-topology edit must NOT write any .org file (silent-write regression)"
    );

    // (ii) an ERROR-level event fired naming the offending doc.
    let errors = cap.errors();
    assert!(
        errors.iter().any(|e| e.contains("prohibited")),
        "expected an ERROR-level tracing event naming the offending doc; captured: {errors:?}"
    );
}

/// The boot sweep hits a prohibited page mid-list: it MUST log the error, skip
/// only that document, and still materialize the legit page's file.
#[tokio::test]
async fn sweep_skips_prohibited_doc_but_materializes_the_legit_one() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    // Prohibited doc FIRST so we prove the loop continues past the error.
    let mut controller = build_controller(
        &f,
        vec![f.prohibited_id.clone(), f.legit_id.clone()],
        root.clone(),
    );

    controller.materialize_missing_page_files().await.expect(
        "the sweep must be a bounded skip, never propagate an Err that aborts the whole sweep",
    );

    // (iii) loop continued: the legit page got its file even though a prior doc
    // errored.
    let legit_path = root.join("Legit Page.org");
    assert!(
        legit_path.exists(),
        "legit page must still materialize despite an earlier prohibited-topology doc; dir \
         contents: {:?}",
        walkdir(&root)
    );
    // The prohibited page must NOT have produced a file.
    let prohibited_path = root.join("Prohibited Page.org");
    assert!(
        !prohibited_path.exists(),
        "prohibited-topology page must not materialize a file"
    );

    // (ii) the skip was observable, not silent.
    let errors = cap.errors();
    assert!(
        errors.iter().any(|e| e.contains("prohibited")),
        "expected an ERROR-level tracing event for the skipped prohibited doc; captured: \
         {errors:?}"
    );
}

/// Minimal recursive directory walk (dev-only; avoids a walkdir dep here).
fn walkdir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
