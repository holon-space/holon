//! A page file whose `#+ID:` (or LogSeq `id::`) names a scheme (`block:abc`)
//! is refused by name through the controller, like a bad heading `:ID:`: the
//! ingest fails, the file is disclosed as refused, and no document is keyed by
//! a double-schemed id. Ruling 2026-09-27.

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::file_format::FormatRegistry;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::RealFileSystem;
use holon_markdown::LogseqMarkdownAdapter;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

/// A page and its blocks.
type Page = (Block, Vec<Block>);

/// Pages and their blocks, editable between passes.
#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<EntityUri, Page>>>);

#[async_trait]
impl BlockReader for Store {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|(_, kids)| kids.clone())
            .unwrap_or_default())
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
        let store = self.0.lock().unwrap();
        Ok(store.values().find_map(|(page, kids)| {
            std::iter::once(page)
                .chain(kids)
                .find(|b| &b.id == id)
                .cloned()
        }))
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        let mut docs: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (_, kids))| (id.clone(), kids.clone()))
            .collect();
        docs.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Ok(docs)
    }
}

#[async_trait]
impl DocumentManager for Store {
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
        Ok(self.0.lock().unwrap().get(id).map(|(page, _)| page.clone()))
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

#[derive(Default)]
struct Disclosures(Mutex<Vec<String>>);

impl holon_filesystem::WritebackDisclosure for Disclosures {
    fn writeback_degraded(&self, _: &str) {}
    fn writeback_stalled(&self, _: &std::path::Path, _: &str) {}
    fn writeback_resumed(&self, _: &std::path::Path) {}
    fn ingest_refused(&self, path: &std::path::Path, _: &str, reason: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("refused {}: {reason}", path.display()));
    }
    fn ingest_recovered(&self, _: &std::path::Path) {}
    fn vault_file_emptied(&self, _: &std::path::Path) {}
    fn writeback_lossy(&self, _: &std::path::Path, _: &str) {}
    fn writeback_faithful(&self, _: &std::path::Path) {}
    fn block_in_two_files(
        &self,
        _: &holon_api::EntityUri,
        _: &holon_api::EntityUri,
        _: Option<&std::path::Path>,
        _: &[&std::path::Path],
        _: bool,
    ) {
    }
    fn block_in_one_file_again(&self, _: &holon_api::EntityUri) {}
    fn deleted_block_kept_in_file(&self, _: &holon_api::EntityUri, _: &std::path::Path) {}
    fn deletion_undone(
        &self,
        _: &holon_api::EntityUri,
        _: &std::path::Path,
        _: &[&std::path::Path],
    ) {
    }
    fn undone_deletion_resolved(&self, _: &holon_api::EntityUri) {}
    fn deletion_ended_by_edit(&self, _: &holon_api::EntityUri, _: &std::path::Path) {}
    fn vault_sync_not_started(&self, _: &std::path::Path, _: &str) {}
    fn vault_state_unreadable(&self, _: &std::path::Path, _: &std::path::Path, _: &str) {}
    fn vault_start_incomplete(&self, _: &std::path::Path, _: &str, _: &str) {}
    fn written_files_unrecorded(&self, _: &std::path::Path, _: &[&std::path::Path], _: &str) {}
    fn written_files_recorded(&self, _: &std::path::Path) {}
    fn deleted_block_gone_from_file(&self, _: &holon_api::EntityUri) {}
}

struct Vault {
    sync: FileSyncController,
    store: Store,
    disclosures: Arc<Disclosures>,
    path: PathBuf,
    _tmp: tempfile::TempDir,
}

fn vault() -> Vault {
    vault_of("Notes.org", |store, root| {
        new_org_sync_controller(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            root,
            Arc::new(NoopOrdering),
            Arc::new(RealFileSystem),
        )
    })
}

fn logseq_vault() -> Vault {
    vault_of("Notes.md", |store, root| {
        let formats = FormatRegistry::new(vec![Arc::new(LogseqMarkdownAdapter::new())]).unwrap();
        FileSyncController::with_formats(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            root,
            Arc::new(formats),
            Arc::new(NoopOrdering),
            Arc::new(RealFileSystem),
        )
    })
}

fn vault_of(file: &str, controller: impl FnOnce(&Store, PathBuf) -> FileSyncController) -> Vault {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let store = Store::default();
    let disclosures = Arc::new(Disclosures::default());
    let sync = controller(&store, root.clone()).with_writeback_disclosure(disclosures.clone());
    Vault {
        sync,
        store,
        disclosures,
        path: root.join(file),
        _tmp: tmp,
    }
}

fn assert_refused_by_name(v: &Vault, result: anyhow::Result<impl std::fmt::Debug>) {
    let err = format!("{:#}", result.expect_err("the file is refused"));
    assert!(err.contains("block:abc"), "the refusal names the id: {err}");
    let disclosed = v.disclosures.0.lock().unwrap().clone();
    assert!(
        disclosed
            .iter()
            .any(|d| d.contains("Notes.") && d.contains("block:abc")),
        "the file is disclosed as refused, naming the id: {disclosed:?}"
    );
    let ids: Vec<String> = v
        .store
        .0
        .lock()
        .unwrap()
        .keys()
        .map(|k| k.as_str().to_string())
        .collect();
    assert!(
        !ids.iter().any(|id| id.contains("block:block:")),
        "a document is keyed by a double-schemed id: {ids:?}"
    );
}

#[tokio::test]
async fn a_new_page_file_with_a_schemed_id_is_refused_by_name() {
    let mut v = vault();
    std::fs::write(&v.path, "#+ID: block:abc\n* A\n").unwrap();
    let path = v.path.clone();
    let result = v.sync.on_file_changed(&path).await;
    assert_refused_by_name(&v, result);
}

#[tokio::test]
async fn a_tracked_page_file_edited_to_a_schemed_id_is_refused_by_name() {
    let mut v = vault();
    let mut page = Block::new_text(EntityUri::block("abc"), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let kid = Block::new_text(EntityUri::block("kid"), page.id.clone(), "A");
    v.store
        .0
        .lock()
        .unwrap()
        .insert(page.id.clone(), (page, vec![kid]));
    v.sync
        .materialize_missing_page_files()
        .await
        .unwrap_or_else(|e| panic!("{e:#}"));
    assert!(
        std::fs::read_to_string(&v.path)
            .unwrap()
            .contains("#+ID: abc"),
        "the page's file is written and tracked"
    );
    let path = v.path.clone();
    std::fs::write(&v.path, "#+ID: block:abc\n* A\n").unwrap();
    let result = v.sync.on_file_changed(&path).await;
    assert_refused_by_name(&v, result);
}

#[tokio::test]
async fn a_logseq_page_whose_id_names_a_scheme_is_refused_by_name() {
    let mut v = logseq_vault();
    std::fs::write(&v.path, "id:: block:abc\n\n- A\n").unwrap();
    let path = v.path.clone();
    let result = v.sync.on_file_changed(&path).await;
    assert_refused_by_name(&v, result);
}
