//! Containment for the block write ops' declared arcs, checked against the
//! real SQL authority rather than against a transcription.
//!
//! `arc_marking_equality` proves the same law for `set_field` over the
//! reference model. This file is its sibling for the ops the reference model
//! has no transition for, and it reads the production providers directly so
//! the answer comes from what the store did, not from what a model believes.
//!
//! Two exact observations, neither of which parses SQL:
//!
//! - **writes** — snapshot every `block_raw` column and every edge junction
//!   before and after the op, and treat a changed cell as a written place.
//!   Nothing is transcribed: a place the op touches shows up as a differing
//!   value and must be declared or the test reds.
//! - **reads** — two sources, unioned. The returned inverse carries the prior
//!   value of every place it restores, which is the convention
//!   `CrudOperations::set_field` states in prose. And the statements the op
//!   issued, captured from the span collector: that is what reaches a read
//!   performed by a NESTED op, whose own inverse the caller discards — a join
//!   deletes through `self.delete`, whose `capture_row` is invisible from
//!   outside.
//!
//! Four limits. The first three can only HIDE a read, never invent one. The
//! fourth is different in kind: it is a way the whole read half could go
//! green having checked nothing, so it is guarded rather than tolerated —
//! `observe` fails the run instead of reporting.
//!
//! - an op may read a place it neither captures nor names in a statement;
//! - the `sql` span attribute is a fingerprint that truncates the middle of any
//!   statement over 440 characters, so a long projection is partly unreadable
//!   here;
//! - **only the SQL authority is covered.** The Loro leg writes and reads
//!   through `LoroBlockOperations` and the outbound projector, and nothing in
//!   this file observes it. A place only that leg touches would pass unseen.
//! - GUARDED: the statement capture depends on the test's runtime scope
//!   reaching the Turso actor thread. When that wiring breaks the collector
//!   returns nothing and the read half would pass on no evidence, so `observe`
//!   fails loudly on an empty capture rather than reporting.
//!
//! The write side is exact for everything the two snapshots can see.
//!
//! @pbt kind oracle
//! @pbt covers arc-truthfulness-sql — declared reads/emits contain what the
//! SQL authority actually reads and writes

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::TransitionArcs;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::UndoAction;
use holon_core::storage::types::StorageEntity;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

/// Columns `block_raw` carries that are NOT declarable arc places
/// (`arc_place: false` in `holon_api::schema::BLOCK`). A change in one of them
/// cannot be declared, so it cannot be demanded.
fn undeclarable_columns() -> BTreeSet<&'static str> {
    holon_api::schema::BLOCK
        .fields
        .iter()
        .filter(|f| !f.arc_place)
        .map(|f| f.name)
        .collect()
}

struct Harness {
    handle: DbHandle,
    blocks: Arc<SqlBlockOperations>,
    crud: Arc<SqlOperationProvider>,
    _backend: TursoBackend,
}

async fn harness() -> Harness {
    let (backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FK pragma");
    holon_turso::schema_modules::CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("CoreSchemaModule");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("BlockSchemaModule");
    BlockMatviewSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("BlockMatviewSchemaModule");
    holon_turso::schema_modules::LinkSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("LinkSchemaModule");

    let crud = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        BLOCK.to_string(),
        BLOCK.to_string(),
        BlockSchemaModule.edge_fields(),
    ));
    let mut block_raw_type_def = Block::type_definition();
    block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
    let cache = Arc::new(
        QueryableCache::<Block>::new(handle.clone(), block_raw_type_def)
            .await
            .expect("block_raw cache"),
    );
    let blocks = Arc::new(SqlBlockOperations::new(crud.clone(), cache));
    Harness {
        handle,
        blocks,
        crud,
        _backend: backend,
    }
}

/// Every observable cell of block state: one entry per `block_raw` row/column,
/// plus one per edge-junction membership.
type Snapshot = BTreeMap<(String, String), String>;

async fn snapshot(handle: &DbHandle) -> Snapshot {
    let mut out: Snapshot = BTreeMap::new();
    let rows = handle
        .query(
            &format!("SELECT * FROM {BLOCK_WRITE_TABLE}"),
            HashMap::new(),
        )
        .await
        .expect("read every block row");
    for row in rows {
        let id = row
            .get("id")
            .and_then(|v| v.as_string().map(str::to_string))
            .expect("every row carries an id");
        for (column, value) in &row {
            out.insert((id.clone(), column.to_string()), format!("{value:?}"));
        }
    }
    for edge in BlockSchemaModule.edge_fields() {
        let rows = handle
            .query(
                &format!(
                    "SELECT {}, {} FROM {}",
                    edge.source_col, edge.target_col, edge.join_table
                ),
                HashMap::new(),
            )
            .await
            .expect("read an edge junction");
        let mut members: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for row in rows {
            let source = row
                .get(edge.source_col.as_str())
                .and_then(|v| v.as_string().map(str::to_string))
                .expect("a junction row names its source");
            let target = row
                .get(edge.target_col.as_str())
                .and_then(|v| v.as_string().map(str::to_string))
                .unwrap_or_default();
            members.entry(source).or_default().insert(target);
        }
        for (source, targets) in members {
            out.insert(
                (source, edge.field.clone()),
                targets.into_iter().collect::<Vec<_>>().join(","),
            );
        }
    }
    out
}

