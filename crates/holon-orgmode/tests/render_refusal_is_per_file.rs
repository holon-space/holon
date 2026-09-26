//! A render refused for one file is that file's failure alone: the batch
//! passes (`materialize_missing_page_files`, `re_render_all_tracked`) leave
//! the refused file untouched, disclose it with the id that no carrier holds,
//! and still write every other file of the pass.

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
struct ErrorCapture(Arc<Mutex<Vec<String>>>);

struct MsgVisitor<'a>(&'a mut String);
impl Visit for MsgVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        write!(self.0, "{}={:?} ", field.name(), value).unwrap();
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

/// A page and its blocks.
type Page = (Block, Vec<Block>);

/// Pages and their blocks, editable between passes.
#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<EntityUri, Page>>>);

impl Store {
    fn page(&self, id: &str, title: &str, kid_id: EntityUri, body: &str) {
        let mut page = Block::new_text(EntityUri::block(id), EntityUri::no_parent(), title);
        page.set_page(true);
        let kid = Block::new_text(kid_id, page.id.clone(), body);
        self.0
            .lock()
            .unwrap()
            .insert(page.id.clone(), (page, vec![kid]));
    }
}

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

fn controller(store: &Store, root: &std::path::Path) -> FileSyncController {
    new_org_sync_controller(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        root.to_path_buf(),
        Arc::new(NoopOrdering),
        Arc::new(RealFileSystem),
    )
}

/// The id no org carrier holds: its `:ID:` would read back as a new key.
fn unwritable() -> EntityUri {
    EntityUri::block(":bad")
}

fn assert_disclosed(cap: &ErrorCapture) {
    let errors = cap.0.lock().unwrap().clone();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("block::bad") && e.contains("Bad Page.org")),
        "the refused file must be disclosed with its id and path; captured: {errors:?}"
    );
}

#[tokio::test]
async fn the_page_sweep_writes_the_other_pages_when_one_render_is_refused() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::default();
    store.page("a-bad", "Bad Page", unwritable(), "bad body");
    store.page(
        "b-good",
        "Good Page",
        EntityUri::block("good-kid"),
        "good body",
    );

    controller(&store, tmp.path())
        .materialize_missing_page_files()
        .await
        .unwrap_or_else(|e| panic!("one refused page aborted the sweep: {e:#}"));

    let good = std::fs::read_to_string(tmp.path().join("Good Page.org"))
        .expect("the good page is written");
    assert!(good.contains("good body"), "{good}");
    assert!(!tmp.path().join("Bad Page.org").exists());
    assert_disclosed(&cap);
}

#[tokio::test]
async fn the_tracked_re_render_writes_the_other_files_when_one_render_is_refused() {
    let cap = ErrorCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::default();
    store.page("a-bad", "Bad Page", EntityUri::block("bad-kid"), "bad body");
    for n in 0..4 {
        store.page(
            &format!("good-{n}"),
            &format!("Good Page {n}"),
            EntityUri::block(&format!("good-kid-{n}")),
            "good body",
        );
    }
    let mut sync = controller(&store, tmp.path());
    sync.materialize_missing_page_files()
        .await
        .unwrap_or_else(|e| panic!("{e:#}"));
    let bad_before = std::fs::read_to_string(tmp.path().join("Bad Page.org")).unwrap();

    store.page("a-bad", "Bad Page", unwritable(), "bad body edited");
    for n in 0..4 {
        store.page(
            &format!("good-{n}"),
            &format!("Good Page {n}"),
            EntityUri::block(&format!("good-kid-{n}")),
            "good body edited",
        );
    }
    sync.re_render_all_tracked(&HashSet::new())
        .await
        .unwrap_or_else(|e| panic!("one refused file aborted the re-render: {e:#}"));

    for n in 0..4 {
        let good = std::fs::read_to_string(tmp.path().join(format!("Good Page {n}.org"))).unwrap();
        assert!(good.contains("good body edited"), "{good}");
    }
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("Bad Page.org")).unwrap(),
        bad_before,
        "the refused file is left untouched"
    );
    assert_disclosed(&cap);
}
