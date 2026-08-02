//! Write-back path derivation must never name a file outside the vault root.
//!
//! A `Page`-tagged block with an EMPTY title (the permanent placeholder husk
//! minted for a vault directory that owns no `<dir>.org` companion) contributes
//! an EMPTY element to `name_chain`. `doc_id_to_path` then did
//! `root_dir.join(chain.join("/")).with_extension("org")`, which turns that
//! empty element into an escape in two distinct shapes:
//!
//!   A. chain `[""]`            → `join("")` is the root itself, and
//!      `with_extension("org")` names the root's SIBLING — Holon wrote
//!      `/Users/martin/Workspaces/pkm/holon-pkm.org`, outside the vault.
//!   B. chain `["", "<name>"]`  → `chain.join("/")` is `"/<name>"`, an ABSOLUTE
//!      component, and `PathBuf::join` DISCARDS the base — the real page
//!      "Optimize RAG" resolved to `/Optimize RAG.org`, hit EROFS, and had its
//!      write-back silently disabled for the session (its edits never reached
//!      disk).
//!
//! Required behavior (parse-don't-validate): derivation FAILS LOUD — no write,
//! no file outside the root, and the refusal is DISCLOSED through the existing
//! `doc_id_to_path` `Err` seam (an ERROR-level event naming the doc, bounded to
//! that one document).

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
// ERROR-level tracing capture (same shape as name_chain_error_propagation.rs).
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

/// Prod-faithful page store: `get_by_id` serves ONLY `Page`-tagged blocks,
/// mirroring `LiveDocumentManager`'s `WHERE tag='Page'` matview — the husk IS
/// `Page`-tagged in prod (that is why the sidebar renders it), so it is visible
/// here and the REAL default `name_chain` walks through it.
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

/// The live-vault shape: an EMPTY-titled `Page` husk at the vault root, with a
/// real page ("Optimize RAG") beneath it and a leaf under that page.
struct Fixtures {
    by_id: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<Block>>,
    husk_id: EntityUri,
    child_page_id: EntityUri,
    legit_id: EntityUri,
}

