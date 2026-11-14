//! The SQL provider's multi-statement writes are all-or-nothing, and its diff
//! guard drops an edit only when the stored value is the same.
//!
//! Oracle: the rows read back from an in-memory Turso built from the
//! production schema modules. A failed write must leave every row it would
//! have touched as it was; a successful one must leave exactly the new value.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use holon::core::SqlOperationProvider;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EdgeField;
use holon_api::EntityName;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::LinkSchemaModule;
use proptest::prelude::*;

const ROOT: &str = "sentinel:no_parent";
const BLOCK: &str = "block:subject";

struct Store {
    _backend: TursoBackend,
    db: DbHandle,
    provider: SqlOperationProvider,
}

async fn store() -> Store {
    let (backend, db) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    db.execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FKs");
    CoreSchemaModule.ensure_schema(&db).await.expect("core");
    BlockSchemaModule.ensure_schema(&db).await.expect("block");
    LinkSchemaModule.ensure_schema(&db).await.expect("links");
    let provider = SqlOperationProvider::with_edge_fields(
        db.clone(),
        "block_raw".to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
        std::sync::Arc::new(holon_api::SystemClock),
    );
    Store {
        _backend: backend,
        db,
        provider,
    }
}

fn entity() -> EntityName {
    EntityName::new("block")
}

