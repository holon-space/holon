//! Contract: a column declared with a format-naming type (`TEXT_FMT_<name>`)
//! keeps TEXT affinity, so the engine stores and returns the authored text
//! byte for byte.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const AUTHORED: [&str; 6] = ["007", "1.50", "10", "1e3", " 5", "0,5Kg + 2 Zehen"];

async fn handle() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
}

/// `typeof` and `|| ''` are expressions, so the read carries no declared type
/// and shows exactly what the engine stored.
async fn stored(handle: &DbHandle, relation: &str, column: &str) -> Vec<(String, String)> {
    handle
        .query(
            &format!(
                "SELECT typeof({column}) AS t, {column} || '' AS raw FROM {relation} ORDER BY id"
            ),
            HashMap::new(),
        )
        .await
        .expect("read stored text")
        .into_iter()
        .map(|row| match (row.get("t"), row.get("raw")) {
            (Some(Value::String(t)), Some(Value::String(raw))) => (t.clone(), raw.clone()),
            other => panic!("{relation}.{column}: unexpected row shape {other:?}"),
        })
        .collect()
}

async fn seed(handle: &DbHandle, decl: &str) {
    handle
        .execute_ddl(&format!(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, c {decl})"
        ))
        .await
        .expect("create table");
    for (i, text) in AUTHORED.iter().enumerate() {
        handle
            .execute(
                "INSERT INTO t (id, c) VALUES (?, ?)",
                vec![
                    turso::Value::Integer(i as i64),
                    turso::Value::Text((*text).into()),
                ],
            )
            .await
            .expect("insert");
    }
}

fn expected_text() -> Vec<(String, String)> {
    AUTHORED
        .iter()
        .map(|text| ("text".to_string(), text.to_string()))
        .collect()
}

#[tokio::test]
async fn a_format_declared_column_keeps_text_affinity() {
    let handle = handle().await;
    seed(&handle, "TEXT_FMT_tsl_count").await;
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW mv_t AS SELECT id, c AS amount FROM t")
        .await
        .expect("create matview");

    assert_eq!(stored(&handle, "t", "c").await, expected_text());
    assert_eq!(stored(&handle, "mv_t", "amount").await, expected_text());
}

#[tokio::test]
async fn a_declared_type_containing_int_takes_integer_affinity() {
    let handle = handle().await;
    seed(&handle, "TEXT_FMT_point").await;

    assert_ne!(stored(&handle, "t", "c").await, expected_text());
}

const AMOUNT: &str = "0,5Kg + 2 Zehen";

/// `t.c` is formatted; `mv_t` reads it through a matview under an alias.
async fn formatted_table_and_view(handle: &DbHandle) {
    handle
        .execute_ddl("CREATE TABLE t (id TEXT PRIMARY KEY, c TEXT_FMT_tsl_count)")
        .await
        .expect("create table");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW mv_t AS SELECT id, c AS amount FROM t")
        .await
        .expect("create matview");
}

async fn insert_amount(handle: &DbHandle, id: &str, amount: Option<&str>) {
    handle
        .execute(
            "INSERT INTO t (id, c) VALUES (?, ?)",
            vec![
                turso::Value::Text(id.into()),
                amount.map_or(turso::Value::Null, |a| turso::Value::Text(a.into())),
            ],
        )
        .await
        .expect("insert");
}

fn assert_quantity(value: Option<&Value>, original: &str, context: &str) {
    match value {
        Some(Value::Quantity(cell)) => {
            assert_eq!(cell.original(), original, "{context}");
            assert_eq!(cell.format().format().name(), "tsl_count", "{context}");
        }
        other => panic!("{context}: expected a tsl_count Value::Quantity, got {other:?}"),
    }
}

#[tokio::test]
async fn a_query_row_of_a_formatted_column_is_a_quantity() {
    let handle = handle().await;
    formatted_table_and_view(&handle).await;
    insert_amount(&handle, "a", Some(AMOUNT)).await;
    insert_amount(&handle, "b", None).await;

    for (sql, column) in [
        ("SELECT c FROM t WHERE id = 'a'", "c"),
        ("SELECT c AS amount FROM t WHERE id = 'a'", "amount"),
        ("SELECT x.c FROM t AS x WHERE x.id = 'a'", "c"),
        ("SELECT amount FROM mv_t WHERE id = 'a'", "amount"),
        ("SELECT amount AS q FROM mv_t WHERE id = 'a'", "q"),
    ] {
        let rows = handle.query(sql, HashMap::new()).await.expect(sql);
        assert_quantity(rows[0].get(column), AMOUNT, sql);
        let rows = handle.query_positional(sql, vec![]).await.expect(sql);
        assert_quantity(rows[0].get(column), AMOUNT, sql);
    }

    let rows = handle
        .query("SELECT c FROM t WHERE id = 'b'", HashMap::new())
        .await
        .expect("null cell");
    assert_eq!(rows[0].get("c"), Some(&Value::Null));
    let rows = handle
        .query(
            "SELECT c || '' AS raw FROM t WHERE id = 'a'",
            HashMap::new(),
        )
        .await
        .expect("expression");
    assert_eq!(rows[0].get("raw"), Some(&Value::String(AMOUNT.into())));
}

