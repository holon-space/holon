//! Fresh-DB headless boot smoke test guarding the "HistoryTables-class" DI
//! regression.
//!
//! Symptom (fresh-db GPUI boot): the gpui frontend builds its DI graph LAZILY,
//! and a fresh file-backed boot PANICKED in `seed_default_layout`. The
//! `BackendEngine` factory wires `TursoHistoryStore` (C2b) over the raw db
//! handle, but the factory never resolved the `DbReady<HistoryTables>` marker,
//! so the `block_history` table did not exist yet. The very first
//! history-recording op during seeding (`block` `create` through the engine)
//! then hit a missing table and unwound.
//!
//! Fix (`crates/holon/src/di/registration.rs`, around lines 564/580): the
//! `BackendEngine` factory now does
//! `inj.resolve_async::<DbReady<HistoryTables>>().await` before building the
//! engine, plus `.with_dependency::<DbReady<HistoryTables>>()` on the provider,
//! so the schema is materialized before any op records history.
//!
//! NOTE (verified 2026-07-17): the `BackendEngine` factory also calls
//! `resolve_eager_roots(all_schema_roots())` up front, and `all_schema_roots`
//! lists `AutomationsJournalView` — which `.with_dependency`s `HistoryTables`.
//! So in the current tree `block_history` is materialized at boot through TWO
//! paths (the explicit `DbReady<HistoryTables>` resolve AND the eager
//! `AutomationsJournalView` root). Reverting ONLY the registration.rs lines no
//! longer reproduces the panic on its own — the eager-roots path still creates
//! the table. This test therefore guards the OVERALL invariant: whatever boot
//! mechanism owns it, `block_history` MUST exist before `seed_default_layout`
//! records history. Removing every boot-time creator of the table makes this
//! test go red with `no such table: block_history` while `seeding block_history
//! op_group sequence` — the exact original regression signature.
//!
//! This test reproduces the lazy-DI construction path against a FRESH
//! FILE-BACKED (not `:memory:`) Turso DB and asserts boot reaches
//! `seed_default_layout` WITHOUT panic, then proves the seed actually ran its
//! history-recording ops to completion by reading back the root layout block.
//! It is HEADLESS — no gpui window; the DI graph construction (+ seed) is the
//! surface under test, not rendering. The SqlOnly wiring below routes every
//! seed block create through `engine.execute_operation("block", "create", ..)`
//! (create_in_tree returns false without Loro), and a genuine create reports an
//! `id` field delta — so history recording is actually reached during seed,
//! which is what makes this a real guard rather than a vacuous one.
//!
//! BugFunnel row 22 (PBT-audit open gap): a fresh-db boot regression that the
//! keystone PBT structurally cannot see (it does not exercise the gpui lazy-DI
//! factory graph on a fresh file DB).
//!
//! @pbt kind harness
//! @pbt covers fresh-db-boot-seed — lazy DI graph resolves
//! DbReady<HistoryTables> before seed_default_layout records history (BugFunnel
//! row 22) @pbt overlaps general_e2e_composed_pbt — kept: no fresh-file gpui
//! lazy-DI boot path in keystone

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