fn fixtures() -> Fixtures {
    let husk = page("husk", EntityUri::no_parent(), "");
    let child_page = page("child-page", husk.id.clone(), "Optimize RAG");
    let child_leaf = non_page("child-leaf", child_page.id.clone(), "rag body");
    let legit = page("legit", EntityUri::no_parent(), "Legit Page");
    let legit_leaf = non_page("legit-leaf", legit.id.clone(), "legit body");

    let mut by_id = HashMap::new();
    for b in [&husk, &child_page, &child_leaf, &legit, &legit_leaf] {
        by_id.insert(b.id.clone(), b.clone());
    }

    let mut children = HashMap::new();
    children.insert(husk.id.clone(), vec![child_page.clone()]);
    children.insert(child_page.id.clone(), vec![child_leaf]);
    children.insert(legit.id.clone(), vec![legit_leaf]);

    Fixtures {
        husk_id: husk.id.clone(),
        child_page_id: child_page.id.clone(),
        legit_id: legit.id.clone(),
        by_id,
        children,
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
    let doc_manager = Arc::new(PageOnlyDocManager {
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

/// Every path OUTSIDE `root` that this fixture could escape to. `root`'s
/// sibling `<root>.org` is the shape-A escape actually observed in the vault.
fn sibling_escape(root: &std::path::Path) -> std::path::PathBuf {
    root.with_extension("org")
}

fn walkdir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// A refusal must be DISCLOSED as a vault-containment failure, not merely as
/// whatever downstream symptom the escaped path happened to produce (EROFS).
fn assert_containment_disclosed(errors: &[String], doc_marker: &str) {
    let named = errors.iter().any(|e| {
        e.contains(doc_marker) && (e.contains("outside the vault") || e.contains("vault root"))
    });
    assert!(
        named,
        "expected an ERROR-level event disclosing a VAULT-CONTAINMENT refusal for {doc_marker}; \
         captured: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// Shape A — chain [""]: the husk itself must never name the root's sibling.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn husk_writeback_never_writes_outside_the_vault_root() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let escape = sibling_escape(&root);
    // Pre-condition: nothing of ours exists outside the vault yet.
    assert!(
        !escape.exists(),
        "test precondition: {escape:?} must not exist"
    );

    let mut controller = build_controller(&f, vec![], root.clone());
    let husk_block = f.by_id[&f.husk_id].clone();
    let result = controller
        .on_block_changed(&f.husk_id, &BlockDelta::Upsert(husk_block))
        .await
        .expect("an underivable path must be a bounded skip, never crash the sync loop");

    // Clean up before asserting so a red run leaves no litter next to the tmpdir.
    let escaped = escape.exists();
    if escaped {
        let _ = std::fs::remove_file(&escape);
    }
    assert!(
        !escaped,
        "write-back derived a path OUTSIDE the vault root and WROTE it: {escape:?}"
    );
    assert!(
        !result,
        "on_block_changed must report 'no file written' (Ok(false)) for an underivable path"
    );
    assert_containment_disclosed(&cap.errors(), "husk");
}

// ---------------------------------------------------------------------------
// Shape B — chain ["", "Optimize RAG"]: an absolute component must not discard
// the vault root and strand a REAL page's write-back.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn child_of_husk_writeback_is_refused_with_containment_disclosure() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let mut controller = build_controller(&f, vec![], root.clone());
    let child_block = f.by_id[&f.child_page_id].clone();
    let result = controller
        .on_block_changed(&f.child_page_id, &BlockDelta::Upsert(child_block))
        .await
        .expect("an underivable path must be a bounded skip, never crash the sync loop");

    assert!(
        !result,
        "on_block_changed must report 'no file written' (Ok(false)) for an underivable path"
    );
    assert!(
        walkdir(&root).is_empty(),
        "nothing may be written under the vault root for an underivable path; found: {:?}",
        walkdir(&root)
    );
    // Red today: the ONLY error is the EROFS write-back-disabled disclosure for
    // `/Optimize RAG.org` — the path escape itself is never disclosed, and the
    // page is silently stranded for the rest of the session.
    assert_containment_disclosed(&cap.errors(), "child-page");
}

// ---------------------------------------------------------------------------
// Disclosure volume: loud ONCE per doc, not once per sync tick.
// ---------------------------------------------------------------------------

/// The condition is permanent until the offending page gains a title, and the
/// sync loop retries on every CDC event — so an unconditional ERROR per attempt
/// would bury every other error in Martin's log. Mark-and-log-once (the EROFS
/// precedent): the ERROR count must be CONSTANT in the number of attempts, not
/// linear in it. It stays greater than zero — the condition must remain
/// visible, just not repeated.
/// Drive `on_block_changed(routed_doc, Upsert(delta_block))` `ticks` times on a
/// fresh controller and return the captured ERROR lines mentioning `marker`.
async fn errors_after(
    ticks: usize,
    routed_doc: fn(&Fixtures) -> EntityUri,
    delta_block: fn(&Fixtures) -> EntityUri,
    marker: &str,
) -> Vec<String> {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let mut controller = build_controller(&f, vec![], tmp.path().to_path_buf());
    let doc = routed_doc(&f);
    let block = f.by_id[&delta_block(&f)].clone();
    for _ in 0..ticks {
        controller
            .on_block_changed(&doc, &BlockDelta::Upsert(block.clone()))
            .await
            .expect("an underivable path must be a bounded skip, never crash the sync loop");
    }
    cap.errors()
        .into_iter()
        .filter(|e| e.contains(marker))
        .collect()
}

/// The count must be CONSTANT in the number of ticks, and non-zero — the
/// condition stays visible, it just stops repeating.
fn assert_constant_in_ticks(once: &[String], many: &[String], case: &str) {
    assert!(
        !once.is_empty(),
        "{case}: the refusal must stay VISIBLE — de-duplication must not silence it"
    );
    assert_eq!(
        many.len(),
        once.len(),
        "{case}: ERROR volume must be constant in the number of sync ticks, not linear: 1 tick \
         gave {} line(s), 20 ticks gave {}. 20-tick capture: {many:?}",
        once.len(),
        many.len(),
    );
}

/// The condition is permanent until the offending page gains a title, and the
/// sync loop retries on every CDC event — so an unconditional ERROR per attempt
/// would bury every other error in Martin's log. Mark-and-log-once (the EROFS
/// precedent).
#[tokio::test]
async fn underivable_write_disclosure_does_not_grow_per_sync_tick() {
    let once = errors_after(
        1,
        |f| f.child_page_id.clone(),
        |f| f.child_page_id.clone(),
        "child-page",
    )
    .await;
    let many = errors_after(
        20,
        |f| f.child_page_id.clone(),
        |f| f.child_page_id.clone(),
        "child-page",
    )
    .await;
    assert_constant_in_ticks(&once, &many, "routed to the failing doc itself");
}

/// The SAME bound must hold when the untitled page is routed to a DIFFERENT
/// document whose own path derives fine — the prod shape named by
/// `on_block_changed`'s own comment: `resolve_doc_for_block` reads the
/// block-feed, whose `is_page` lags the authoritative store, so a just-minted
/// page is routed to its PARENT. Concretely: create a page and do not type its
/// title yet.
///
/// This case is what a per-doc (rather than per-`(entity, site)`) clear misses:
/// the parent's successful derivation wipes the untitled page's identity-file
/// mark on every tick, so the ERROR fires forever.
#[tokio::test]
async fn untitled_page_routed_to_its_parent_discloses_once_not_per_tick() {
    let once = errors_after(1, |f| f.legit_id.clone(), |f| f.husk_id.clone(), "husk").await;
    let many = errors_after(20, |f| f.legit_id.clone(), |f| f.husk_id.clone(), "husk").await;
    assert_constant_in_ticks(&once, &many, "untitled page routed to its parent");
}

// ---------------------------------------------------------------------------
// The alias registrar is an UNPROVEN source for deletes too, not just writes.
// ---------------------------------------------------------------------------

/// Alias registrar that hands back a path OUTSIDE the vault — the shape a
/// stale/rewritten alias entry produces.
struct OutOfVaultAliasRegistrar {
    prior: std::path::PathBuf,
}

#[async_trait]
impl holon_filesystem::sync_ports::AliasRegistrar for OutOfVaultAliasRegistrar {
    async fn register_alias(&self, _: &EntityUri, _: &std::path::Path) {}
    async fn resolve_alias_to_path(&self, _: &EntityUri) -> Option<std::path::PathBuf> {
        Some(self.prior.clone())
    }
}

/// A page rename removes the page's previous on-disk home, and that previous
/// path comes from the alias registrar. If the alias names a file OUTSIDE the
/// vault, the rename cleanup DELETES it — the same containment escape as an
/// out-of-vault write, in the direction that destroys data rather than
/// littering. The write path's alias read is proven; this one must be too.
#[tokio::test]
async fn rename_cleanup_never_deletes_outside_the_vault_root() {
    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    // A bystander file OUTSIDE the vault, as if a stale alias pointed at it.
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("someone-elses-notes.org");
    std::fs::write(&victim, b"* precious\n").unwrap();

    let reader = Arc::new(FixtureReader {
        by_id: f.by_id.clone(),
        children: f.children.clone(),
        documents: vec![],
    });
    let doc_manager = Arc::new(PageOnlyDocManager {
        by_id: f.by_id.clone(),
    });
    let mut controller = new_org_sync_controller(
        reader,
        doc_manager,
        root.clone(),
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    )
    .with_alias_registrar(Arc::new(OutOfVaultAliasRegistrar {
        prior: victim.clone(),
    }));

    // `legit` is a well-formed page, so everything except the alias is healthy:
    // the ONLY unproven input is the prior path fed to the delete.
    let legit_block = f.by_id[&f.legit_id].clone();
    let _ = controller
        .on_block_changed(&f.legit_id, &BlockDelta::Upsert(legit_block))
        .await;

    assert!(
        victim.exists(),
        "the rename cleanup DELETED a file outside the vault root: {victim:?}"
    );
}

// ---------------------------------------------------------------------------
// Blast radius: the refusal is per-document — the sweep still serves the rest.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sweep_skips_underivable_docs_but_materializes_the_legit_one() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let escape = sibling_escape(&root);
    assert!(
        !escape.exists(),
        "test precondition: {escape:?} must not exist"
    );

    // Underivable docs FIRST so we prove the sweep continues past them.
    let mut controller = build_controller(
        &f,
        vec![
            f.husk_id.clone(),
            f.child_page_id.clone(),
            f.legit_id.clone(),
        ],
        root.clone(),
    );
    controller
        .materialize_missing_page_files()
        .await
        .expect("the sweep must be a bounded skip, never abort wholesale");

    let escaped = escape.exists();
    if escaped {
        let _ = std::fs::remove_file(&escape);
    }
    assert!(
        !escaped,
        "the boot sweep wrote OUTSIDE the vault root: {escape:?}"
    );
    assert!(
        root.join("Legit Page.org").exists(),
        "the legit page must still materialize; dir contents: {:?}",
        walkdir(&root)
    );
    assert_containment_disclosed(&cap.errors(), "husk");
}

// ---------------------------------------------------------------------------
// Shape C — an IMAGE block's path. `block.content` of an image block is author-
// or CRDT-sync-supplied data, and `materialize_images` turns it into a write
// target. A traversal segment there is the same containment escape as an
// underivable name chain, in the direction that PLANTS bytes: a peer that can
// send a block can send the bytes to go with it.
// ---------------------------------------------------------------------------

/// The bytes half of a synced image block — always present, as if a peer had
/// sent the image along with the block that names it.
struct PeerImageBytes;

#[async_trait]
impl holon_filesystem::ImageDataProvider for PeerImageBytes {
    async fn read_image_data(&self, _: &EntityUri) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(Some(b"PEER-SUPPLIED-IMAGE-BYTES".to_vec()))
    }
    async fn write_image_data(&self, _: &EntityUri, _: Vec<u8>) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A page owning ONE image block whose content is `image_content`.
fn image_fixtures(image_content: &str) -> Fixtures {
    let owner = page("legit", EntityUri::no_parent(), "Legit Page");
    let image = Block::new_image(
        EntityUri::block("img"),
        owner.id.clone(),
        image_content.to_string(),
    );

    let mut by_id = HashMap::new();
    for b in [&owner, &image] {
        by_id.insert(b.id.clone(), b.clone());
    }
    let mut children = HashMap::new();
    children.insert(owner.id.clone(), vec![image.clone()]);

    Fixtures {
        husk_id: owner.id.clone(),
        child_page_id: owner.id.clone(),
        legit_id: owner.id.clone(),
        by_id,
        children,
    }
}

/// Drive ONE image-block upsert through the real write-back and report
/// `(the owning page's org file was written, the block's content afterwards)`.
async fn materialize_image_with_content(
    image_content: &str,
    root: &std::path::Path,
) -> (bool, String) {
    let f = image_fixtures(image_content);
    let image = f.by_id[&EntityUri::block("img")].clone();
    let content = image.content.clone();
    let mut controller = build_controller(&f, vec![f.legit_id.clone()], root.to_path_buf())
        .with_image_data(Arc::new(PeerImageBytes));
    controller
        .on_block_changed(&f.legit_id, &BlockDelta::Upsert(image))
        .await
        .expect("a refused image path must be a bounded skip, never abort write-back");
    (root.join("Legit Page.org").exists(), content)
}

#[tokio::test]
async fn image_path_traversal_never_writes_outside_the_vault_root() {
    // The vault is a SUBDIRECTORY of the tmpdir, so `../` escapes the vault
    // while still landing inside the tmpdir the test cleans up.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let escape = tmp.path().join("escape.png");
    assert!(
        !escape.exists(),
        "test precondition: {escape:?} must not exist"
    );

    let (page_written, content) = materialize_image_with_content("../escape.png", &root).await;

    let planted = std::fs::read(&escape).unwrap_or_default();
    assert!(
        !escape.exists(),
        "an image block's content escaped the vault root and PLANTED {} bytes at {escape:?}: {:?}",
        planted.len(),
        String::from_utf8_lossy(&planted),
    );
    // A refused image must not take the surrounding write-back down with it.
    assert!(
        page_written,
        "the owning page's org file must still be written; dir: {:?}",
        walkdir(&root)
    );
    assert_eq!(
        content, "../escape.png",
        "the block keeps its content — the path is refused as a WRITE TARGET, not rewritten"
    );
}

#[tokio::test]
async fn contained_image_path_still_materializes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();

    let (page_written, _) = materialize_image_with_content("attachments/sub/img.png", &root).await;

    assert!(page_written, "the owning page's org file must be written");
    assert!(
        root.join("attachments/sub/img.png").exists(),
        "a CONTAINED image path must still materialize; dir: {:?}",
        walkdir(&root)
    );
}
