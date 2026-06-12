//! A Turso-free [`BlockQuerySource`] backed directly by the Loro tree.
//!
//! This is the read seam ADR 0004 Phase 9 needs so a wiring *without* Turso can
//! still answer the block queries the PBT invariants depend on. It walks the
//! `LoroBackend` tree directly (`CoreOperations::list_children` / `get_blocks`)
//! — **not** the `QueryableCache`, which is a thin wrapper over a Turso
//! `DbHandle` and would re-introduce the Turso dependency we are removing.
//!
//! Per the redesigned seam, the *reads* are synchronous against a captured
//! [`BlockSnapshot`]; the only async concern is producing that snapshot. For
//! Loro there is no CDC to settle, so `snapshot()` just performs one DFS capture
//! of the tree. Sibling order is the Loro tree's own child order (the fractional
//! index the SQL projection would otherwise materialize as `sort_key`), so the
//! capture is in canonical sibling order (ADR 0005) without any SQL `ORDER BY`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::{Injector, Provider, Shared};
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_core::storage::block_query::Result;
use holon_core::storage::{BlockQuerySource, BlockSnapshot};
use tokio::sync::RwLock;

use holon_loro::LoroBackend;
use crate::api::operation_dispatcher::OperationDispatcher;
use holon_api::repository::CoreOperations;
use crate::api::{DispatchingOperationEngine, OperationEngine};
use holon_core::OperationProvider;
use crate::sync::loro_block_operations::LoroBlockOperations;
use crate::sync::loro_document_store::LoroDocumentStore;

/// Captures blocks straight from a [`LoroBackend`] tree, with no Turso
/// connection, into a [`BlockSnapshot`].
pub struct LoroBlockQuerySource {
    backend: Arc<LoroBackend>,
}

impl LoroBlockQuerySource {
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        Self { backend }
    }

    /// Append `parent`'s subtree to `out` in pre-order: each block immediately
    /// before its own subtree, siblings in canonical Loro order.
    fn collect_subtree<'a>(
        &'a self,
        parent: &'a EntityUri,
        out: &'a mut Vec<Block>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // `list_children` returns child URIs in Loro tree order; `get_blocks`
            // preserves that input order.
            let child_ids = self.backend.list_children(parent.as_str()).await?;
            let children = self.backend.get_blocks(child_ids).await?;
            for child in children {
                let child_id = child.id.clone();
                out.push(child);
                self.collect_subtree(&child_id, out).await?;
            }
            Ok(())
        })
    }
}

#[async_trait]
impl BlockQuerySource for LoroBlockQuerySource {
    async fn snapshot(&self) -> Result<BlockSnapshot> {
        let mut ordered = Vec::new();
        // `no_parent` resolves to the Loro tree roots (see `LoroBackend::list_children`).
        self.collect_subtree(&EntityUri::no_parent(), &mut ordered)
            .await?;

        // Navigation focus is a Turso matview today with no Loro-native source.
        // Under a Loro-only wiring `inv-focus-roots` drops (it reads that
        // matview), so empty focus-roots is the correct, disclosed behaviour for
        // this slice — see plan task V3. An in-memory nav source is deferred.
        Ok(BlockSnapshot::from_ordered(ordered, Vec::new()))
    }
}

/// Register `Arc<dyn BlockQuerySource>` resolving to a [`LoroBlockQuerySource`]
/// over `backend`. This is the **Loro arm** of the no-Turso `BlockQuerySource`
/// resolution rule (the Turso arm is `register_turso_block_query_source`): call
/// it from a `build_no_turso_container` `setup_fn` once a `LoroBackend` is
/// available, so `LoroMemory` assemblies resolve a Loro-native source.
pub fn register_loro_block_query_source(injector: &Injector, backend: Arc<LoroBackend>) {
    injector.provide::<dyn BlockQuerySource>(Provider::root(move |_| {
        Arc::new(LoroBlockQuerySource::new(backend.clone())) as Arc<dyn BlockQuerySource>
    }));
}