fn params<const N: usize>(pairs: [(&str, Value); N]) -> StorageEntity {
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

fn cases(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

async fn create_block(st: &Store, id: &str) {
    st.provider
        .execute_operation(
            &entity(),
            "create",
            params([("id", s(id)), ("parent_id", s(ROOT)), ("content", s("c"))]),
        )
        .await
        .expect("seed create");
}

async fn query_strings(db: &DbHandle, sql: &str, column: &str) -> Vec<String> {
    db.query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .into_iter()
        .map(|row| match row.get(column) {
            Some(Value::String(v)) => v.clone(),
            other => panic!("{sql}: column {column} is not a string: {other:?}"),
        })
        .collect()
}

fn junction(field: EdgeField) -> (&'static str, &'static str, &'static str) {
    match field {
        EdgeField::Tags => ("block_tags", "block_id", "tag"),
        EdgeField::Requires => ("block_requires", "block_id", "required_id"),
        EdgeField::AdviceSuppressed => ("advice_suppressed", "anchor_id", "lesson_id"),
        EdgeField::ContributesTo => ("block_contributes_to", "block_id", "target_id"),
    }
}

async fn stored_targets(db: &DbHandle, field: EdgeField, id: &str) -> BTreeSet<String> {
    let (table, source, target) = junction(field);
    query_strings(
        db,
        &format!("SELECT {target} AS t FROM {table} WHERE {source} = '{id}'"),
        "t",
    )
    .await
    .into_iter()
    .collect()
}

/// A tag is any string; the reference fields hold block ids.
fn target_id(field: EdgeField, name: &str) -> String {
    match field {
        EdgeField::Tags => name.to_string(),
        _ => format!("block:{name}"),
    }
}

async fn set_edges(
    st: &Store,
    field: EdgeField,
    targets: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    st.provider
        .execute_operation(
            &entity(),
            "set_field",
            params([
                ("id", s(BLOCK)),
                ("field", s(field.column())),
                (
                    "value",
                    Value::Array(targets.iter().map(|t| s(t)).collect()),
                ),
            ]),
        )
        .await
        .map(drop)
}

#[derive(Clone, Debug)]
struct EdgeSetCase {
    field: EdgeField,
    old: BTreeSet<String>,
    new: Vec<String>,
}

impl EdgeSetCase {
    fn has_duplicate(&self) -> bool {
        self.new.iter().collect::<BTreeSet<_>>().len() != self.new.len()
    }
}

/// A replace either stores the new set or fails and keeps the old one: the
/// junction primary key refuses a repeated target, so a repeat is the failing
/// write.
fn edge_set_is_all_or_nothing(case: &EdgeSetCase) -> Result<(), TestCaseError> {
    runtime().block_on(async {
        let st = store().await;
        create_block(&st, BLOCK).await;
        let old: Vec<String> = case.old.iter().cloned().collect();
        set_edges(&st, case.field, &old)
            .await
            .expect("seeding a distinct target set");
        prop_assert_eq!(&stored_targets(&st.db, case.field, BLOCK).await, &case.old);

        let outcome = set_edges(&st, case.field, &case.new).await;
        let stored = stored_targets(&st.db, case.field, BLOCK).await;
        if case.has_duplicate() {
            prop_assert!(outcome.is_err(), "a repeated target must fail the write");
            prop_assert_eq!(&stored, &case.old, "a failed replace must keep the old set");
        } else {
            prop_assert!(
                outcome.is_ok(),
                "a distinct target list must store: {:?}",
                outcome
            );
            prop_assert_eq!(stored, case.new.iter().cloned().collect::<BTreeSet<_>>());
        }
        Ok(())
    })
}

fn edge_field() -> impl Strategy<Value = EdgeField> {
    proptest::sample::select(EdgeField::ALL.to_vec())
}

fn edge_set_case() -> impl Strategy<Value = EdgeSetCase> {
    let name = "[a-z0-9-]{1,6}";
    (
        edge_field(),
        proptest::collection::btree_set(name, 0..4),
        proptest::collection::btree_set(name, 0..4),
        any::<bool>(),
        any::<prop::sample::Index>(),
    )
        .prop_map(|(field, old, new, duplicate, at)| {
            let mut new: Vec<String> = new.into_iter().collect();
            if duplicate {
                let repeated = if new.is_empty() {
                    "dup".to_string()
                } else {
                    new[at.index(new.len())].clone()
                };
                new.push(repeated.clone());
                if new.len() == 1 {
                    new.push(repeated);
                }
            }
            EdgeSetCase {
                field,
                old: old.iter().map(|n| target_id(field, n)).collect(),
                new: new.iter().map(|n| target_id(field, n)).collect(),
            }
        })
}

#[test]
fn edge_set_is_all_or_nothing_over_every_edge_field() {
    let examples = [EdgeSetCase {
        field: EdgeField::Tags,
        old: ["a", "b"].map(String::from).into(),
        new: ["x", "x"].map(String::from).into(),
    }];
    for case in &examples {
        edge_set_is_all_or_nothing(case).unwrap_or_else(|e| panic!("{case:?}: {e}"));
    }

    let drawn = AtomicUsize::new(0);
    let duplicates = AtomicUsize::new(0);
    proptest!(cases(48), |(case in edge_set_case())| {
        drawn.fetch_add(1, Ordering::Relaxed);
        if case.has_duplicate() {
            duplicates.fetch_add(1, Ordering::Relaxed);
        }
        edge_set_is_all_or_nothing(&case)?;
    });
    let (drawn, duplicates) = (drawn.into_inner(), duplicates.into_inner());
    assert!(
        duplicates * 4 >= drawn && duplicates * 4 <= drawn * 3,
        "the failing arm must be drawn in about half the cases: {duplicates} of {drawn}"
    );
}

/// Every row a purge of `BLOCK` reads or removes: the row, its junction rows
/// (cascaded) and its outbound `block_links` rows (deleted explicitly).
async fn purge_footprint(db: &DbHandle) -> (Vec<String>, BTreeSet<String>, Vec<String>) {
    let rows = query_strings(
        db,
        &format!("SELECT id FROM block_raw WHERE id = '{BLOCK}'"),
        "id",
    )
    .await;
    let tags = stored_targets(db, EdgeField::Tags, BLOCK).await;
    let links = query_strings(
        db,
        &format!("SELECT target FROM block_links WHERE source_block_id = '{BLOCK}'"),
        "target",
    )
    .await;
    (rows, tags, links)
}

/// A purge removes the row with its edges, or — when one of its statements
/// fails — leaves all of them. The failure is a test-only trigger that raises
/// on the `block_links` DELETE, the last statement a purge runs.
fn purge_is_all_or_nothing(tags: &BTreeSet<String>, fail: bool) -> Result<(), TestCaseError> {
    runtime().block_on(async {
        let st = store().await;
        create_block(&st, BLOCK).await;
        set_edges(
            &st,
            EdgeField::Tags,
            &tags.iter().cloned().collect::<Vec<_>>(),
        )
        .await
        .expect("seed tags");
        st.db
            .execute(
                &format!(
                    "INSERT INTO block_links (source_block_id, target, kind) \
                     VALUES ('{BLOCK}', 'Elsewhere', 'wiki')"
                ),
                vec![],
            )
            .await
            .expect("seed link");
        if fail {
            st.db
                .execute_ddl(
                    "CREATE TRIGGER refuse_link_delete BEFORE DELETE ON block_links \
                     BEGIN SELECT RAISE(ABORT, 'link delete refused'); END",
                )
                .await
                .expect("the Turso fork must accept a RAISE trigger");
        }
        let before = purge_footprint(&st.db).await;
        prop_assert_eq!(&before.1, tags);

        let outcome = st
            .provider
            .execute_operation(&entity(), "delete", params([("id", s(BLOCK))]))
            .await;
        let after = purge_footprint(&st.db).await;
        if fail {
            prop_assert!(outcome.is_err(), "the trigger must fail the purge");
            prop_assert_eq!(
                after,
                before,
                "a failed purge must leave the row and its edges"
            );
        } else {
            prop_assert!(outcome.is_ok(), "purge: {:?}", outcome.err());
            prop_assert_eq!(after, (Vec::new(), BTreeSet::new(), Vec::new()));
        }
        Ok(())
    })
}

#[test]
fn purge_is_all_or_nothing_with_and_without_a_failing_statement() {
    let examples = [(["a", "b"].map(String::from).into(), false)];
    for (tags, fail) in &examples {
        purge_is_all_or_nothing(tags, *fail).unwrap_or_else(|e| panic!("{tags:?} {fail}: {e}"));
    }
    proptest!(cases(16), |(
        tags in proptest::collection::btree_set("[a-z0-9]{1,6}", 0..4),
        fail in any::<bool>(),
    )| {
        purge_is_all_or_nothing(&tags, fail)?;
    });
}

/// The plain columns the update diff guard compares. `content` is stored in
/// its canonical form (trailing whitespace trimmed); `source_name` verbatim.
#[derive(Clone, Copy, Debug)]
enum TextColumn {
    Content,
    SourceName,
}

impl TextColumn {
    fn name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::SourceName => "source_name",
        }
    }

    fn stored_form(self, written: &str) -> String {
        match self {
            Self::Content => {
                holon_api::content_canonical::canonicalize_stored_content(written, false)
            }
            Self::SourceName => written.to_string(),
        }
    }
}

