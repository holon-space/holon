//! The row a no-Turso session renders for a block (`LoroBlockQuerySource` →
//! `block_to_row`) against the `block` matview row the Loro→SQL projection
//! writes for the same block. Both rows are enriched the way the reactive
//! engine enriches them, with the Turso-free resolver's computed fields.
//!
//! @pbt kind harness
//! @pbt covers loro-ui-row-sql-parity — every consumer-visible field of the
//!   Loro UI row equals the SQL row's, list-valued fields as sets and
//!   `sort_key` as sibling order

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::EdgeField;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::entity_profile::ProfileResolving;
use holon_api::lifecycle::DEFAULT_SHUTDOWN_TIMEOUT;
use holon_api::lifecycle::SessionShutdown;
use holon_api::widget_spec::EnrichedRow;
use holon_core::OperationProvider;
use holon_core::storage::BlockQuery;
use holon_core::storage::BlockQuerySource;
use holon_loro::DocScope;
use holon_loro::LoroBackend;
use holon_loro::block_to_params;
use holon_loro::loro_block_operations::LoroBlockOperations;
use holon_loro::loro_document_store::LoroDocumentStore;
use holon_loro_wiring::loro_block_query_source::LoroBlockQuerySource;
use holon_loro_wiring::loro_ui_watcher::block_to_row;
use holon_loro_wiring::loro_ui_watcher::build_turso_free_profile_resolver;
use holon_loro_wiring::loro_ui_watcher::sibling_index;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::LinkSchemaModule;
use tokio::sync::RwLock;

/// Columns only the SQL row carries, each with why the Loro row must not.
const SQL_ONLY: &[(&str, &str)] = &[
    (
        "write_seq",
        "SQL write-ordering stamp; no profile or render expression reads it",
    ),
    (
        "_change_origin",
        "CDC provenance of the SQL write; no consumer reads it from a row",
    ),
    (
        "property_kinds",
        "storage-internal encoding of `properties`, already applied to it",
    ),
    (
        "block_type",
        "Loro keeps it in node meta outside `Block`; no shipped profile reads it",
    ),
    (
        "completed",
        "Loro keeps it in node meta outside `Block`; no shipped profile reads it",
    ),
];

/// Columns both rows carry whose values differ by construction.
const VALUE_EXEMPT: &[(&str, &str)] = &[(
    "updated_at",
    "the projection stamps its own write time; no profile or render expression reads it",
)];

/// Order-free edge sets, stored as JSON text arrays in both rows.
fn as_set(column: &str, value: &Value) -> BTreeSet<String> {
    let Value::String(json) = value else {
        panic!("{column} is not JSON text: {value:?}");
    };
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_else(|e| panic!("{column} is not a JSON string array ({json}): {e}"))
        .into_iter()
        .collect()
}

fn create_params(fields: &[(&str, Value)]) -> StorageEntity {
    fields
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

fn strings(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::String(s.to_string())).collect())
}

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

