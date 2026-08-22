//! `rehome_entity` driven through a REAL container.
//!
//! The provider resolves the shared `BlockOrdering` and dispatches `move_block`
//! through the router, so it only means anything against a built DI graph: a
//! test that handed it its own writer would be exercising a second write path
//! that production does not have.
//!
//! @pbt kind harness
//! @pbt covers rehome-entity-op — a leaf leaves the document that holds it, the
//! two homes are read on either side of the move, and the result states the
//! move's price
//! @pbt overlaps general_e2e_composed_pbt — the keystone transition drives the
//! same op; this pins the operation's own contract (refusals + priced result)
//! without a draw

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::Value;
use holon_loro_wiring::EventInfraModule;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime"),
    )
}

/// A container wired the way `fresh_db_boot_seed_smoke` wires one, plus the
/// re-home provider.
async fn engine(db_path: std::path::PathBuf) -> Arc<holon::api::BackendEngine> {
    let (engine, _) = holon::di::create_backend_engine_with_extras(
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
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root_async(
                |resolver| async move {
                    let registry = Arc::new(
                        holon_capability::registry::shipped_profiles().expect("profiles parse"),
                    );
                    Arc::new(holon_app::rehome_entity::RehomeEntityProvider::new(
                        resolver, registry,
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
    .expect("container builds");
    engine
}

async fn seed(engine: &holon::api::BackendEngine, id: &str, parent: &str, is_page: bool) {
    // Through the ops layer, not raw SQL: the container's cache reads the
    // `block` matview, so a raw INSERT is invisible until CDC propagates.
    let mut p: HashMap<Arc<str>, Value> = HashMap::new();
    p.insert(Arc::from("id"), Value::String(id.to_string()));
    p.insert(Arc::from("parent_id"), Value::String(parent.to_string()));
    p.insert(Arc::from("content"), Value::String(id.to_string()));
    p.insert(Arc::from("content_type"), Value::String("text".into()));
    p.insert(
        Arc::from("sort_key"),
        Value::String(format!("a{}", id.len())),
    );
    engine
        .execute_operation(
            &holon_api::EntityName::new("block"),
            "create",
            p,
            holon_api::operation_engine::OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("seed {id}: {e:#}"));
    if is_page {
        // Page-ness is a row in the `block_tags` junction, which
        // `block_is_page` reads directly — no matview propagation involved, and
        // this minimal wiring advertises no `add_tag`.
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO block_tags (block_id, tag) VALUES ('{id}', '{}')",
                    holon_api::PAGE_TAG
                ),
                vec![],
            )
            .await
            .expect("tag page");
    }
}

async fn parent_of(engine: &holon::api::BackendEngine, id: &str) -> String {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT parent_id FROM block_raw WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read parent");
    rows.into_iter()
        .next()
        .and_then(|r| {
            r.get("parent_id")
                .and_then(|v| v.as_string().map(str::to_string))
        })
        .expect("block present")
}

fn params(id: &str, target: &str) -> HashMap<Arc<str>, Value> {
    let mut p = HashMap::new();
    p.insert(Arc::from("id"), Value::String(id.to_string()));
    p.insert(Arc::from("target"), Value::String(target.to_string()));
    p
}

/// The move, with both homes READ on either side of it rather than taken from
/// the target parameter, and the price stated.
#[test]
fn a_leaf_leaves_its_document_and_the_result_prices_the_move() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(dir.path().join("t.db")).await;
        seed(&engine, "block:page", "sentinel:no_parent", true).await;
        seed(&engine, "block:leaf", "block:page", false).await;

        let result = engine
            .execute_operation(
                &holon_api::EntityName::new("block"),
                "rehome_entity",
                params("block:leaf", "holon-native"),
                holon_api::operation_engine::OpOrigin::User,
            )
            .await
            .expect("a leaf under a page can move home");

        assert_eq!(parent_of(&engine, "block:leaf").await, "sentinel:no_parent");
        let Some(Value::Object(facts)) = result.response else {
            panic!("the move must report its homes");
        };
        assert_eq!(facts.get("from"), Some(&Value::String("org".into())));
        assert_eq!(
            facts.get("to"),
            Some(&Value::String("holon-native".into())),
            "the TO home is read AFTER the move: {facts:?}"
        );
        assert!(
            matches!(facts.get("rehoming_cost"), Some(Value::Array(_))),
            "every move states its price: {facts:?}"
        );
    });
}

/// A childless top-level page is already at the root, so the re-parent moves
/// nothing. Reporting `Ok` there would tell a caller a move happened.
#[test]
fn a_move_that_changes_no_home_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(dir.path().join("t.db")).await;
        seed(&engine, "block:lonepage", "sentinel:no_parent", true).await;

        let err = engine
            .execute_operation(
                &holon_api::EntityName::new("block"),
                "rehome_entity",
                params("block:lonepage", "holon-native"),
                holon_api::operation_engine::OpOrigin::User,
            )
            .await
            .expect_err("nothing moved, so nothing may be reported as a move");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("block:lonepage") && msg.contains("already sits at the tree root"),
            "the refusal must name the block and why nothing can move: {msg}"
        );
    });
}

#[test]
fn a_non_leaf_is_refused_and_nothing_moves() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(dir.path().join("t.db")).await;
        seed(&engine, "block:page", "sentinel:no_parent", true).await;
        seed(&engine, "block:branchx", "block:page", false).await;
        seed(&engine, "block:twig", "block:branchx", false).await;

        let err = engine
            .execute_operation(
                &holon_api::EntityName::new("block"),
                "rehome_entity",
                params("block:branchx", "holon-native"),
                holon_api::operation_engine::OpOrigin::User,
            )
            .await
            .expect_err("a non-leaf must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("block:branchx") && msg.contains("leaf"),
            "the refusal must name the block and the leaf rule: {msg}"
        );
        assert_eq!(parent_of(&engine, "block:branchx").await, "block:page");
    });
}

#[test]
fn a_logseq_db_target_is_refused_before_anything_moves() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(dir.path().join("t.db")).await;
        seed(&engine, "block:page", "sentinel:no_parent", true).await;
        seed(&engine, "block:leaf", "block:page", false).await;

        let err = engine
            .execute_operation(
                &holon_api::EntityName::new("block"),
                "rehome_entity",
                params("block:leaf", "logseq-db"),
                holon_api::operation_engine::OpOrigin::User,
            )
            .await
            .expect_err("logseq-db cannot receive an entity");
        let msg = format!("{err:#}");
        assert!(msg.contains("creation") && msg.contains("kvs_writer"));
        assert_eq!(parent_of(&engine, "block:leaf").await, "block:page");
    });
}
