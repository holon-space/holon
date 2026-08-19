//! Copy-on-write default-layout seeding (F4 stale-seed remedy).
//!
//! Martin's ruling: the bundled default layout (`block:__default__`) is a
//! VIRTUAL seed doc — it re-seeds from the shipped asset on every boot and is
//! NEVER auto-materialized to a vault `.org` file. The FIRST user edit
//! materializes the file, and from then on that file WINS: `__default__.org`
//! on disk is the durable "user modified the seed layout" marker, so the seed
//! must neither create nor replace the default layout while the file exists.
//!
//! Boot-time materialization of `block:__default__` crept in via the Fork B B2
//! sweep (`feat(filesystem): B2 fileless-page materialization`, a92d7eb7,
//! 2026-07-12) and pinned the vault to a stale Jul-12 seed. This guards the
//! remedy at the seed boundary: `default_org_exists` gates the default-layout
//! seed/re-seed off, while the always-seeded journals machinery is untouched.
//!
//! @pbt kind harness
//! @pbt covers seed-copy-on-write — a present `__default__.org` (user-owned)
//! suppresses the default-layout seed/re-seed; the journals machinery still
//! seeds. Complements the reseed-mechanism unit test
//! (`holon-loro::reseed_content_refreshes_drift_and_is_churn_free`) and the
//! boot no-file guard in the frontend slice.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon::storage::BLOCK_READ_TABLE;
use holon_loro_wiring::EventInfraModule;

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

async fn has_block(engine: &holon::api::BackendEngine, id: &str) -> bool {
    !engine
        .db_handle()
        .query(
            &format!("SELECT id FROM {BLOCK_READ_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("query block presence")
        .is_empty()
}

/// No `__default__.org` on disk → the virtual default layout seeds normally
/// (`block:root-layout` lands). This is the auto-update / first-boot path.
#[test]
fn no_default_file_seeds_the_default_layout() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = build_engine(dir.path().join("a.db")).await;

        holon_app::seed_default_layout(
            &engine, ordering, false, /* default_org_exists */ false,
        )
        .await
        .expect("seed must complete");

        assert!(
            has_block(&engine, holon_api::ROOT_LAYOUT_BLOCK_ID).await,
            "with no __default__.org on disk, the default layout must seed (root-layout present)"
        );
    });
}

/// A present `__default__.org` (the user materialized / owns the layout) WINS:
/// the default layout is neither seeded nor replaced (`block:root-layout` and
/// `block:__default__` absent) — the org scan will ingest the on-disk file
/// instead. The always-on journals machinery still seeds.
#[test]
fn present_default_file_suppresses_default_layout_seed() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = build_engine(dir.path().join("b.db")).await;

        holon_app::seed_default_layout(
            &engine, ordering, false, /* default_org_exists */ true,
        )
        .await
        .expect("seed must complete");

        assert!(
            !has_block(&engine, holon_api::ROOT_LAYOUT_BLOCK_ID).await,
            "a present __default__.org must WIN — root-layout must NOT be seeded/replaced \
             (copy-on-write: the materialized user-owned layout is authoritative)"
        );
        assert!(
            !has_block(&engine, holon_api::DEFAULT_DOC_BLOCK_ID).await,
            "the __default__ page itself must NOT be re-seeded when its file wins"
        );
        assert!(
            has_block(&engine, holon_frontend::JOURNALS_PAGE_ID).await,
            "the journals machinery seeds regardless of the default-layout gate"
        );
    });
}