/// Blocks in parent-before-child order, each built by a production `create`.
fn fixture() -> Vec<StorageEntity> {
    let root = EntityUri::no_parent().to_string();
    vec![
        create_params(&[
            ("id", text("block:doc")),
            ("parent_id", text(&root)),
            ("content", text("Doc")),
            ("tags", strings(&["Page"])),
        ]),
        create_params(&[
            ("id", text("block:first")),
            ("parent_id", text("block:doc")),
            ("content", text("first block")),
            ("tags", strings(&["zeta", "decision", "alpha"])),
            ("requires", strings(&["block:third", "block:second"])),
            ("contributes_to", strings(&["block:second"])),
            ("advice_suppressed", strings(&["block:third", "block:doc"])),
            ("properties", text(r#"{"custom":"x","expand_default":1}"#)),
            (
                "marks",
                text(&holon_api::marks_to_json(&[holon_api::MarkSpan::new(
                    0,
                    5,
                    holon_api::InlineMark::Bold,
                )])),
            ),
        ]),
        create_params(&[
            ("id", text("block:second")),
            ("parent_id", text("block:doc")),
            ("content", text("second")),
            ("task_state", text("TODO")),
            ("collapsed", Value::Boolean(true)),
            (
                "properties",
                text(r#"{"priority":3,"effort":"2h","completed":"no","block_type":"memo"}"#),
            ),
        ]),
        create_params(&[
            ("id", text("block:third")),
            ("parent_id", text("block:doc")),
            ("content", text("from children")),
            ("content_type", text("source")),
            ("source_language", text("holon_prql")),
            ("source_name", text("q")),
        ]),
        create_params(&[
            ("id", text("block:nested")),
            ("parent_id", text("block:first")),
            ("content", text("nested")),
            ("widget_only", Value::Boolean(true)),
            ("tags", strings(&["decision"])),
        ]),
    ]
}

async fn sql_provider() -> (SqlOperationProvider, holon::storage::turso::DbHandle) {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("turso");
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FKs");
    CoreSchemaModule.ensure_schema(&handle).await.expect("core");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block");
    BlockMatviewSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block matview");
    LinkSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("links");
    let provider = SqlOperationProvider::with_edge_fields(
        handle.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
    );
    (provider, handle)
}

fn enrich(resolver: &Arc<dyn ProfileResolving>, row: StorageEntity) -> BTreeMap<String, Value> {
    EnrichedRow::from_storage(row, |r| resolver.resolve_with_computed(r).1)
        .into_inner()
        .into_iter()
        .collect()
}

#[tokio::test]
async fn loro_ui_row_matches_the_sql_block_row() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = LoroDocumentStore::new(tmp.path().to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.expect("global doc");
    let backend = Arc::new(LoroBackend::from_document(doc));
    let ops = LoroBlockOperations::new(Arc::new(RwLock::new(store)));
    let entity: EntityName = "block".to_string().into();

    let fixture = fixture();
    let ids: Vec<EntityUri> = fixture
        .iter()
        .map(|p| EntityUri::parse(p["id"].as_string().unwrap()).unwrap())
        .collect();
    for params in &fixture {
        ops.execute_operation(&entity, "create", params.clone())
            .await
            .unwrap_or_else(|e| panic!("Loro create {params:?}: {e}"));
    }

    // SQL leg: the projection's own read of the Loro doc, written through the
    // production SQL create, read back from the `block` matview.
    let (provider, handle) = sql_provider().await;
    let projected: HashMap<EntityUri, _> = backend
        .projected_blocks()
        .expect("projected blocks")
        .into_iter()
        .map(|snap| (snap.block.id.clone(), snap))
        .collect();
    for id in &ids {
        provider
            .execute_operation(&entity, "create", block_to_params(&projected[id]))
            .await
            .unwrap_or_else(|e| panic!("SQL create {id}: {e}"));
    }
    let sql_rows: HashMap<EntityUri, StorageEntity> = handle
        .query("SELECT * FROM block", HashMap::new())
        .await
        .expect("block matview read")
        .into_iter()
        .map(|row| {
            let id = EntityUri::parse(row["id"].as_string().unwrap()).unwrap();
            (id, row)
        })
        .collect();

    // Loro leg: the no-Turso read path.
    let source: Arc<dyn BlockQuerySource> = Arc::new(LoroBlockQuerySource::new(backend.clone()));
    let snapshot = source.snapshot().await.expect("Loro snapshot");
    let shutdown = SessionShutdown::new();
    let resolver = build_turso_free_profile_resolver(source.clone(), &shutdown);
    shutdown
        .shutdown(DEFAULT_SHUTDOWN_TIMEOUT)
        .await
        .expect("the resolver's refresh task stops");

    let sql_only: BTreeSet<&str> = SQL_ONLY.iter().map(|(c, _)| *c).collect();
    let value_exempt: BTreeSet<&str> = VALUE_EXEMPT.iter().map(|(c, _)| *c).collect();
    let list_valued: BTreeSet<&str> = EdgeField::ALL.iter().map(|f| f.column()).collect();
    let mut diffs: Vec<String> = Vec::new();

    for id in &ids {
        let block = snapshot
            .block_by_id(id)
            .unwrap_or_else(|| panic!("{id} absent from the Loro snapshot"));
        let loro = enrich(
            &resolver,
            block_to_row(&block, sibling_index(&snapshot, &block)),
        );
        let sql = enrich(
            &resolver,
            sql_rows
                .get(id)
                .unwrap_or_else(|| panic!("{id} absent from the block matview"))
                .clone(),
        );

        for column in sql.keys().filter(|c| !loro.contains_key(*c)) {
            if !sql_only.contains(column.as_str()) {
                diffs.push(format!(
                    "{id}: `{column}` only in the SQL row: {:?}",
                    sql[column]
                ));
            }
        }
        for column in loro.keys().filter(|c| !sql.contains_key(*c)) {
            diffs.push(format!(
                "{id}: `{column}` only in the Loro row: {:?}",
                loro[column]
            ));
        }
        for (column, loro_value) in &loro {
            let Some(sql_value) = sql.get(column) else {
                continue;
            };
            if column == "sort_key" || value_exempt.contains(column.as_str()) {
                continue;
            }
            let equal = if list_valued.contains(column.as_str()) {
                as_set(column, loro_value) == as_set(column, sql_value)
            } else {
                loro_value == sql_value
            };
            if !equal {
                diffs.push(format!(
                    "{id}: `{column}` Loro {loro_value:?} != SQL {sql_value:?}"
                ));
            }
        }
    }

    // `sort_key` encodes sibling order in each adapter's own key space (ADR
    // 0005), so the rows agree when each parent's children sort the same way.
    let parents: BTreeSet<EntityUri> = ids
        .iter()
        .map(|id| snapshot.block_by_id(id).unwrap().parent_id)
        .collect();
    for parent in &parents {
        let children: Vec<EntityUri> = ids
            .iter()
            .filter(|id| snapshot.block_by_id(id).unwrap().parent_id == *parent)
            .cloned()
            .collect();
        let order_by = |key: &dyn Fn(&EntityUri) -> String| {
            let mut sorted = children.clone();
            sorted.sort_by_key(|id| (key(id), id.to_string()));
            sorted
        };
        let loro_order = order_by(&|id| {
            let block = snapshot.block_by_id(id).unwrap();
            block_to_row(&block, sibling_index(&snapshot, &block))["sort_key"]
                .as_string()
                .unwrap()
                .to_string()
        });
        let sql_order = order_by(&|id| sql_rows[id]["sort_key"].as_string().unwrap().to_string());
        if loro_order != sql_order {
            diffs.push(format!(
                "children of {parent}: Loro sort_key order {loro_order:?} != SQL {sql_order:?}"
            ));
        }
    }

    assert!(
        diffs.is_empty(),
        "Loro UI row and SQL block row disagree:\n{}",
        diffs.join("\n")
    );
}
