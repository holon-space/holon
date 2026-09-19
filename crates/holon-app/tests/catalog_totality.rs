//! Compilation totality over the catalog a live block actually advertises.
//!
//! The sibling lock in `holon-net` reads the macro-generated descriptors
//! directly; this one reads `BackendEngine::derived_net`, so a hand-written
//! descriptor that drops a declaration the trait makes reds here and nowhere
//! else. The census body is `eprintln!`-only on purpose: it is a measurement
//! for a reader of the log, and the assertions below it are what gate.

use std::collections::BTreeMap;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_net::Analyzability;
use holon_net::CompiledNet;
use holon_net::TransitionKey;
use holon_net::TransitionSource;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

/// The write ops whose declarations ADR 0032 admits to the analyzable set.
const ANALYZABLE_BLOCK_OPS: &[&str] = &[
    "create",
    "delete",
    "move_block",
    "split_block",
    "join_block",
];

/// Production SqlOnly block wiring, identical to the
/// `arc_affects_consistency` and `authority_place_reservation` precedents —
/// so all three oracles read the catalog a live block's profile carries.
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

/// Every block operation transition of `net`, keyed by op name.
fn block_transitions(net: &CompiledNet) -> BTreeMap<&str, &Analyzability> {
    net.transitions
        .iter()
        .filter_map(|t| match &t.source {
            TransitionSource::Operation { entity, op } if entity.to_string() == BLOCK => {
                Some((op.as_str(), &t.analyzability))
            }
            _ => None,
        })
        .collect()
}

fn census_table(net: &CompiledNet) -> String {
    let rows = block_transitions(net);
    let analyzable = rows
        .values()
        .filter(|a| matches!(a, Analyzability::Analyzable))
        .count();
    let mut out = format!(
        "[catalog-census] {} advertised block ops: {analyzable} analyzable, {} unanalyzable\n",
        rows.len(),
        rows.len() - analyzable
    );
    out.push_str("[catalog-census] op | verdict | undeclared halves\n");
    for (op, analyzability) in rows {
        let (verdict, halves) = match analyzability {
            Analyzability::Analyzable => ("Analyzable".to_string(), String::new()),
            Analyzability::Unanalyzable { undeclared } => (
                "Unanalyzable".to_string(),
                undeclared
                    .iter()
                    .map(|h| format!("{h:?}"))
                    .collect::<Vec<_>>()
                    .join("+"),
            ),
        };
        out.push_str(&format!("[catalog-census] {op} | {verdict} | {halves}\n"));
    }
    out
}

/// The conflict and cycle analyses reduced to what a before/after comparison
/// needs: a new cycle or a new contended place is a finding about the
/// declarations, not about this test.
fn analysis_summary(net: &CompiledNet) -> String {
    let conflicts = holon_net::conflicts(net);
    let cycles = holon_net::cycles(net);
    let mut out = format!(
        "[catalog-census] conflicts: {} contended places, {} unanalyzable\n",
        conflicts.contentions.len(),
        conflicts.unanalyzable.len()
    );
    for contention in &conflicts.contentions {
        out.push_str(&format!(
            "[catalog-census] contention {} writers={:?} readers={:?}\n",
            contention.place,
            contention
                .writers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            contention
                .readers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        ));
    }
    out.push_str(&format!(
        "[catalog-census] cycles: {} findings\n",
        cycles.cycles.len()
    ));
    for finding in &cycles.cycles {
        out.push_str(&format!(
            "[catalog-census] cycle {:?}\n",
            finding
                .transitions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        ));
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn block_catalog_analyzability_census() {
    let engine = block_engine().await;
    let net = engine
        .derived_net()
        .expect("the advertised catalog compiles");

    let rows = block_transitions(&net);
    assert!(
        !rows.is_empty(),
        "no block operation reached the net — an empty census would report \
         perfect analyzability"
    );

    eprint!("{}", census_table(&net));
    eprint!("{}", analysis_summary(&net));
}

/// The five write ops declare a marking delta on their trait methods, so a
/// descriptor that advertises them as unanalyzable is dropping a declaration
/// the tree already makes.
#[tokio::test(flavor = "multi_thread")]
async fn the_block_write_ops_compile_analyzable() {
    let engine = block_engine().await;
    let net = engine
        .derived_net()
        .expect("the advertised catalog compiles");

    for op in ANALYZABLE_BLOCK_OPS {
        let key = TransitionKey::operation(BLOCK, op).expect("a dotless entity");
        let transition = net
            .transition(&key)
            .unwrap_or_else(|| panic!("the block catalog advertises {op}"));
        assert_eq!(
            transition.analyzability,
            Analyzability::Analyzable,
            "{op} is advertised unanalyzable; its trait method declares both halves"
        );
    }
}

/// `set_field` declares `block.sort_key` EXCLUDED — the ordering authority
/// mints order keys — and the SQL authority confirms it never writes one. The
/// marking delta's `structural` aspect nonetheless lowers onto both placement
/// columns, so without subtraction at compile time the advertised net would
/// report a write that cannot happen, and an enabledness evaluator reading it
/// would order against that write.
#[tokio::test(flavor = "multi_thread")]
async fn the_excluded_order_key_is_not_a_declared_write_of_set_field() {
    let engine = block_engine().await;
    let net = engine
        .derived_net()
        .expect("the advertised catalog compiles");
    let report = holon_net::conflicts(&net);

    let sort_key = report
        .contentions
        .iter()
        .find(|c| c.place.to_string() == "block.sort_key")
        .expect("several block ops write the order key, so it is contended");
    let writers: Vec<String> = sort_key.writers.iter().map(ToString::to_string).collect();

    assert!(
        !writers.iter().any(|w| w.ends_with(".set_field")),
        "set_field excludes block.sort_key, yet the advertised net lists it as a \
         writer: {writers:?}"
    );
    assert!(
        writers.iter().any(|w| w.ends_with(".move_block")),
        "the subtraction must remove only the excluded place — move_block writes the \
         order key and must still appear: {writers:?}"
    );
}
