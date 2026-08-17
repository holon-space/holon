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
use holon_loro::LoroBlockOperations;
use holon_loro::LoroDocumentStore;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::sync::RwLock as TokioRwLock;

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
        .filter(
            |d| matches!(d.menu_exposure, MenuExposure::Listed { surfaces } if surfaces.slash_menu),
        )
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
        "delete_subtree",
        "delete_keep_children",
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

/// Prod Loro-authority block wiring reproduction for the menu-uniqueness lock.
///
/// The desktop app under `loro:true` registers BOTH structural authorities for
/// `block`: `SqlBlockOperations` (EventInfraModule) AND `LoroBlockOperations`
/// (LoroModule). Both delegate their structural ops to the SAME
/// `__operations_block_operations::block_operations` descriptor set, so the
/// dispatcher's non-dedup union (`OperationDispatcher::operations`) advertises
/// indent / outdent / move_up / move_down / embed_entity TWICE. That
/// double-advertisement is knowingly tolerated at DISPATCH by
/// `STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST` (first-registered-wins routing is
/// unaffected), but WITHOUT presentation dedup it leaks a duplicate slash-menu
/// row per op (dogfood-round3 B1: "Delete"/"Cycle Task State"/structural ops
/// listed twice).
///
/// Only these two providers are registered (no bare `SqlOperationProvider`), so
/// block CRUD (delete / cycle_task_state / create / set_field) is advertised
/// exactly once by `LoroBlockOperations` — the ONLY duplicated ops are the
/// structural set, all of which are in the dispatcher allowlist, so
/// `available_operations` returns the dup'd catalog without tripping the
/// debug-build uniqueness assertion.
async fn block_engine_loro_authority() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            // Structural authority (EventInfraModule analogue).
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
            // Loro CRUD + structural authority (LoroModule analogue): re-advertises
            // the structural block ops, producing the double-advertisement.
            .with_operation_provider_factory(|_backend| {
                let dir = tempfile::tempdir().expect("loro tempdir");
                let path = dir.path().to_path_buf();
                // Keep the backing dir alive for the whole test (the provider is
                // never asked to WRITE here — only `operations()` is read — but
                // the store must not vanish mid-test).
                std::mem::forget(dir);
                let store = Arc::new(TokioRwLock::new(LoroDocumentStore::new(path)));
                Arc::new(LoroBlockOperations::new(store)) as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with dual (Sql+Loro) block structural authorities")
}

/// Inc2 of the slash-menu correspondence lock: registry ↔ menu UNIQUENESS.
///
/// The positive lock above asserts the menu CONTAINS exactly the `Listed` ops
/// (presence). It ran over a set, so it was structurally blind to a Listed op
/// appearing more than once — the dogfood-round3 B1 duplicate ("same op
/// double-advertised by two providers renders two identical menu rows"). This
/// asserts every rendered menu entry is unique by `(label, operation id)`, over
/// the prod Loro-authority wiring that actually produces the dup.
#[tokio::test(flavor = "multi_thread")]
async fn slash_menu_entries_are_unique_by_label_and_op_id() {
    let engine = block_engine_loro_authority().await;
    let catalog = engine.available_operations(BLOCK).await;

    let wirings: Vec<OperationWiring> = catalog
        .iter()
        .cloned()
        .map(|d| d.to_default_wiring())
        .collect();
    let id_ctx: HashMap<String, Value> =
        [("id".into(), Value::String("block:probe".into()))].into();
    let menu = CommandProvider::build_command_items(&wirings, &id_ctx, "");

    // No (label, op id) pair may appear more than once — a repeated pair is a
    // user-visible duplicate row.
    let mut pair_counts: HashMap<(String, String), usize> = HashMap::new();
    for item in &menu {
        *pair_counts
            .entry((item.label.clone(), item.id.clone()))
            .or_default() += 1;
    }
    let duplicates: BTreeSet<String> = pair_counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|((label, id), c)| format!("{id} ({label}) ×{c}"))
        .collect();
    assert!(
        duplicates.is_empty(),
        "slash menu has duplicate entries — an op advertised by multiple \
         providers must render exactly ONE row (presentation dedup, first-wins; \
         dispatch is unchanged): {duplicates:?}"
    );

    // Guard the fix's dedup KEY: we dedup by op id, never by label alone. If two
    // DIFFERENT op ids collide on a display label, that is a finding to report,
    // not something the dedup may silently merge — surface it loudly here.
    let mut label_to_ids: HashMap<String, BTreeSet<String>> = HashMap::new();
    for item in &menu {
        label_to_ids
            .entry(item.label.clone())
            .or_default()
            .insert(item.id.clone());
    }
    let label_collisions: BTreeSet<String> = label_to_ids
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(label, ids)| format!("{label:?} -> {ids:?}"))
        .collect();
    assert!(
        label_collisions.is_empty(),
        "distinct operations share a display label (report as a finding; the \
         menu dedup keys on op id, so these correctly remain separate rows): \
         {label_collisions:?}"
    );

    // Non-vacuity: the structural ops the two authorities BOTH advertise must
    // each still be present exactly once.
    for op in [
        "indent",
        "outdent",
        "move_up",
        "move_down",
        "embed_entity",
        "delete_subtree",
        "delete_keep_children",
    ] {
        let count = menu.iter().filter(|i| i.id == op).count();
        assert_eq!(
            count,
            1,
            "structural op {op:?} must render exactly once; got {count} \
             (menu ids: {:?})",
            menu.iter().map(|i| &i.id).collect::<Vec<_>>()
        );
    }
}