/// Boot a fresh file-backed DB through the SAME lazy-DI entry the gpui
/// frontend uses, in the minimal SqlOnly configuration `seed_default_layout`
/// needs. Returns the engine and the `BlockOrdering` the seed drives.
async fn boot_fresh_db(
    db_path: std::path::PathBuf,
) -> (
    Arc<holon::api::backend_engine::BackendEngine>,
    Arc<dyn holon_core::block_ordering::BlockOrdering>,
) {
    assert!(
        !db_path.exists(),
        "db file must not pre-exist — this is the FRESH-db regression path"
    );

    // Build the engine through the SAME lazy-DI entry the gpui frontend uses
    // (`create_backend_engine_with_extras`), configuring the minimal SqlOnly
    // module set that `seed_default_layout` needs:
    //   - `EventInfraModule` supplies `dyn BlockOrdering` (over the block matview)
    //     which the seed drives, plus the STRUCTURAL block ops.
    //   - a bare `SqlOperationProvider` for the `block` entity supplies the CRUD
    //     ops (create / set_field / delete). EventInfraModule alone advertises only
    //     structural ops, so the engine's operation-registry startup check rejects
    //     the wiring without this — same fix the SqlOnly embedder
    //     (frontends/holon-worker/src/lib.rs) applies.
    // In this SqlOnly config `BlockOrdering::create_in_tree` returns false, so each
    // seed block routes through `engine.execute_operation("block", "create", ..)` —
    // the history-recording path that fired the regression. Resolving
    // `dyn BlockOrdering` in the `extra_resolve` closure exercises the same graph
    // the FrontendSession factory walks.
    holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule.configure(injector).map_err(|e| {
                anyhow::anyhow!("configure EventInfraModule for fresh-db boot: {e}")
            })?;
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
    .expect("fresh-db lazy DI graph must build (BackendEngine + BlockOrdering) without error")
}

/// Fresh file-backed boot must resolve the lazy DI graph (including
/// `DbReady<HistoryTables>`) and run `seed_default_layout` to completion —
/// history-recording ops and all — WITHOUT panicking.
#[test]
fn fresh_db_boot_reaches_seed_without_history_tables_panic() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir for fresh db");
        let (engine, ordering) = boot_fresh_db(dir.path().join("fresh.db")).await;

        // The regression fires HERE: the first history-recording op during
        // seeding hit a missing `block_history` table before the fix.
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect(
                "seed_default_layout must complete on a fresh file DB — a panic/error here means \
                 the lazy DI factory did not resolve DbReady<HistoryTables> before history-\
                 recording ops ran (regression: block_history table missing)",
            );

        // Positive proof the seed actually RAN (not skipped): the root layout
        // block exists, which only happens after seed_default_layout drove its
        // history-recording create ops to completion.
        let db = engine.db_handle();
        let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID;
        let rows = db
            .query(
                &format!("SELECT id FROM {BLOCK_READ_TABLE} WHERE id = '{root_id}'"),
                HashMap::new(),
            )
            .await
            .expect("query root layout block after seed");
        assert!(
            !rows.is_empty(),
            "root layout block {root_id} must exist after seed_default_layout — seed did not run \
             its history-recording ops to completion"
        );
    });
}

/// The `graph_eav` schema (BG-5) must stay deleted: a production boot creates
/// NONE of its tables.
///
/// It was created at every boot and read by nothing — GQL resolves nodes and
/// edges over the TYPED entity tables instead (`MappedNodeResolver` /
/// `ForeignKeyEdgeResolver` in `crates/holon-turso/src/graph_schema.rs`). This
/// pins the absence against a re-introduction, which would otherwise be
/// invisible: no test would fail, the tables would simply reappear.
#[test]
fn production_boot_creates_no_graph_eav_tables() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir for fresh db");
        let (engine, _ordering) = boot_fresh_db(dir.path().join("fresh.db")).await;

        let names = [
            "nodes",
            "edges",
            "node_labels",
            "property_keys",
            "node_props_int",
            "node_props_text",
            "node_props_real",
            "node_props_bool",
            "node_props_json",
            "edge_props_int",
            "edge_props_text",
            "edge_props_real",
            "edge_props_bool",
            "edge_props_json",
        ];
        let in_list = names
            .iter()
            .map(|n| format!("'{n}'"))
            .collect::<Vec<_>>()
            .join(", ");

        let db = engine.db_handle();
        let rows = db
            .query(
                &format!("SELECT name FROM sqlite_master WHERE name IN ({in_list})"),
                HashMap::new(),
            )
            .await
            .expect("query sqlite_master for graph_eav tables");

        let found: Vec<String> = rows
            .iter()
            .filter_map(|r| match r.get("name") {
                Some(holon_api::Value::String(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(
            found.is_empty(),
            "graph_eav was deleted under BG-5, but a production boot created {found:?} — the \
             typed tables plus their per-type matviews are the model; do not reinstate the \
             generic EAV schema"
        );
    });
}
