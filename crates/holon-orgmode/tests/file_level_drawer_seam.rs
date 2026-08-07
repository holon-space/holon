//! The file-level `:PROPERTIES:` drawer across the REAL ingest→store→write-back
//! seam, not the parse↔render unit.
//!
//! `holon-org-format`'s own tests prove `parse(render(x)) == x` inside one
//! crate. They stayed green while the drawer was still deleted from every real
//! file, because the app never renders the PARSED doc-root — it renders the
//! STORE's, and nothing copied the drawer onto it. Anything that claims the
//! drawer survives has to drive `FileSyncController::on_file_changed`, which is
//! what this file does.
//!
//! The fixture is the org-roam shape the feature exists for: a file-level
//! drawer carrying `:ID:` and an unmodelled `:ROAM_REFS:`, and headlines with
//! NO per-headline `:ID:`. Those missing headline ids are what make the
//! controller mint ids and force a real write-back, so the assertions below are
//! on bytes the app actually rewrote — not on a file it happened to leave
//! alone.
//!
//! @pbt kind harness
//! @pbt covers file-level-drawer-seam — drawer + drawer-`:ID:` identity survive
//! ingest→store→write-back (dogfood 2026-08-07)

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

const DRAWER_ID: &str = "20260807T101010";

/// The blocks the ingest created, shared with [`StoreReader`] so the
/// controller's post-ingest doc walk sees what it just wrote — as it does in
/// prod. An always-empty reader makes `on_file_changed` abort before the
/// write-back this file is about.
type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

#[derive(Clone, Default)]
struct RecordingOrdering {
    updates: Arc<Mutex<Vec<holon_api::StorageEntity>>>,
    store: Store,
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
    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        _: Option<&EntityUri>,
        id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &HashMap<String, holon_api::Value>,
        tags: &holon_api::types::Tags,
        _: &[EntityUri],
        _: &[EntityUri],
    ) -> OrderingResult<bool> {
        let mut block = Block::new_text(
            id.clone(),
            parent_id.clone(),
            content.as_text().unwrap_or(""),
        );
        block.properties = properties.clone();
        block.tags = tags.clone();
        self.store.lock().unwrap().insert(id.clone(), block);
        // `false` = "no separate consolidator persisted this" — the same answer
        // the default impl gives. Returning `true` without also wiring a
        // projection sink trips the controller's DI-wiring assertion.
        Ok(false)
    }
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        self.updates.lock().unwrap().push(params);
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

/// Reads back what the ingest created, so the controller's completeness gate
/// behaves as it does against a real store.
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
    async fn get_block_authoritative(&self, _: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    async fn resolve_link_marks(&self, _: &mut [Block]) -> anyhow::Result<()> {
        Ok(())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// Mirrors `LiveDocumentManager`: `get_by_id` resolves only `Page`s. Records
/// every `update_metadata` so a test can see what the ingest actually persisted
/// onto the doc-root — the seam where the drawer was being dropped.
#[derive(Clone)]
struct RecordingDocManager {
    /// Shared and MUTABLE on purpose: in prod `update_metadata` writes the
    /// doc-root to the store and the write-back's later `get_by_id` reads it
    /// back. A double that records the update without applying it hands
    /// write-back a stale root and hides exactly the bug under test.
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
    metadata: Arc<Mutex<Vec<Block>>>,
    created: Arc<Mutex<Vec<Block>>>,
}

#[async_trait]
impl DocumentManager for RecordingDocManager {
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
        self.created.lock().unwrap().push(doc.clone());
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
        self.metadata.lock().unwrap().push(doc.clone());
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

/// The org-roam shape: file-level drawer first, an unmodelled key beside the
/// `:ID:`, and a headline with no id of its own.
fn fixture() -> String {
    format!(
        ":PROPERTIES:\n:ID: {DRAWER_ID}\n:ROAM_REFS: https://example.com/paper\n:END:\n#+TITLE: \
         Seam\n* A heading with no id\n"
    )
}

/// Live counterexample 1: the drawer is INDENTED. Org allows it and so must we.
fn indented_fixture() -> String {
    format!(
        "  :PROPERTIES:\n  :ID: {DRAWER_ID}\n  :ROAM_REFS: \
         https://example.com/paper\n  :END:\n#+TITLE: Seam\n* A heading with no id\n"
    )
}

/// Live counterexample 2: a value-less `:KEY:` line, which orgize's property
/// grammar refuses — voiding the whole drawer if we relied on it.
fn value_less_key_fixture() -> String {
    format!(
        ":PROPERTIES:\n:ID: {DRAWER_ID}\n:ARCHIVED:\n:ROAM_REFS: \
         https://example.com/paper\n:END:\n#+TITLE: Seam\n* A heading with no id\n"
    )
}

struct Ingested {
    disk: String,
    persisted_doc: Vec<Block>,
    created_docs: Vec<Block>,
}

/// `seeded = true` models a vault that already knows the page; `false` models
/// a file the store has NEVER seen — a fresh org-roam note dropped into the
/// vault. Only the unseeded case exercises identity resolution: with the page
/// already present, name-chain lookup finds it and lands on the right id even
/// when the drawer was never consulted, which silently hides a broken
/// `doc_id_from_content`.
async fn ingest_fixture(seeded: bool) -> Ingested {
    ingest_content(seeded, &fixture()).await
}

async fn ingest_content(seeded: bool, content: &str) -> Ingested {
    let uri = EntityUri::block(DRAWER_ID);
    let mut seed_map = HashMap::new();
    if seeded {
        let mut page = Block::new_text(uri.clone(), EntityUri::no_parent(), "Seam");
        page.set_page(true);
        seed_map.insert(uri.clone(), page);
    }
    let metadata = Arc::new(Mutex::new(Vec::new()));
    let created = Arc::new(Mutex::new(Vec::new()));
    let doc_manager = RecordingDocManager {
        by_id: Arc::new(Mutex::new(seed_map)),
        metadata: metadata.clone(),
        created: created.clone(),
    };

    let ordering = RecordingOrdering::default();
    let store = ordering.store.clone();
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize: on macOS `/var` is a symlink and the controller's
    // `strip_prefix(root)` works on the resolved shape.
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Seam.org");
    std::fs::write(&path, content).unwrap();

    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store)),
        Arc::new(doc_manager),
        root.clone(),
        Arc::new(ordering),
        Arc::new(RealFileSystem),
    );
    controller
        .on_file_changed(&path)
        .await
        .expect("ingest of a drawer-identified file must not error");

    let persisted_doc = metadata.lock().unwrap().clone();
    let created_docs = created.lock().unwrap().clone();
    Ingested {
        disk: std::fs::read_to_string(&path).unwrap(),
        persisted_doc,
        created_docs,
    }
}