#[tokio::test]
async fn a_change_feed_row_of_a_formatted_column_is_a_quantity() {
    use holon_turso::turso::ChangeData;

    let handle = handle().await;
    formatted_table_and_view(&handle).await;
    let mut rx = handle.subscribe_row_changes();
    insert_amount(&handle, "a", Some(AMOUNT)).await;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let data = loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("a change on mv_t within 5 s")
            .expect("feed open");
        assert!(
            batch.metadata.degraded.is_none(),
            "{:?}",
            batch.metadata.degraded
        );
        if batch.metadata.relation_name != "mv_t" {
            continue;
        }
        match &batch.inner.items[0].change {
            ChangeData::Created { data, .. } | ChangeData::Updated { data, .. } => {
                break data.clone();
            }
            other => panic!("expected an upsert on mv_t, got {other:?}"),
        }
    };
    assert_quantity(data.get("amount"), AMOUNT, "mv_t change feed");
}

#[tokio::test]
async fn a_commit_listener_row_of_a_formatted_column_is_a_quantity() {
    use holon_turso::commit_event::CommitChange;

    let handle = handle().await;
    formatted_table_and_view(&handle).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    handle
        .listen_from_snapshot(
            "SELECT 1 AS one",
            move |event| {
                if event.relation() == "mv_t" {
                    tx.send(event.changes()).expect("test alive");
                }
            },
            |_| {},
        )
        .await
        .expect("listen");
    insert_amount(&handle, "a", Some(AMOUNT)).await;

    let changes = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("a commit on mv_t within 5 s")
        .expect("listener alive")
        .expect("the changes decode");
    match &changes[0] {
        CommitChange::Upsert(row) => assert_quantity(row.get("amount"), AMOUNT, "commit listener"),
        other => panic!("expected an upsert, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_format_in_a_declared_type_fails_the_read() {
    let handle = handle().await;
    handle
        .execute_ddl("CREATE TABLE u (id TEXT PRIMARY KEY, c TEXT_FMT_nope)")
        .await
        .expect("create table");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW mv_u AS SELECT id, c FROM u")
        .await
        .expect("create matview");
    let mut rx = handle.subscribe_row_changes();
    handle
        .execute("INSERT INTO u (id, c) VALUES ('a', '1')", vec![])
        .await
        .expect("insert");

    let err = handle
        .query("SELECT c FROM u", HashMap::new())
        .await
        .expect_err("an unknown format is not read as text")
        .to_string();
    assert!(err.contains("unknown format `nope`"), "{err}");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let degraded = loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("a batch on mv_u within 5 s")
            .expect("feed open");
        if batch.metadata.relation_name == "mv_u" {
            break batch.metadata.degraded;
        }
    };
    let degraded = degraded.expect("the change feed discloses the row it could not type");
    assert!(degraded.contains("unknown format `nope`"), "{degraded}");
}

/// Reads `sql` over a formatted `t.c` (plus `v`, a plain view, and `mv_cte`, a
/// matview over a CTE) and returns the cell under `column`.
async fn read_shape(sql: &str, column: &str) -> Value {
    let handle = handle().await;
    formatted_table_and_view(&handle).await;
    handle
        .execute_ddl("CREATE VIEW v AS SELECT id, c FROM t")
        .await
        .expect("create view");
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW mv_cte AS \
             WITH q AS (SELECT id, c FROM t) SELECT id, c AS amount FROM q",
        )
        .await
        .expect("create matview over a cte");
    insert_amount(&handle, "a", Some(AMOUNT)).await;
    let rows = handle.query(sql, HashMap::new()).await.expect(sql);
    rows[0]
        .get(column)
        .unwrap_or_else(|| panic!("{sql}: no column {column}"))
        .clone()
}

async fn assert_shape_typed(sql: &str, column: &str) {
    let value = read_shape(sql, column).await;
    assert_quantity(Some(&value), AMOUNT, sql);
}

async fn assert_shape_untyped(sql: &str, column: &str) {
    let value = read_shape(sql, column).await;
    assert_eq!(value, Value::String(AMOUNT.into()), "{sql}");
}

