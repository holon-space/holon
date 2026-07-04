//! A page title containing a dot must own the file its title names.
//!
//! Derivation used `Path::with_extension("org")`, which REPLACES the segment
//! after the last dot instead of appending: the page `citrix-STX.BROWSER_AGENT`
//! derived `citrix-STX.org`. Two harms follow — the title that comes back out
//! of the file is not the one that went in, and a page actually titled
//! `citrix-STX` derives the SAME file, so one page silently overwrites the
//! other.
//!
//! The second test covers the MIGRATION: a vault that already holds the
//! truncated file Holon itself wrote. The page's alias still points at that
//! file, so the existing rename/double-homing cleanup in
//! `materialize_page_identity_file` must re-home the page onto the corrected
//! path and remove the truncated one — no page may stay double-homed.

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

const DOTTED_TITLE: &str = "citrix-STX.BROWSER_AGENT";
/// What `with_extension("org")` truncated the dotted title to.
const TRUNCATED_STEM: &str = "citrix-STX";

// ---------------------------------------------------------------------------
// Test doubles (same shape as `vault_path_escape.rs`).
// ---------------------------------------------------------------------------

struct FixtureReader {
    by_id: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<Block>>,
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
        Ok(Vec::new())
    }
}

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

/// Alias registrar backed by a real map, so a re-registration is observable —
/// the migration's proof that the page CHANGED its home, not merely that a
/// second file appeared.
#[derive(Default)]
struct MapAliasRegistrar(Mutex<HashMap<EntityUri, PathBuf>>);

impl MapAliasRegistrar {
    fn with(id: &EntityUri, path: &Path) -> Self {
        let mut m = HashMap::new();
        m.insert(id.clone(), path.to_path_buf());
        Self(Mutex::new(m))
    }

    fn get(&self, id: &EntityUri) -> Option<PathBuf> {
        self.0.lock().unwrap().get(id).cloned()
    }
}

#[async_trait]
impl holon_filesystem::sync_ports::AliasRegistrar for MapAliasRegistrar {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path) {
        self.0
            .lock()
            .unwrap()
            .insert(doc_id.clone(), path.to_path_buf());
    }
    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        self.get(doc_id)
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

/// `Agents/` (a directory page) holding the dotted page and its leaf.
struct Fixtures {
    by_id: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<Block>>,
    dotted_id: EntityUri,
}

fn fixtures() -> Fixtures {
    let dir = page("agents-dir", EntityUri::no_parent(), "Agents");
    let dotted = page("dotted-page", dir.id.clone(), DOTTED_TITLE);
    let leaf = non_page("dotted-leaf", dotted.id.clone(), "agent body");

    let mut by_id = HashMap::new();
    for b in [&dir, &dotted, &leaf] {
        by_id.insert(b.id.clone(), b.clone());
    }
    let mut children = HashMap::new();
    children.insert(dir.id.clone(), vec![dotted.clone()]);
    children.insert(dotted.id.clone(), vec![leaf]);

    Fixtures {
        dotted_id: dotted.id.clone(),
        by_id,
        children,
    }
}

/// macOS hands `tempdir()` a `/var/…` path that resolves to `/private/var/…`;
/// the controller's containment proof compares the two spellings literally, so
/// the vault root and every path a test hands it must be the SAME spelling.
fn vault_root(tmp: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(tmp.path()).unwrap()
}

fn build_controller(
    f: &Fixtures,
    root: PathBuf,
    registrar: Arc<MapAliasRegistrar>,
) -> holon_filesystem::FileSyncController {
    new_org_sync_controller(
        Arc::new(FixtureReader {
            by_id: f.by_id.clone(),
            children: f.children.clone(),
        }),
        Arc::new(PageOnlyDocManager {
            by_id: f.by_id.clone(),
        }),
        root,
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    )
    .with_alias_registrar(registrar)
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
    let prev = siblings
        .iter()
        .take_while(|b| b.id != block.id)
        .filter(|b| b.parent_id == block.parent_id)
        .last()
        .map(|b| b.id.clone());
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

/// Write-back lands in the file the TITLE names — dots and all.
#[tokio::test]
async fn a_dotted_page_title_writes_back_to_its_own_file() {
    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);
    let registrar = Arc::new(MapAliasRegistrar::default());
    let mut controller = build_controller(&f, root.clone(), registrar.clone());

    let dotted = f.by_id[&f.dotted_id].clone();
    write_back(&mut controller, &f, &f.dotted_id, &dotted)
        .await
        .expect("a well-formed dotted page must write back");

    let expected = root.join("Agents").join(format!("{DOTTED_TITLE}.org"));
    let truncated = root.join("Agents").join(format!("{TRUNCATED_STEM}.org"));
    assert!(
        expected.exists(),
        "the page's own file was never written: {expected:?}"
    );
    assert!(
        !truncated.exists(),
        "the title was TRUNCATED at its dot — wrote {truncated:?} instead"
    );
    assert_eq!(
        registrar.get(&f.dotted_id),
        Some(expected),
        "the page must own the file its title names"
    );
}

/// Migration: the vault already holds the truncated file Holon wrote before
/// the fix, and the page's alias still points at it. The next write must
/// re-home the page — new file written, truncated one removed, alias updated —
/// so the page is never double-homed.
#[tokio::test]
async fn a_page_homed_at_the_truncated_file_is_relocated_on_the_next_write() {
    let f = fixtures();
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_root(&tmp);

    let stale = root.join("Agents").join(format!("{TRUNCATED_STEM}.org"));
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, b"#+ID: dotted-page\n* agent body\n").unwrap();

    let registrar = Arc::new(MapAliasRegistrar::with(&f.dotted_id, &stale));
    let mut controller = build_controller(&f, root.clone(), registrar.clone());

    let dotted = f.by_id[&f.dotted_id].clone();
    write_back(&mut controller, &f, &f.dotted_id, &dotted)
        .await
        .expect("re-homing a mis-filed page must not crash the sync loop");

    let expected = root.join("Agents").join(format!("{DOTTED_TITLE}.org"));
    assert!(
        expected.exists(),
        "the page was not re-homed onto its correct file: {expected:?}"
    );
    assert!(
        !stale.exists(),
        "the page stayed DOUBLE-HOMED — the truncated file {stale:?} survived the re-home"
    );
    assert_eq!(
        registrar.get(&f.dotted_id),
        Some(expected),
        "the alias must follow the page to its new home"
    );
}