/// Register the prod Turso-free Loro read stack over `storage_dir`: opens (or
/// creates) the persistent `LoroDocumentStore`, then registers a `LoroDocumentStore`
/// provider plus a **synchronous** `Arc<dyn BlockQuerySource>` provider over the
/// store's global Loro doc. Returns the shared `Arc<RwLock<LoroDocumentStore>>` so
/// the caller can hand the *same* store to [`register_loro_operation_engine`]
/// (reads + writes then share one doc).
///
/// The async work (loading the doc from disk) happens **up front**, so the
/// registered `BlockQuerySource` is a sync provider — `block_query_frontend`
/// resolves it synchronously (its `FrontendSession`/`ReactiveEngine` assembly is
/// sync), so don't push async into the frontend resolve. This mirrors the test
/// arm [`register_loro_block_query_source`] (which takes a pre-built backend);
/// here we build that backend from the store's freshly-loaded global doc.
pub async fn register_loro_read_stack(
    injector: &Injector,
    storage_dir: std::path::PathBuf,
) -> Arc<RwLock<LoroDocumentStore>> {
    let doc_store = LoroDocumentStore::new(storage_dir);
    // Load/create the persistent global doc once, up front.
    let doc = doc_store
        .get_global_doc()
        .await
        .expect("register_loro_read_stack: get_global_doc failed");
    let backend = Arc::new(LoroBackend::from_document(doc));

    // A `LoroDocumentStore` clone shares the global-doc `Arc`, so the registered
    // store, the read backend, and the write store below all observe one doc.
    let store_for_provider = doc_store.clone();
    injector.provide::<LoroDocumentStore>(Provider::root(move |_| {
        Shared::new(store_for_provider.clone())
    }));
    injector.provide::<dyn BlockQuerySource>(Provider::root(move |_| {
        Arc::new(LoroBlockQuerySource::new(backend.clone())) as Arc<dyn BlockQuerySource>
    }));

    Arc::new(RwLock::new(doc_store))
}