#[tokio::test]
async fn shape_select_star_keeps_the_type() {
    assert_shape_typed("SELECT * FROM t", "c").await;
}

#[tokio::test]
async fn shape_union_keeps_the_type() {
    assert_shape_typed("SELECT c FROM t UNION SELECT c FROM t", "c").await;
}

#[tokio::test]
async fn shape_union_all_keeps_the_type() {
    assert_shape_typed("SELECT c FROM t UNION ALL SELECT c FROM t", "c").await;
}

#[tokio::test]
async fn shape_join_keeps_the_type() {
    assert_shape_typed("SELECT a.c FROM t a JOIN t b ON a.id = b.id", "c").await;
}

#[tokio::test]
async fn shape_matview_over_a_cte_keeps_the_type() {
    assert_shape_typed("SELECT amount FROM mv_cte", "amount").await;
}

#[tokio::test]
async fn shape_plain_view_loses_the_type() {
    assert_shape_untyped("SELECT c FROM v", "c").await;
}

#[tokio::test]
async fn shape_cte_in_the_query_loses_the_type() {
    assert_shape_untyped("WITH q AS (SELECT c FROM t) SELECT c FROM q", "c").await;
}

#[tokio::test]
async fn shape_derived_subquery_loses_the_type() {
    assert_shape_untyped("SELECT c FROM (SELECT c FROM t) AS s", "c").await;
}

#[tokio::test]
async fn shape_json_object_flattening_loses_the_type() {
    assert_shape_untyped("SELECT json_object('c', c) AS data FROM t", "c").await;
}

/// A matview created on a raw connection never passes through the DDL funnel,
/// so the schema catalog does not know it.
async fn raw_matview_over_formatted_table() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    handle
        .execute_ddl("CREATE TABLE t (id TEXT PRIMARY KEY, c TEXT_FMT_tsl_count)")
        .await
        .expect("create table");
    backend
        .get_connection()
        .expect("raw connection")
        .execute("CREATE MATERIALIZED VIEW mv_raw AS SELECT id, c FROM t", ())
        .await
        .expect("create matview on the raw connection");
    std::mem::forget(backend);
    handle
}

#[tokio::test]
async fn a_change_on_a_relation_unknown_to_the_catalog_degrades_the_batch() {
    let handle = raw_matview_over_formatted_table().await;
    let mut rx = handle.subscribe_row_changes();
    insert_amount(&handle, "a", Some(AMOUNT)).await;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let degraded = loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("a batch on mv_raw within 5 s")
            .expect("feed open");
        if batch.metadata.relation_name == "mv_raw" {
            break batch.metadata.degraded;
        }
    };
    let degraded = degraded.expect("the change feed discloses a relation it cannot type");
    assert!(degraded.contains("mv_raw"), "{degraded}");
}

#[tokio::test]
async fn a_commit_on_a_relation_unknown_to_the_catalog_fails_to_decode() {
    let handle = raw_matview_over_formatted_table().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    handle
        .listen_from_snapshot(
            "SELECT 1 AS one",
            move |event| {
                if event.relation() == "mv_raw" {
                    tx.send(event.changes()).expect("test alive");
                }
            },
            |_| {},
        )
        .await
        .expect("listen");
    insert_amount(&handle, "a", Some(AMOUNT)).await;

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("a commit on mv_raw within 5 s")
        .expect("listener alive");
    let err = result.expect_err("an unknown relation is not decoded as plain text");
    assert!(err.contains("mv_raw"), "{err}");
}

/// Turso hands every commit's changes to every backend on the database, so a
/// backend must also type a relation another backend created.
#[tokio::test]
async fn a_change_on_a_relation_another_backend_created_is_typed() {
    let db = TursoBackend::open_database(":memory:").expect("in-memory db");
    let (creator, creator_handle) =
        TursoBackend::new(db.clone(), tokio::sync::broadcast::channel(1024).0).expect("creator");
    let (observer, observer_handle) =
        TursoBackend::new(db, tokio::sync::broadcast::channel(1024).0).expect("observer");
    observer_handle
        .query("SELECT 1 AS one", HashMap::new())
        .await
        .expect("observer ready before the DDL");
    let mut rx = observer_handle.subscribe_row_changes();
    formatted_table_and_view(&creator_handle).await;
    insert_amount(&creator_handle, "a", Some(AMOUNT)).await;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let batch = loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("a change on mv_t within 5 s")
            .expect("feed open");
        if batch.metadata.relation_name == "mv_t" {
            break batch;
        }
    };
    assert!(
        batch.metadata.degraded.is_none(),
        "{:?}",
        batch.metadata.degraded
    );
    std::mem::forget((creator, observer));
}