/// The places whose cells differ between the two snapshots. A row appearing or
/// vanishing is a change to `block.id` as well as to each of its cells.
fn written_places(before: &Snapshot, after: &Snapshot) -> BTreeSet<String> {
    let undeclarable = undeclarable_columns();
    let mut places = BTreeSet::new();
    let ids_before: BTreeSet<&String> = before.keys().map(|(id, _)| id).collect();
    let ids_after: BTreeSet<&String> = after.keys().map(|(id, _)| id).collect();
    if ids_before != ids_after {
        places.insert("block.id".to_string());
    }
    for key in before.keys().chain(after.keys()) {
        if before.get(key) == after.get(key) {
            continue;
        }
        let (_, column) = key;
        if undeclarable.contains(column.as_str()) {
            continue;
        }
        places.insert(format!("block.{column}"));
    }
    places
}

/// The places the returned inverse restores, restricted to those the subject
/// already held before the op ran.
///
/// The restriction is what makes this read evidence rather than a guess: a
/// param naming a place the subject had no value for is a value the op MINTED
/// (a `create`'s inverse names the id it just allocated), not one it read.
fn places_restored_by_the_inverse(undo: &UndoAction, before: &Snapshot) -> BTreeSet<String> {
    let UndoAction::Undo(operation) = undo else {
        return BTreeSet::new();
    };
    let declarable: BTreeSet<String> = holon_api::schema::BLOCK
        .fields
        .iter()
        .filter(|f| f.arc_place)
        .map(|f| f.name.to_string())
        .collect();
    let Some(subject) = operation
        .params
        .get("id")
        .and_then(|v| v.as_string().map(str::to_string))
    else {
        return BTreeSet::new();
    };
    operation
        .params
        .keys()
        .filter(|key| declarable.contains(&key.to_string()))
        .filter(|key| before.contains_key(&(subject.clone(), key.to_string())))
        .map(|key| format!("block.{key}"))
        .collect()
}

/// Whether `column` occurs in `sql` as a whole identifier rather than as part
/// of a longer one — `content` must not match inside `content_type`.
fn names_column(sql: &str, column: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let bytes = sql.as_bytes();
    let mut from = 0;
    while let Some(hit) = sql[from..].find(column) {
        let start = from + hit;
        let end = start + column.len();
        let before_ok = start == 0 || !word(bytes[start - 1] as char);
        let after_ok = end == sql.len() || !word(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// Whether the statement's projection is a bare `*`. `COUNT(*)` is not a
/// projection of every column and must not expand to one.
fn projects_every_column(sql: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    let Some(select) = upper.find("SELECT") else {
        return false;
    };
    let rest = &upper[select + "SELECT".len()..];
    let projection = match rest.find(" FROM ") {
        Some(end) => &rest[..end],
        None => rest,
    };
    projection
        .split(',')
        .any(|term| term.trim() == "*" || term.trim().ends_with(".*"))
}

/// Every SQL statement the op issued, as the span collector recorded it.
///
/// The `sql` span attribute is a FINGERPRINT: statements over 440 characters
/// keep only their head and tail. That truncation can only HIDE a column, so
/// this evidence under-counts reads and never invents one.
fn sql_statements() -> Vec<String> {
    // Teeth probe: simulate the capture failing, to prove the guard below
    // actually reds rather than the oracle passing on no evidence.
    if std::env::var("HOLON_ARC_ORACLE_DROP_SQL").is_ok() {
        return Vec::new();
    }
    ["query", "execute"]
        .into_iter()
        .flat_map(|name| SpanCollector::global().spans_named(name))
        .filter_map(|span| {
            span.attributes
                .iter()
                .find(|kv| kv.key.as_str() == "sql")
                .map(|kv| kv.value.to_string())
        })
        .collect()
}

/// The places a captured statement proves the op read.
///
/// This is what reaches the reads an op performs through a NESTED op — the
/// `capture_row` / `capture_edges` pair a delete runs to build its inverse is
/// invisible from outside, because the caller discards that inverse and
/// returns its own.
fn places_read_by_sql(statements: &[String]) -> BTreeSet<String> {
    let stored: Vec<&str> = holon_api::schema::BLOCK
        .fields
        .iter()
        .filter(|f| f.arc_place && f.name != holon_api::schema::block::AFTER_BLOCK_ID)
        .map(|f| f.name)
        .collect();
    let junctions: Vec<(String, String)> = BlockSchemaModule
        .edge_fields()
        .into_iter()
        .map(|e| (e.join_table, e.field))
        .collect();

    let mut places = BTreeSet::new();
    for sql in statements {
        if !sql.trim_start().to_ascii_uppercase().starts_with("SELECT") {
            continue;
        }
        for (table, field) in &junctions {
            if names_column(sql, table) {
                places.insert(format!("block.{field}"));
            }
        }
        if !names_column(sql, BLOCK_WRITE_TABLE) {
            continue;
        }
        if projects_every_column(sql) {
            places.extend(stored.iter().map(|c| format!("block.{c}")));
            continue;
        }
        for column in &stored {
            if names_column(sql, column) {
                places.insert(format!("block.{column}"));
            }
        }
    }
    places
}

fn descriptor_of<'a>(catalog: &'a [OperationDescriptor], name: &str) -> &'a OperationDescriptor {
    catalog
        .iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("the block catalog advertises {name}"))
}