/// Register the Loro-native operation capability (`Arc<dyn OperationEngine>`)
/// into a no-Turso container — the **write** counterpart of the read stack above
/// (ADR 0004 Phase 9, Stage 4). `doc_store` must be the *same* store (sharing the
/// global Loro doc `Arc`) the reads observe, so a mutation is immediately visible
/// to the next read snapshot.
///
/// The engine wraps a [`DispatchingOperationEngine`] over an
/// [`OperationDispatcher`] holding a cache-free [`LoroBlockOperations`] block
/// provider, giving a no-Turso session full block CRUD + undo/redo. It is built
/// once (the undo stack is per-session state) and handed out by clone.
pub fn register_loro_operation_engine(
    injector: &Injector,
    doc_store: Arc<RwLock<LoroDocumentStore>>,
) {
    let block_ops = LoroBlockOperations::new(doc_store);
    // Navigation ops (focus / back / forward / home) have no Turso substrate in
    // a Loro-only session; an in-memory provider keeps per-device focus history
    // so click / arrow / back-forward navigation dispatches succeed.
    let nav_ops = crate::navigation::InMemoryNavigationProvider::new();
    let dispatcher = OperationDispatcher::new(vec![
        Arc::new(block_ops) as Arc<dyn OperationProvider>,
        Arc::new(nav_ops) as Arc<dyn OperationProvider>,
    ]);
    let engine: Arc<dyn OperationEngine> =
        Arc::new(DispatchingOperationEngine::new(Arc::new(dispatcher)));
    injector.provide::<dyn OperationEngine>(Provider::root(move |_| engine.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::repository::Lifecycle;
    use crate::di::lifecycle::build_no_turso_container;
    use holon_api::BlockContent;
    use holon_core::storage::BlockQuery;

    async fn seed_backend() -> (Arc<LoroBackend>, EntityUri, Vec<EntityUri>, EntityUri) {
        let backend = LoroBackend::create_new("loro-query-source-test".to_string())
            .await
            .unwrap();

        // root
        //   first
        //   second
        //     grandchild
        //   third
        let root = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
            .await
            .unwrap();
        let first = backend
            .create_block(root.id.clone(), BlockContent::text("first"), None)
            .await
            .unwrap();
        let second = backend
            .create_block(root.id.clone(), BlockContent::text("second"), None)
            .await
            .unwrap();
        let grandchild = backend
            .create_block(second.id.clone(), BlockContent::text("grandchild"), None)
            .await
            .unwrap();
        let third = backend
            .create_block(root.id.clone(), BlockContent::text("third"), None)
            .await
            .unwrap();

        let ordered_children = vec![first.id.clone(), second.id.clone(), third.id.clone()];
        (Arc::new(backend), root.id, ordered_children, grandchild.id)
    }

    #[tokio::test]
    async fn children_ordered_matches_loro_sibling_order() {
        let (backend, root, expected, _gc) = seed_backend().await;
        let snap = LoroBlockQuerySource::new(backend).snapshot().await.unwrap();

        let children = snap.children_ordered(&root);
        let got: Vec<&str> = children.iter().map(|b| b.content_text()).collect();
        assert_eq!(got, vec!["first", "second", "third"]);

        let got_ids: Vec<EntityUri> = children.iter().map(|b| b.id.clone()).collect();
        assert_eq!(got_ids, expected);
    }

    #[tokio::test]
    async fn children_order_follows_loro_tree_not_insertion_order() {
        // Seed order is first, second, third. Move `third` to sit right after
        // `first` in the Loro tree. `children_ordered` must report the NEW tree
        // order — proving order is read from the LoroTree (the source of truth),
        // not from insertion/creation order or any Turso projection.
        let (backend, root, expected, _gc) = seed_backend().await;
        let (first, third) = (&expected[0], &expected[2]);

        backend
            .update_block_position(third.as_str(), root.as_str(), Some(first.as_str()))
            .await
            .unwrap();

        let snap = LoroBlockQuerySource::new(backend).snapshot().await.unwrap();
        let children = snap.children_ordered(&root);
        let got: Vec<&str> = children.iter().map(|b| b.content_text()).collect();
        assert_eq!(got, vec!["first", "third", "second"]);
    }

    #[tokio::test]
    async fn block_by_id_returns_some_then_none() {
        let (backend, root, expected, _gc) = seed_backend().await;
        let snap = LoroBlockQuerySource::new(backend).snapshot().await.unwrap();

        let found = snap.block_by_id(&expected[0]);
        assert_eq!(found.unwrap().content_text(), "first");

        assert!(snap.block_by_id(&root).is_some());

        // ALLOW(entity_uri_from_raw): test-fixture literal
        let missing = EntityUri::from_raw("block:does-not-exist");
        assert!(snap.block_by_id(&missing).is_none());
    }

    #[tokio::test]
    async fn descendants_ordered_is_preorder() {
        let (backend, root, _children, _gc) = seed_backend().await;
        let snap = LoroBlockQuerySource::new(backend).snapshot().await.unwrap();

        let descendants = snap.descendants_ordered(&root);
        let got: Vec<&str> = descendants.iter().map(|b| b.content_text()).collect();
        // Pre-order: parent immediately before its subtree, siblings in order.
        assert_eq!(got, vec!["first", "second", "grandchild", "third"]);
    }

    #[tokio::test]
    async fn focus_roots_is_empty_without_turso() {
        let (backend, _root, _children, _gc) = seed_backend().await;
        let snap = LoroBlockQuerySource::new(backend).snapshot().await.unwrap();
        assert!(snap.focus_roots().is_empty());
    }

    /// End-to-end Loro arm of Item 2: a `LoroMemory` container assembles with
    /// **zero Turso**, resolves `Arc<dyn BlockQuerySource>` wired to the real
    /// `LoroBlockQuerySource`, and reads a snapshot straight from the Loro tree.
    #[tokio::test]
    async fn no_turso_container_resolves_loro_block_query_source() {
        let (backend, root, expected, _gc) = seed_backend().await;

        let injector = build_no_turso_container(":memory:".into(), move |inj| {
            register_loro_block_query_source(inj, backend);
            Ok(())
        })
        .await
        .expect("Turso-free container must assemble");

        let source = injector.resolve::<dyn BlockQuerySource>();
        let snap = source.snapshot().await.expect("snapshot");

        // Reads come from the Loro tree, no Turso involved.
        let children = snap.children_ordered(&root);
        let got: Vec<&str> = children.iter().map(|b| b.content_text()).collect();
        assert_eq!(got, vec!["first", "second", "third"]);
        assert!(snap.block_by_id(&expected[0]).is_some());
    }

    /// The prod arm: `register_loro_read_stack` opens the store up front and
    /// registers a **sync** `BlockQuerySource` provider sharing that one global
    /// doc. Seed the store, then resolve the source synchronously and read it
    /// back — proving the prod-shaped (sync-resolve) assembly works.
    #[tokio::test]
    async fn no_turso_container_builds_loro_read_stack_from_store() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        let injector = build_no_turso_container(":memory:".into(), |_| Ok(()))
            .await
            .expect("Turso-free container must assemble");
        // Async work up front; registers sync providers.
        let store = register_loro_read_stack(&injector, dir).await;

        // Seed the SAME global doc the resolved source will read.
        let doc = store.read().await.get_global_doc().await.unwrap();
        let backend = LoroBackend::from_document(doc);
        let root = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
            .await
            .unwrap();
        backend
            .create_block(root.id.clone(), BlockContent::text("a"), None)
            .await
            .unwrap();
        backend
            .create_block(root.id.clone(), BlockContent::text("b"), None)
            .await
            .unwrap();

        // Sync resolve — the prod-correctness point of decision #3.
        let source = injector.resolve::<dyn BlockQuerySource>();
        let snap = source.snapshot().await.unwrap();
        let children = snap.children_ordered(&root.id);
        let got: Vec<&str> = children.iter().map(|b| b.content_text()).collect();
        assert_eq!(got, vec!["a", "b"]);
    }

    /// Persistence: a mutation through `LoroBlockOperations` (which calls
    /// `save_doc` → `LoroDocumentStore::save_all`) must survive reopening a fresh
    /// store over the same `storage_dir` — the file-backed no-Turso durability
    /// guarantee the prod wiring relies on.
    #[tokio::test]
    async fn loro_block_op_persists_across_store_reopen() {
        use holon_core::CrudOperations;
        use holon_api::Value;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        let child_id = {
            let doc_store = LoroDocumentStore::new(dir.clone());
            // `backend` and `ops` share this store's global doc (clone shares the Arc).
            let doc = doc_store.get_global_doc().await.unwrap();
            let backend = LoroBackend::from_document(doc);
            let root = backend
                .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
                .await
                .unwrap();
            let child = backend
                .create_block(root.id.clone(), BlockContent::text("before"), None)
                .await
                .unwrap();

            let ops = LoroBlockOperations::new(Arc::new(RwLock::new(doc_store)));
            ops.set_field(
                child.id.as_str(),
                "content",
                Value::String("persisted".into()),
            )
            .await
            .unwrap(); // save_doc → save_all writes the .loro snapshot to `dir`
            child.id.to_string()
        }; // drop everything — only the on-disk snapshot remains

        // Reopen a brand-new store over the same dir.
        let store2 = LoroDocumentStore::new(dir);
        let doc2 = store2.get_global_doc().await.unwrap();
        let backend2 = LoroBackend::from_document(doc2);
        let reloaded = backend2.get_block(&child_id).await.unwrap();
        assert_eq!(
            reloaded.content, "persisted",
            "the no-Turso mutation must persist across a store reopen"
        );
    }
}
