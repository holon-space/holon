//! The root slot while the boot seed is still writing the default layout.
//!
//! The seed writes the layout block by block, so the store passes through a
//! state where `block:root-layout` has a panel but that panel has no source or
//! render child yet. The test holds the real seed at exactly that point and
//! watches the root slot through the production `watch_ui`:
//! - seed pending: the root renders `loading`, never an `error` widget;
//! - seed settled over the same half-written store: the root renders the error,
//!   because now nothing more is coming;
//! - seed resumed: the root renders the synthesized layout.
//!
//! @pbt kind harness
//! @pbt covers root-slot-pending-while-seeding — a perspective whose panels
//! are not written yet renders as loading during the boot seed, as an error
//! only after it

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use fluxdi::Module;
use fluxdi::Provider;
use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::RenderExpr;
use holon_api::streaming::UiEvent;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result;
use holon_loro_wiring::EventInfraModule;
use tokio::sync::oneshot;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

async fn build_engine(
    db_path: std::path::PathBuf,
) -> (
    Arc<holon::api::BackendEngine>,
    Arc<dyn holon_core::block_ordering::BlockOrdering>,
) {
    holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure EventInfraModule: {e}"))?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
                |resolver| {
                    let db = resolver
                        .resolve::<dyn holon::di::DbHandleProvider>()
                        .handle();
                    Arc::new(holon::core::SqlOperationProvider::new(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            Ok(())
        },
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .expect("lazy DI graph must build (BackendEngine + BlockOrdering)")
}

/// Delegates to the real ordering, but holds the first create under
/// `hold_parent` until released — so the seed stops right after writing that
/// block and before writing its first child.
struct HoldingOrdering {
    inner: Arc<dyn BlockOrdering>,
    hold_parent: EntityUri,
    reached: Mutex<Option<oneshot::Sender<()>>>,
    release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait]
impl BlockOrdering for HoldingOrdering {
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<()> {
        self.inner.place(uri, parent_id, after_id).await
    }

    async fn prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        self.inner.prev_sibling(id).await
    }

    async fn next_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        self.inner.next_sibling(id).await
    }

    async fn first_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        self.inner.first_child(parent_id).await
    }

    async fn last_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        self.inner.last_child(parent_id).await
    }

    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
        new_id: &EntityUri,
        content: BlockContent,
        properties: &HashMap<String, holon_api::Value>,
        edges: &holon_api::BlockEdges,
    ) -> Result<bool> {
        if *parent_id == self.hold_parent {
            let reached = self.reached.lock().unwrap().take();
            if let Some(reached) = reached {
                reached.send(()).expect("the test waits for the hold");
                let release = self.release.lock().await.take().expect("one hold");
                release.await.expect("the test releases the hold");
            }
        }
        self.inner
            .create_in_tree(parent_id, after_id, new_id, content, properties, edges)
            .await
    }

    async fn reseed_content(&self, blocks: &[(EntityUri, String)]) -> Result<usize> {
        self.inner.reseed_content(blocks).await
    }

    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> Result<()> {
        self.inner.update_in_tree(params).await
    }

    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> Result<()> {
        self.inner.delete_in_tree(params).await
    }

    async fn children(&self, parent_id: &EntityUri) -> Result<Vec<EntityUri>> {
        self.inner.children(parent_id).await
    }
}

fn function_name(expr: &RenderExpr) -> &str {
    match expr {
        RenderExpr::FunctionCall { name, .. } => name,
        other => panic!("the root slot renders a function call, got {other:?}"),
    }
}

/// The next Structure event's render expression, skipping data batches.
async fn next_structure(watch: &mut holon_api::streaming::WatchHandle, what: &str) -> RenderExpr {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match watch.recv().await.expect("the root watch stays open") {
                UiEvent::Structure { render_expr, .. } => return render_expr,
                UiEvent::Data { .. } => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no root Structure event within 10 s: {what}"))
}

#[test]
fn root_slot_is_loading_while_the_seed_writes_the_layout() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = build_engine(dir.path().join("seed.db")).await;

        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let holding = Arc::new(HoldingOrdering {
            inner: ordering,
            hold_parent: EntityUri::block("default-left-sidebar"),
            reached: Mutex::new(Some(reached_tx)),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });

        let seed_running = engine.layout_seed().begin();
        let seed = tokio::spawn({
            let engine = engine.clone();
            async move { holon_app::seed_default_layout(&engine, holding, false, false).await }
        });
        tokio::time::timeout(Duration::from_secs(30), reached_rx)
            .await
            .expect("the seed reaches the left sidebar's first child within 30 s")
            .expect("the hold signals");

        let root = holon_api::root_layout_block_uri();
        let mut watch = holon::api::ui_watcher::watch_ui(engine.clone(), root)
            .await
            .expect("watch the root slot");

        let pending = next_structure(&mut watch, "initial render while seeding").await;
        assert_eq!(
            function_name(&pending),
            "loading",
            "while the seed is still writing the layout, a perspective whose panels have no \
             source or render child yet is pending, not an error; the root rendered {pending:?}"
        );

        drop(seed_running);
        let settled = next_structure(&mut watch, "re-render once the seed settles").await;
        assert_eq!(
            function_name(&settled),
            "error",
            "once the seed has settled, a perspective with no displayable panel is a real \
             error; the root rendered {settled:?}"
        );

        release_tx.send(()).expect("the seed waits for the release");
        seed.await
            .expect("seed task joins")
            .expect("seed completes");
        let layout = loop {
            let expr = next_structure(&mut watch, "layout after the seed resumes").await;
            if function_name(&expr) != "error" {
                break expr;
            }
        };
        assert_eq!(
            function_name(&layout),
            "if_space",
            "the fully seeded perspective synthesizes the responsive layout; got {layout:?}"
        );
    });
}