fn declared_emits(descriptor: &OperationDescriptor) -> BTreeSet<String> {
    descriptor
        .arcs
        .written_places()
        .iter()
        .map(|p| p.to_string())
        .collect()
}

fn declared_reads(descriptor: &OperationDescriptor) -> BTreeSet<String> {
    match &descriptor.arcs {
        TransitionArcs::Undeclared => BTreeSet::new(),
        TransitionArcs::Declared { reads, .. } => reads.iter().map(|p| p.to_string()).collect(),
    }
}

fn seed(id: &str, parent: Option<&str>, sort_key: &str, content: &str) -> String {
    let parent_sql = match parent {
        Some(p) => format!("'{p}'"),
        None => "'sentinel:no_parent'".to_string(),
    };
    format!(
        "INSERT INTO {BLOCK_WRITE_TABLE} (id, parent_id, sort_key, content, content_type, \
         created_at, updated_at) VALUES ('{id}', {parent_sql}, '{sort_key}', '{content}', \
         'text', 1, 1)"
    )
}

/// P, with children A and B. Enough shape for a move, a split, a join and a
/// delete to have somewhere to go.
async fn seeded(handle: &DbHandle) {
    handle
        .execute_ddl(
            "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                      created_at, updated_at) VALUES ('sentinel:no_parent', NULL, 'a0', '', \
                      'text', 1, 1)",
        )
        .await
        .ok();
    for statement in [
        seed("block:P", None, "a1", "parent"),
        seed("block:A", Some("block:P"), "a1", "alpha text"),
        seed("block:B", Some("block:P"), "a2", "beta text"),
    ] {
        handle
            .execute_ddl(&statement)
            .await
            .expect("seed a block row");
    }
}

struct Observation {
    op: String,
    written: BTreeSet<String>,
    read_evidence: BTreeSet<String>,
}

/// Run one op through the production provider and report what the store did.
async fn observe(
    harness: &Harness,
    provider: &dyn OperationProvider,
    op: &str,
    params: StorageEntity,
) -> Observation {
    let before = snapshot(&harness.handle).await;
    let entity = EntityName::new(BLOCK);
    SpanCollector::global().reset();
    let result = provider
        .execute_operation(&entity, op, params)
        .await
        .unwrap_or_else(|e| panic!("{op} must succeed against the seeded store: {e}"));
    let statements = sql_statements();
    // The statement capture depends on this test's runtime scope reaching the
    // Turso actor thread. When that wiring breaks the collector returns
    // nothing, every read-evidence set collapses to what the inverse alone
    // carries, and the read half goes quietly green — the exact shape of a
    // green gate that checked nothing. Refuse to report on no evidence.
    assert!(
        !statements.is_empty(),
        "0 statements captured while running {op}: the span collector saw no SQL, so \
         the read half of this oracle would pass on no evidence. Check that the test \
         drives a runtime built with `attach_scope_to_runtime`."
    );
    let after = snapshot(&harness.handle).await;
    Observation {
        op: op.to_string(),
        written: written_places(&before, &after),
        read_evidence: places_restored_by_the_inverse(&result.undo, &before)
            .into_iter()
            .chain(places_read_by_sql(&statements))
            .collect(),
    }
}

fn param(entries: &[(&str, &str)]) -> StorageEntity {
    entries
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).to_string())))
        .collect()
}