/// Inc3: every LISTED op must have a REACHABLE completion from the id-only
/// editor context — presence in the menu is not the same as being invocable.
///
/// The two locks above assert what the menu SHOWS. This one asserts what
/// happens when the user picks a row. An op whose remaining params the popup
/// cannot collect returns `PopupResult::NotActive`, which the frontends map to
/// `EditorAction::None` — indistinguishable from "no popup was open", so Enter
/// falls through to `split_block` and silently mangles the block instead of
/// running the command (Martin, GPUI dogfooding 2026-08-17: `/enti` + Enter
/// split the block and left the literal "/enti" behind).
///
/// Runs over the whole Listed set, not just `embed_entity`, so the next op
/// that grows an uncollectable param is caught at the descriptor.
#[tokio::test(flavor = "multi_thread")]
async fn every_listed_command_has_a_reachable_completion() {
    use holon_frontend::popup_menu::PopupProvider;
    use holon_frontend::popup_menu::PopupResult;

    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;
    let wirings: Vec<OperationWiring> = catalog
        .iter()
        .cloned()
        .map(|d| d.to_default_wiring())
        .collect();
    let id_ctx: HashMap<String, Value> =
        [("id".into(), Value::String("block:probe".into()))].into();

    let menu = CommandProvider::build_command_items(&wirings, &id_ctx, "");
    assert!(!menu.is_empty(), "non-vacuity: the menu must not be empty");

    let mut dead_ends = Vec::new();
    for item in &menu {
        let provider = CommandProvider::new(wirings.clone(), id_ctx.clone());
        match provider.on_select(item, "") {
            // Executes now, or opens the param picker — both are reachable.
            PopupResult::Execute { .. } | PopupResult::Updated => {}
            other => dead_ends.push((item.id.clone(), format!("{other:?}"))),
        }
    }

    assert!(
        dead_ends.is_empty(),
        "these Listed commands are shown but cannot be completed — selecting \
         them falls through to split_block instead of running: {dead_ends:?}"
    );
}

/// The specific dogfood witness: picking "Embed Entity" must open the target
/// picker (param collection), keyed on the descriptor declaring `target_uri`
/// as an entity reference rather than a bare string.
#[tokio::test(flavor = "multi_thread")]
async fn embed_entity_opens_the_target_picker() {
    use holon_frontend::command_provider::is_collecting_params;
    use holon_frontend::command_provider::param_search_entity;
    use holon_frontend::popup_menu::PopupItem;
    use holon_frontend::popup_menu::PopupProvider;
    use holon_frontend::popup_menu::PopupResult;

    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;
    let wirings: Vec<OperationWiring> = catalog
        .iter()
        .cloned()
        .map(|d| d.to_default_wiring())
        .collect();
    let id_ctx: HashMap<String, Value> =
        [("id".into(), Value::String("block:probe".into()))].into();

    let provider = CommandProvider::new(wirings, id_ctx);
    let item = PopupItem {
        id: "embed_entity".into(),
        label: "Embed Entity".into(),
        icon: None,
    };
    let result = provider.on_select(&item, "enti");

    assert!(
        matches!(result, PopupResult::Updated),
        "selecting Embed Entity must keep the popup open to collect \
         `target_uri`; got {result:?}"
    );
    assert!(
        is_collecting_params(&provider),
        "the provider must be in param collection after the pick"
    );
    assert_eq!(param_search_entity(&provider), Some(BLOCK.to_string()));
}