/// THE regression: after the app has ingested and rewritten the file, the
/// author's drawer is still there, with its unmodelled key.
#[tokio::test]
async fn the_drawer_survives_a_real_write_back() {
    let out = ingest_fixture(true).await;

    assert!(
        out.disk.starts_with(":PROPERTIES:\n"),
        "the file-level drawer must still open the file after write-back; got:\n---\n{}---",
        out.disk
    );
    assert!(
        out.disk.contains(&format!(":ID: {DRAWER_ID}\n")),
        "the author's drawer :ID: must survive write-back; got:\n---\n{}---",
        out.disk
    );
    assert!(
        out.disk.contains(":ROAM_REFS: https://example.com/paper\n"),
        "an unmodelled drawer key must survive write-back; got:\n---\n{}---",
        out.disk
    );
}

/// THE identity seam, on a file the store has never seen — the only shape that
/// actually exercises it. `FileSyncController` asks the format for the file's
/// id BEFORE parsing; when that probe ignores the drawer the answer is `None`,
/// the controller mints a name-chain uuid, and the document's identity is NOT
/// the one the author wrote. Every link written against the drawer's id then
/// points at nothing.
#[tokio::test]
async fn a_never_seen_file_takes_its_identity_from_the_drawer() {
    let out = ingest_fixture(false).await;

    let doc = out
        .created_docs
        .iter()
        .chain(out.persisted_doc.iter())
        .next()
        .unwrap_or_else(|| panic!("ingest neither created nor persisted a doc-root"));
    assert_eq!(
        format!("block:{DRAWER_ID}"),
        doc.id.as_str(),
        "the document must be identified by the drawer's :ID:, not by a freshly minted \
         name-chain uuid"
    );
}

/// The same file, once round-tripped: an unseeded ingest must not stamp a
/// second carrier either. This is the shape that bricks the file — the app
/// writes `#+ID: <minted>` beside the drawer, and every later parse rejects the
/// conflict.
#[tokio::test]
async fn a_never_seen_file_is_not_bricked_by_its_first_write_back() {
    let out = ingest_fixture(false).await;

    assert!(
        !out.disk.contains("#+ID:"),
        "write-back stamped a second identity carrier onto a drawer-identified file; the next \
         ingest rejects it as a conflict and the document is broken for good. got:\n---\n{}---",
        out.disk
    );
    // The quieter failure, and the worse one: the drawer is kept but its `:ID:`
    // is re-derived from a document identity that never came from the drawer,
    // so the author's id is overwritten in place with no error anywhere.
    assert!(
        out.disk.contains(&format!(":ID: {DRAWER_ID}\n")),
        "the author's drawer :ID: was REWRITTEN by write-back — the document silently changed \
         identity and every link to the old id now dangles. got:\n---\n{}---",
        out.disk
    );
    holon_org_format::parse_org_file(
        std::path::Path::new("/vault/Seam.org"),
        &out.disk,
        &EntityUri::no_parent(),
        std::path::Path::new("/vault"),
    )
    .unwrap_or_else(|e| panic!("the app bricked its own file: {e}\n---\n{}---", out.disk));
}