/// Every observation this oracle can make, over one seeded store per op.
async fn observations() -> Vec<Observation> {
    let mut out = Vec::new();

    // create, seeded with every edge field an ingest may carry — the open
    // `fields` bag is exactly why `emits` must name them all.
    let h = harness().await;
    seeded(&h.handle).await;
    out.push(
        observe(&h, h.crud.as_ref(), "create", {
            let mut fields = param(&[
                ("id", "block:N"),
                ("parent_id", "block:P"),
                ("sort_key", "a3"),
                ("content", "new"),
            ]);
            for (field, target) in [
                ("tags", "Page"),
                ("requires", "block:A"),
                ("advice_suppressed", "block:B"),
                ("contributes_to", "block:B"),
            ] {
                fields.insert(
                    field.into(),
                    Value::Array(vec![Value::String(target.to_string())]),
                );
            }
            fields
        })
        .await,
    );

    let h = harness().await;
    seeded(&h.handle).await;
    out.push(observe(&h, h.crud.as_ref(), "delete", param(&[("id", "block:A")])).await);

    // `set_field` is the sixth analyzable op and the one whose EXCLUDED
    // `block.sort_key` the delta half also speaks about — the reparenting
    // case is what decides whether the exclusion is true of the op.
    let h = harness().await;
    seeded(&h.handle).await;
    out.push(
        observe(
            &h,
            h.crud.as_ref(),
            "set_field",
            param(&[("id", "block:A"), ("field", "content"), ("value", "edited")]),
        )
        .await,
    );

    let h = harness().await;
    seeded(&h.handle).await;
    out.push(
        observe(
            &h,
            h.crud.as_ref(),
            "set_field",
            param(&[
                ("id", "block:B"),
                ("field", "parent_id"),
                ("value", "block:A"),
            ]),
        )
        .await,
    );

    let h = harness().await;
    seeded(&h.handle).await;
    out.push(
        observe(
            &h,
            h.blocks.as_ref(),
            "move_block",
            param(&[("id", "block:B"), ("parent_id", "block:A")]),
        )
        .await,
    );

    let h = harness().await;
    seeded(&h.handle).await;
    out.push(
        observe(
            &h,
            h.blocks.as_ref(),
            "split_block",
            [
                ("id".into(), Value::String("block:A".to_string())),
                ("position".into(), Value::Integer(5)),
            ]
            .into_iter()
            .collect(),
        )
        .await,
    );

    let h = harness().await;
    seeded(&h.handle).await;
    out.push(
        observe(
            &h,
            h.blocks.as_ref(),
            "join_block",
            [
                ("id".into(), Value::String("block:B".to_string())),
                ("position".into(), Value::Integer(0)),
            ]
            .into_iter()
            .collect(),
        )
        .await,
    );

    out
}

fn catalog(harness: &Harness) -> Vec<OperationDescriptor> {
    let mut ops = harness.crud.operations();
    ops.extend(harness.blocks.operations());
    ops
}

/// A runtime whose worker threads belong to this test's observability scope,
/// so SQL issued on the Turso actor thread reaches the collector. A plain
/// `#[tokio::test]` leaves those threads unscoped and the statement evidence
/// silently empty.
fn scoped_runtime() -> std::sync::Arc<tokio::runtime::Runtime> {
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    std::sync::Arc::new(builder.build().expect("tokio runtime"))
}

#[test]
fn declared_emits_contain_every_place_the_sql_authority_writes() {
    scoped_runtime().clone().block_on(async {
        let reference = harness().await;
        let catalog = catalog(&reference);
        let observations = observations().await;

        let mut checked = 0usize;
        let mut failures = Vec::new();
        for observation in &observations {
            let descriptor = descriptor_of(&catalog, &observation.op);
            let declared = declared_emits(descriptor);
            assert!(
                !declared.is_empty(),
                "{} declares no out-arc, so this containment check would be vacuous",
                observation.op
            );
            for place in &observation.written {
                checked += 1;
                if !declared.contains(place) {
                    failures.push(format!(
                        "{} WROTE {place} and does not declare it: an omitted written place is \
                     the ADR 0031 red, and `emits(excluded(place, reason))` is the only \
                     sanctioned silence. Declared: {declared:?}",
                        observation.op
                    ));
                }
            }
        }

        assert!(
            checked > 0,
            "no written place was observed at all — the harness ran no op that changed the \
         store, so this oracle proved nothing"
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
fn declared_reads_contain_every_place_the_inverse_captures() {
    scoped_runtime().clone().block_on(async {
        let reference = harness().await;
        let catalog = catalog(&reference);
        let observations = observations().await;

        let mut checked = 0usize;
        let mut failures = Vec::new();
        for observation in &observations {
            let descriptor = descriptor_of(&catalog, &observation.op);
            let declared = declared_reads(descriptor);
            for place in &observation.read_evidence {
                checked += 1;
                if !declared.contains(place) {
                    failures.push(format!(
                        "{} READ {place} — its inverse restores that place, or a statement it \
                     issued names it — and it is not in `reads`. Declared: {declared:?}",
                        observation.op
                    ));
                }
            }
        }

        assert!(
            checked > 0,
            "no read evidence was gathered at all — no inverse carried a held place and no \
         statement was captured, so this oracle proved nothing"
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}
