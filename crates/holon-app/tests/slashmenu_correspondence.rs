//! Inc1 of the slash-menu ↔ registry correspondence lock.
//!
//! Feeds the REAL block operation catalog
//! (`BackendEngine::available_operations` — the SAME assembly the profile
//! resolver and MCP discovery use, now single-sourced through
//! `block_synthetic_descriptors`) through the production
//! `CommandProvider::build_command_items` with an id-only editor context, and
//! asserts the rendered slash menu equals EXACTLY the ops classified `Listed`
//! at their descriptor.
//!
//! This is the registry-fed oracle the hand-built single-op mirror in
//! `command_provider.rs` structurally could not be: with the full catalog, the
//! cross-op intent-filter regression (GPUI dogfood 2026-07-20, bug b — the menu
//! collapsed to only "Turn into page" because `convert_block_to_page` maps from
//! the universal `id`) makes this test RED (menu = {convert} ⊊ the Listed set).

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::MenuExposure;
use holon_api::OperationWiring;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_frontend::command_provider::CommandProvider;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

/// A backend engine with the production SqlOnly block wiring (CRUD authority
/// `SqlOperationProvider` + structural provider `SqlBlockOperations`), so
/// `available_operations("block")` returns the SAME catalog a live block's
/// profile carries: set_field/delete/cycle_task_state/… + indent/outdent/
/// move_up/move_down/embed_entity + the `convert_block_to_page` synthetic.
async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    BLOCK.to_string(),
                    BLOCK.to_string(),
                    descriptors,
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    BLOCK.to_string(),
                    BLOCK.to_string(),
                    descriptors,
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread runtime.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with block providers")
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_menu_equals_the_listed_ops_resolvable_from_id_context() {
    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;

    // Expected = ops classified `Listed` at their descriptor. (Every Listed
    // block op resolves at least its `id` from an id-only context, so it is
    // build_command_items-visible — the RHS needs no separate resolvability
    // filter here; a Listed op that could NOT resolve from `id` would surface
    // as a real gap.)
    let expected_listed: BTreeSet<String> = catalog
        .iter()
        .filter(|d| matches!(d.menu_exposure, MenuExposure::Listed))
        .map(|d| d.name.clone())
        .collect();

    // Actual = what the production command menu builder renders from the real
    // catalog with the id-only context the editor supplies.
    let wirings: Vec<OperationWiring> = catalog
        .iter()
        .cloned()
        .map(|d| d.to_default_wiring())
        .collect();
    let id_ctx: HashMap<String, Value> =
        [("id".into(), Value::String("block:probe".into()))].into();
    let menu: BTreeSet<String> = CommandProvider::build_command_items(&wirings, &id_ctx, "")
        .into_iter()
        .map(|item| item.id)
        .collect();

    assert_eq!(
        menu, expected_listed,
        "the slash menu must be EXACTLY the `Listed` ops resolvable from an \
         id-only context — no Listed op silently missing (bug b), and no \
         gesture/internal op leaking in"
    );

    // Non-vacuity + explicit regression witnesses for the dogfood bug: the
    // plain structural commands must all be present (they vanished when the
    // intent filter collapsed the menu to the single id-mapped op).
    for op in [
        "indent",
        "outdent",
        "move_up",
        "move_down",
        "delete",
        "convert_block_to_page",
    ] {
        assert!(
            menu.contains(op),
            "Listed command {op:?} must be in the slash menu; got {menu:?}"
        );
    }

    // `PickerBacked` ops (instantiate_template) are surfaced via the template
    // picker, never as a bare command — so they are never in this set.
    assert!(
        !menu.contains("instantiate_template"),
        "instantiate_template is PickerBacked and must not appear as a bare command"
    );
}