/// The identity claim, in prod rather than in the parse unit: a drawer-only
/// file must NOT be treated as identity-less. When it is, the controller
/// force-writes `#+ID: <name-chain uuid>` beside the drawer — and the next
/// ingest sees two disagreeing carriers and rejects the file for good.
#[tokio::test]
async fn no_second_id_carrier_is_stamped_onto_the_file() {
    let out = ingest_fixture(true).await;

    assert!(
        !out.disk.contains("#+ID:"),
        "the drawer already identifies this document — write-back must not add a `#+ID:` \
         carrier, which the next parse would reject as a conflict. got:\n---\n{}---",
        out.disk
    );
}

/// Re-ingesting what the app itself wrote must work. This is the trap the
/// identity bug set: the first write-back produced a file the second ingest
/// refuses, so the document breaks permanently and the app caused it.
#[tokio::test]
async fn what_the_app_wrote_back_can_be_ingested_again() {
    let out = ingest_fixture(true).await;

    let parsed = holon_org_format::parse_org_file(
        std::path::Path::new("/vault/Seam.org"),
        &out.disk,
        &EntityUri::no_parent(),
        std::path::Path::new("/vault"),
    )
    .unwrap_or_else(|e| {
        panic!(
            "the app's own write-back is not re-ingestable — it wrote a file its parser \
             rejects: {e}\n---\n{}---",
            out.disk
        )
    });
    assert_eq!(
        format!("block:{DRAWER_ID}"),
        parsed.document.id.as_str(),
        "identity must still come from the drawer on re-ingest"
    );
}

/// The store seam itself: the drawer has to be COPIED onto the persisted
/// doc-root, because write-back renders the persisted root and not the parsed
/// one. Asserting on disk alone would not localize a regression to here.
#[tokio::test]
async fn the_drawer_reaches_the_persisted_doc_root() {
    use holon_org_format::OrgDocumentExt;

    let out = ingest_fixture(true).await;
    let doc = out
        .persisted_doc
        .last()
        .unwrap_or_else(|| panic!("ingest persisted no doc-root metadata at all"));
    let drawer = doc.file_drawer().unwrap_or_else(|| {
        panic!(
            "the persisted doc-root carries no file-level drawer — write-back renders THIS block, \
             so the drawer is gone from disk on the next write. properties = {:?}",
            doc.properties
        )
    });
    assert_eq!(
        Some(&serde_json::Value::String(DRAWER_ID.to_string())),
        drawer.get("ID"),
    );
    assert_eq!(
        Some(&serde_json::Value::String(
            "https://example.com/paper".to_string()
        )),
        drawer.get("ROAM_REFS"),
    );
}

/// LIVE COUNTEREXAMPLE 1, end to end: an INDENTED file-level drawer on a file
/// the store has never seen. The live failure rewrote the author's `:ID:` in
/// place with a minted uuid and logged nothing.
#[tokio::test]
async fn an_indented_drawer_keeps_its_identity_through_the_seam() {
    let out = ingest_content(false, &indented_fixture()).await;

    let doc = out
        .created_docs
        .iter()
        .chain(out.persisted_doc.iter())
        .next()
        .unwrap_or_else(|| panic!("ingest neither created nor persisted a doc-root"));
    assert_eq!(
        format!("block:{DRAWER_ID}"),
        doc.id.as_str(),
        "an indented drawer is still the file's drawer — its :ID: identifies the document"
    );
    assert!(
        out.disk.contains(&format!(":ID: {DRAWER_ID}\n")),
        "the author's :ID: must survive, not be rewritten to a minted uuid. got:\n---\n{}---",
        out.disk
    );
    assert!(
        !out.disk.contains("#+ID:"),
        "no second carrier may be stamped on. got:\n---\n{}---",
        out.disk
    );
    // Indentation CANONICALIZES to column 0 (disclosed in ORG_SYNTAX.md).
    assert!(
        out.disk.starts_with(":PROPERTIES:\n"),
        "got:\n---\n{}---",
        out.disk
    );
}

/// LIVE COUNTEREXAMPLE 2, end to end: a value-less `:KEY:`. The live failure
/// voided the whole drawer, stamped `#+ID:`, and sank the drawer below
/// `#+TITLE:` where it is no longer a file-level drawer at all.
#[tokio::test]
async fn a_value_less_key_survives_the_seam_without_voiding_the_drawer() {
    let out = ingest_content(false, &value_less_key_fixture()).await;

    let doc = out
        .created_docs
        .iter()
        .chain(out.persisted_doc.iter())
        .next()
        .unwrap_or_else(|| panic!("ingest neither created nor persisted a doc-root"));
    assert_eq!(
        format!("block:{DRAWER_ID}"),
        doc.id.as_str(),
        "one value-less key must not cost the document its identity"
    );
    assert!(
        out.disk.starts_with(":PROPERTIES:\n"),
        "the drawer must stay ABOVE #+TITLE:, where org still reads it as file-level. \
         got:\n---\n{}---",
        out.disk
    );
    assert!(
        out.disk.contains(":ARCHIVED: \n"),
        "the value-less key survives. got:\n---\n{}---",
        out.disk
    );
    assert!(
        !out.disk.contains("#+ID:"),
        "no second carrier may be stamped on. got:\n---\n{}---",
        out.disk
    );
}