async fn read_column(db: &DbHandle, column: &str) -> Value {
    db.query(
        &format!("SELECT {column} AS v FROM block_raw WHERE id = '{BLOCK}'"),
        HashMap::new(),
    )
    .await
    .expect("read column")
    .into_iter()
    .next()
    .expect("the block row")
    .remove("v")
    .expect("the selected column")
}

/// The stored row; `properties` as its TEXT, since the read path re-prints a
/// nested JSON property in hash order.
async fn read_row(db: &DbHandle) -> std::collections::BTreeMap<String, Value> {
    let mut row = db
        .query(
            &format!("SELECT *, properties AS properties_text FROM block_raw WHERE id = '{BLOCK}'"),
            HashMap::new(),
        )
        .await
        .expect("read row")
        .into_iter()
        .next()
        .expect("the block row");
    row.remove("properties").expect("the properties column");
    row.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

async fn update(st: &Store, fields: StorageEntity) {
    let mut p = fields;
    p.insert("id".into(), s(BLOCK));
    st.provider
        .execute_operation(&entity(), "update", p)
        .await
        .expect("update");
}

/// An edit reaches the store whenever the bytes it stores differ from the
/// stored ones, JSON-equal or not.
fn content_edit_by_bytes(column: TextColumn, old: &str, new: &str) -> Result<(), TestCaseError> {
    runtime().block_on(async {
        let st = store().await;
        create_block(&st, BLOCK).await;
        update(&st, params([(column.name(), s(old))])).await;
        update(&st, params([(column.name(), s(new))])).await;
        prop_assert_eq!(
            read_column(&st.db, column.name()).await,
            s(&column.stored_form(new)),
            "{} edit {:?} -> {:?}",
            column.name(),
            old,
            new
        );
        Ok(())
    })
}

fn old_text() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(r#"{"b":1,"a":2}"#.to_string()),
        Just(r#"{"a":[1,2],"c":"x"}"#.to_string()),
        Just("true".to_string()),
        Just("42".to_string()),
        Just(r#""quoted""#.to_string()),
        "[a-z]{1,8}( [a-z]{1,8})?",
    ]
}

/// A new value derived from the old one: a trailing space, reflowed
/// whitespace, reordered object keys, or the same bytes.
fn edit(old: &str, how: u8) -> String {
    match how {
        0 => format!("{old} "),
        1 => old.replace(',', ", ").replace(':', ": "),
        2 => match serde_json::from_str::<serde_json::Value>(old) {
            Ok(serde_json::Value::Object(map)) => {
                let mut pairs: Vec<_> = map.into_iter().collect();
                pairs.reverse();
                let body: Vec<String> = pairs
                    .into_iter()
                    .map(|(k, v)| format!("{}:{}", serde_json::to_string(&k).unwrap(), v))
                    .collect();
                format!("{{{}}}", body.join(","))
            }
            _ => old.to_string(),
        },
        _ => old.to_string(),
    }
}

#[test]
fn content_edit_by_bytes_reaches_the_store() {
    let examples = [
        (TextColumn::SourceName, "true", "true "),
        (TextColumn::Content, r#"{"a":1}"#, r#"{ "a": 1 }"#),
        (TextColumn::SourceName, r#"{"a":1}"#, r#"{ "a": 1 }"#),
    ];
    for (column, old, new) in examples {
        content_edit_by_bytes(column, old, new)
            .unwrap_or_else(|e| panic!("{column:?} {old:?} -> {new:?}: {e}"));
    }
    proptest!(cases(32), |(
        column in prop_oneof![Just(TextColumn::Content), Just(TextColumn::SourceName)],
        old in old_text(),
        how in 0u8..4,
    )| {
        content_edit_by_bytes(column, &old, &edit(&old, how))?;
    });
}

/// `properties` is compared as JSON: a stored bag whose key order differs
/// from the canonical form, at any depth, is the same bag, so re-writing it is
/// a no-op.
#[test]
fn a_reordered_properties_bag_is_not_an_edit() {
    runtime().block_on(async {
        let st = store().await;
        create_block(&st, BLOCK).await;
        st.db
            .execute(
                &format!(
                    "UPDATE block_raw SET properties = '{{\"b\":1,\"a\":2,\"c\":{{\"y\":2,\"x\":1}}}}', \
                     property_kinds = '{{\"c\":\"json\"}}', updated_at = 1 WHERE id = '{BLOCK}'"
                ),
                vec![],
            )
            .await
            .expect("store a non-canonical bag");
        update(
            &st,
            params([
                ("a", Value::Integer(2)),
                ("b", Value::Integer(1)),
                ("c", Value::Json(r#"{"x":1,"y":2}"#.to_string())),
                ("updated_at", Value::Integer(2)),
            ]),
        )
        .await;
        assert_eq!(read_column(&st.db, "updated_at").await, Value::Integer(1));
        assert_eq!(
            read_column(&st.db, "properties").await,
            s(r#"{"b":1,"a":2,"c":{"y":2,"x":1}}"#),
            "the stored bag keeps its bytes"
        );
    });
}

/// A second update with the same values writes no column: `updated_at`, which
/// only a write that changed something carries along, keeps the first value.
#[test]
fn an_equal_second_update_writes_nothing() {
    runtime().block_on(async {
        let st = store().await;
        create_block(&st, BLOCK).await;
        let fields = |at: i64| {
            params([
                ("content", s(r#"{"a":1}"#)),
                ("source_name", s("true")),
                ("marks", s(&holon_api::marks_to_json(&[]))),
                ("plain", s("value")),
                ("doc", Value::Json(r#"{"y":2,"x":1}"#.to_string())),
                ("updated_at", Value::Integer(at)),
            ])
        };
        update(&st, fields(1)).await;
        let kinds = read_column(&st.db, "property_kinds").await;
        assert!(
            matches!(kinds, Value::String(_)),
            "the fixture must store a property_kinds map, else the check is vacuous: {kinds:?}"
        );
        let before = read_row(&st.db).await;
        update(&st, fields(2)).await;
        let after = read_row(&st.db).await;
        let changed: Vec<_> = after
            .iter()
            .filter(|(k, v)| before.get(*k) != Some(*v))
            .collect();
        assert!(
            changed.is_empty(),
            "re-written: {changed:?}\nbefore: {before:?}"
        );
    });
}
