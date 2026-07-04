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
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

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
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
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

/// TRULY PROD-FAITHFUL doc manager: `get_by_id` returns a block ONLY if it is a
/// `Page`, mirroring `LiveDocumentManager`'s `WHERE tag='Page'` matview. A
/// non-page content block is invisible to the page store (`None`), exactly as
/// in prod — the shape `FixtureDocManager` (which serves any seeded block) does
/// not reproduce.
struct PageOnlyDocManager {
    by_id: HashMap<EntityUri, Block>,
}

#[async_trait]
impl DocumentManager for PageOnlyDocManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.by_id.get(id).filter(|b| b.is_page()).cloned())
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

/// Row-23/29 write-back destruction fixture. A page `container`
/// (→ `Container.org`) whose subtree contains a PROHIBITED page (`prohibited`
/// parented to the non-page `b1`), so `name_chain(prohibited)` fails loud — the
/// exact real-vault first-boot shape where a re-homed `* Holon` subtree landed
/// page-under-non-page. `truncate` picks what `get_blocks(container)` returns:
/// the FULL subtree (used to seed `Container.org` on disk) or LEAVES-ONLY (the
/// re-homed projection that drops the prohibited subtree). `by_id` is always
/// the full topology, so the REAL `name_chain` assertion runs against it.
fn container_fixtures(truncate: bool) -> Fixtures {
    let container = page("container", EntityUri::no_parent(), "Container");
    let leaf1 = non_page("leaf1", container.id.clone(), "leaf one body");
    let leaf2 = non_page("leaf2", container.id.clone(), "leaf two body");
    let b1 = non_page("b1", container.id.clone(), "a plain section (non-page)");
    let prohibited = page("prohibited", b1.id.clone(), "Prohibited Page");
    let prohibited_child = non_page("prohibited-child", prohibited.id.clone(), "prohibited body");

    let mut by_id = HashMap::new();
    for b in [
        &container,
        &leaf1,
        &leaf2,
        &b1,
        &prohibited,
        &prohibited_child,
    ] {
        by_id.insert(b.id.clone(), b.clone());
    }

    // `get_blocks(container)` returns the doc's blocks FLAT (the renderer rebuilds
    // the tree from parent_id). Full = seed; truncated = the drop that de-inlined
    // the prohibited subtree.
    let doc_blocks = if truncate {
        vec![leaf1, leaf2]
    } else {
        vec![leaf1, leaf2, b1, prohibited.clone(), prohibited_child]
    };
    let mut children = HashMap::new();
    children.insert(container.id.clone(), doc_blocks);

    Fixtures {
        by_id,
        children,
        legit_id: container.id.clone(),
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

/// Previous sibling of `block` in the authoritative sequence: the last block
/// before it that shares its parent, `None` if it is first in its group.
fn prev_sibling(blocks: &[Block], block: &Block) -> Option<EntityUri> {
    blocks
        .iter()
        .take_while(|b| b.id != block.id)
        .filter(|b| b.parent_id == block.parent_id)
        .last()
        .map(|b| b.id.clone())
}

/// Drive one block edit the way production does: the holder is seeded from the
/// authority (production seeds it from the block feed's initial snapshot), then
/// the edit is applied at the position the authority already gives the block.
async fn write_back(
    controller: &mut holon_filesystem::FileSyncController,
    f: &Fixtures,
    doc: &EntityUri,
    block: &Block,
) -> anyhow::Result<bool> {
    controller.seed_holder_from_authority(doc).await?;
    let siblings = f.children.get(doc).cloned().unwrap_or_default();
    let prev = prev_sibling(&siblings, block);
    controller
        .on_block_changed(
            doc,
            &BlockDelta::Upsert {
                block: block.clone(),
                prev,
            },
        )
        .await
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
    let result = write_back(&mut controller, &f, &f.prohibited_id, &prohibited_block)
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

/// **BugFunnel row 23/29 (RED-first, PROD-FAITHFUL): a write-back that would
/// DROP a prohibited-topology subtree HARD-VETOES + quarantines — driven by the
/// REAL `name_chain` assertion, no stub.** This reproduces the first-boot
/// 6,245-line `Projects.org` destruction: a `* Holon` heading collided with a
/// same-named subdir page, the re-homed subtree landed page-under-non-page, and
/// on the block-driven re-render its blocks were absent from the projection.
/// The guard's sibling-grounding calls the REAL `name_chain` on those absent
/// blocks → it fails loud (the 749-error storm) → they stay UNGROUNDED and the
/// write is REFUSED under the UNRESOLVABLE error, which names the prohibited
/// topology rather than reporting a generic removal.
#[tokio::test]
async fn writeback_drop_of_prohibited_subtree_hard_vetoes_prod_name_chain() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let container = EntityUri::block("container");
    let path = root.join("Container.org");

    // 1) Seed Container.org on disk with the FULL subtree (incl. the prohibited
    //    page under the non-page `b1`). The store returns the whole doc, so the
    //    render is complete and nothing is dropped.
    let full = container_fixtures(false);
    let leaf1 = full.by_id[&EntityUri::block("leaf1")].clone();
    let mut seeder = build_controller(&full, vec![], root.clone());
    write_back(&mut seeder, &full, &container, &leaf1)
        .await
        .expect("seeding the full Container.org must succeed (no drop)");
    let seeded = std::fs::read_to_string(&path).expect("Container.org must exist after seed");
    assert!(
        seeded.contains("Prohibited Page") && seeded.contains("prohibited body"),
        "precondition: the full prohibited subtree is on disk; got {seeded:?}"
    );

    // 2) The store now returns ONLY the leaves — the prohibited subtree was
    //    re-homed/de-inlined, so a re-render drops it. A fresh controller's guard
    //    grounds the absent blocks via the REAL name_chain, which fails loud
    //    (prohibited page under non-page `b1`) → UNRESOLVABLE → HARD VETO.
    let truncated = container_fixtures(true);
    let mut controller = build_controller(&truncated, vec![], root.clone());
    let veto = write_back(&mut controller, &truncated, &container, &leaf1).await;
    assert!(
        veto.is_err(),
        "a write-back dropping a prohibited-topology subtree (name_chain fails loud) must \
         HARD-VETO, never silently truncate the file"
    );
    let msg = format!("{:#}", veto.unwrap_err());
    assert!(
        msg.contains("UNRESOLVABLE"),
        "the veto must name the unresolvable-drop guard; got {msg}"
    );

    // 3) Disk is intact — the truncated projection was refused.
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("Prohibited Page") && after.contains("prohibited body"),
        "the vetoed write must leave the full subtree on disk; got {after:?}"
    );

    // 4) The grounding failure was surfaced loudly (fail-loud, not silent).
    let errors = cap.errors();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("name_chain failed loud") || e.contains("UNRESOLVABLE")),
        "expected a loud ERROR for the name_chain grounding failure; captured: {errors:?}"
    );
}

/// **BugFunnel row 23/29 (RED-first, TRULY PROD-FAITHFUL): a folder-companion
/// whose projection legitimately DE-INLINES a child page must PROCEED, not be
/// falsely hard-vetoed by a `name_chain` grounding storm.**
///
/// This reproduces the real `Projects.org` first-boot storm more faithfully
/// than [`writeback_drop_of_prohibited_subtree_hard_vetoes_prod_name_chain`],
/// which seeds the non-page blocks INTO the doc manager. In prod the page store
/// (`LiveDocumentManager`'s `WHERE tag='Page'` matview) tracks ONLY pages, so
/// `get_by_id` returns `None` for a non-page content heading — the actual log
/// signature was "Page block '…' not found in name_chain" (the ancestor-lookup
/// branch), NOT "non-page ancestor". Here `PageOnlyDocManager.get_by_id`
/// mirrors that: it returns a block only if it is a `Page`.
///
/// Topology: a companion page `container` (→ `Container.org`) inlines a CHILD
/// PAGE `child` (→ `Container/Child.org`) whose subtree holds a NON-PAGE
/// heading `child-body`. The store's render of `Container.org` de-inlines the
/// whole child subtree, so both `child` and `child-body` are absent from the
/// projection. Write-back grounding resolves each absent block:
///   - `child` is a page → resolves to its own sibling file
///     `Container/Child.org` (whose on-disk content grounds the drop);
///   - `child-body` is NOT a page → `get_by_id` misses → PRE-FIX `name_chain`
///     hit "not found" and marked it UNRESOLVABLE → false HARD-VETO +
///     quarantine at every boot. POST-FIX it resolves to an empty chain
///     (`Ok(None)`, "not a page"), the sibling grounding covers it, and the
///     legitimate de-inline write PROCEEDS.
#[tokio::test]
async fn companion_deinline_of_child_page_content_is_not_vetoed_prod_page_only_store() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let container = EntityUri::block("container");
    let container_path = root.join("Container.org");
    let child_path = root.join("Container").join("Child.org");

    // Seed the settled folder-companion on disk:
    //  - Container.org (companion) still inlines the child page + its non-page body
    //    — the pre-de-inline source about to be re-rendered.
    //  - Container/Child.org (sibling) owns the de-inlined child subtree.
    std::fs::create_dir_all(child_path.parent().unwrap()).unwrap();
    std::fs::write(
        &container_path,
        "#+ID: container\n* Child\n:PROPERTIES:\n:ID: child\n:END:\n** child body text\n\
         :PROPERTIES:\n:ID: child-body\n:END:\n",
    )
    .unwrap();
    std::fs::write(
        &child_path,
        "#+ID: child\n* child body text\n:PROPERTIES:\n:ID: child-body\n:END:\n",
    )
    .unwrap();

    // Page store holds ONLY the two PAGES (container, child) — never the
    // non-page `child-body`, exactly like the `WHERE tag='Page'` matview.
    let container_page = page("container", EntityUri::no_parent(), "Container");
    let child_page = page("child", container.clone(), "Child");
    let child_body = non_page("child-body", EntityUri::block("child"), "child body text");

    let mut reader_by_id = HashMap::new();
    for b in [&container_page, &child_page, &child_body] {
        reader_by_id.insert(b.id.clone(), b.clone());
    }
    // The render of Container.org de-inlines the child subtree → empty body.
    let mut children = HashMap::new();
    children.insert(container.clone(), Vec::<Block>::new());

    let reader = Arc::new(FixtureReader {
        by_id: reader_by_id,
        children,
        documents: vec![],
    });
    let mut page_only = HashMap::new();
    page_only.insert(container_page.id.clone(), container_page.clone());
    page_only.insert(child_page.id.clone(), child_page.clone());
    let doc_manager = Arc::new(PageOnlyDocManager { by_id: page_only });
    let mut controller = new_org_sync_controller(
        reader,
        doc_manager,
        root.clone(),
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    );

    // The de-inline is legitimate — the child subtree is preserved in the
    // sibling file — so the write must PROCEED, not hard-veto.
    controller
        .seed_holder_from_authority(&container)
        .await
        .expect("seeding the holder must not fail");
    // `children[container]` is empty — the child subtree is de-inlined — so the
    // container page has no preceding sibling inside its own document.
    let result = controller
        .on_block_changed(
            &container,
            &BlockDelta::Upsert {
                block: container_page.clone(),
                prev: None,
            },
        )
        .await;
    assert!(
        result.is_ok(),
        "a legitimate companion de-inline (child subtree preserved in its sibling file) must NOT \
         be hard-vetoed by a name_chain grounding storm; got {:?}",
        result.err().map(|e| format!("{e:#}")),
    );

    // No UNRESOLVABLE / name_chain-failed error was emitted — the storm is gone.
    let errors = cap.errors();
    assert!(
        !errors
            .iter()
            .any(|e| e.contains("UNRESOLVABLE") || e.contains("name_chain failed loud")),
        "the folder-companion de-inline must not emit any UNRESOLVABLE / name_chain grounding \
         error; captured: {errors:?}"
    );

    // The child subtree is still safe in its own file (nothing was destroyed).
    let sibling = std::fs::read_to_string(&child_path).unwrap();
    assert!(
        sibling.contains("child body text") && sibling.contains("child-body"),
        "the child page's own file must be untouched; got {sibling:?}"
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
