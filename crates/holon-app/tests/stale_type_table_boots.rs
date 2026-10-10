//! A stored `<type>_raw` table whose shape differs from the type's declaration
//! does not stop the boot.
//!
//! A column the declaration added (nullable, or with a default) is added to the
//! stored table, the rows survive, and an info condition says so. Any other
//! difference refuses only that type: the rest of Holon works, a condition
//! names the type, the table, the difference and the row count, the table is
//! quarantined so no read path finds it, writes to the type fail naming that
//! condition, and no row is deleted until the user runs the condition's
//! remedy.
//!
//! @pbt kind harness
//! @pbt covers stale-type-table-boots — a stored type table that drifted from
//! its declaration is adapted (added column) or refused alone (anything else),
//! disclosed on the condition bus, with a drop-table remedy
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never changes a
//! type declaration between boots

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon::api::HolonService;
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::QueryEngine;
use holon_api::QueryLanguage;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const FIRST_DB: &str = "first.db";
const LATER_DB: &str = "later.db";

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    bus: Arc<ConditionBus>,
    _session: Arc<FrontendSession>,
}

async fn boot(dir: &Path, db_file: &str) -> Booted {
    let config = HolonConfig {
        db_path: Some(dir.join(db_file)),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let bus = Arc::new(ConditionBus::new());
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        bus.clone(),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring must boot a session");
    Booted {
        engine,
        injector,
        bus,
        _session: session,
    }
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), v.clone()))
        .collect()
}

async fn op(booted: &Booted, entity: &str, op: &str, p: StorageEntity) -> anyhow::Result<()> {
    booted
        .engine
        .execute_operation(&EntityName::new(entity), op, p, OpOrigin::User)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{entity}.{op}: {e}"))
}

async fn stock(booted: &Booted, id: &str, extra: &[(&str, Value)]) -> anyhow::Result<()> {
    let mut fields = vec![
        ("id", Value::String(id.into())),
        ("name", Value::String(format!("{id} name"))),
        ("quantity", Value::Float(1.0)),
    ];
    fields.extend(extra.iter().cloned());
    op(booted, "pantry_item", "create", params(&fields)).await
}

async fn sql(booted: &Booted, sql: &str) -> Vec<StorageEntity> {
    booted
        .engine
        .db_handle()
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

async fn engine_read(booted: &Booted, prql: &str) -> anyhow::Result<Vec<StorageEntity>> {
    let compiled = booted
        .engine
        .compile_to_sql(prql, QueryLanguage::HolonPrql)?;
    booted
        .engine
        .execute_query(compiled, HashMap::new(), None)
        .await
}

fn conditions(bus: &ConditionBus) -> Vec<Condition> {
    bus.subscribe().current
}

fn described(conditions: &[Condition]) -> Vec<String> {
    conditions
        .iter()
        .map(|c| {
            let detail = c.reason.detail(&c.subject);
            format!(
                "[{}] {} {} | {}",
                c.condition_key().kind,
                c.subject,
                detail.headline,
                detail.body.join(" | ")
            )
        })
        .collect()
}

/// Boot a fresh session, stock two pantry items and one person, shut down,
/// and hand back a copy of its database that `reshape` changed the way an
/// older binary would have left it.
async fn seed_then_reshape(dir: &Path, reshape: impl FnOnce(&Arc<turso_core::Connection>)) {
    {
        let first = boot(dir, FIRST_DB).await;
        let schema_conditions: Vec<String> = described(&conditions(&first.bus))
            .into_iter()
            .filter(|c| {
                [
                    "[type-table-refused]",
                    "[table-columns-added]",
                    "[table-rebuilt]",
                    "[schema-module-failed]",
                ]
                .iter()
                .any(|kind| c.starts_with(kind))
            })
            .collect();
        assert!(
            schema_conditions.is_empty(),
            "a fresh database matches every declaration: {schema_conditions:#?}"
        );
        stock(&first, "pantry-item:flour", &[])
            .await
            .expect("stock flour");
        stock(&first, "pantry-item:salt", &[])
            .await
            .expect("stock salt");
        first
            .engine
            .db_handle()
            .execute(
                "INSERT INTO person_raw (id, email, role) VALUES ('person:ada', 'ada@x.y', NULL)",
                vec![],
            )
            .await
            .expect("a person row");
        holon_app::shutdown_session(&first.injector)
            .await
            .expect("shut the first session down");
    }
    for suffix in ["", "-wal"] {
        let from = dir.join(format!("{FIRST_DB}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, dir.join(format!("{LATER_DB}{suffix}")))
                .expect("copy the first session's database");
        }
    }
    let db = holon::storage::turso::TursoBackend::open_database(dir.join(LATER_DB))
        .expect("open between the sessions");
    let conn = db.connect().expect("connect");
    reshape(&conn);
}

fn exec(conn: &Arc<turso_core::Connection>, sql: &str) {
    conn.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// Every view whose definition mentions `pantry_item`, with its SQL, in
/// creation order.
fn pantry_views(conn: &Arc<turso_core::Connection>) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'view' AND sql LIKE \
             '%pantry_item%' ORDER BY rowid",
        )
        .expect("prepare");
    stmt.run_collect_rows()
        .expect("list views")
        .into_iter()
        .map(|row| match row.as_slice() {
            [turso_core::Value::Text(name), turso_core::Value::Text(sql)] => {
                (name.as_str().to_string(), sql.as_str().to_string())
            }
            other => panic!("view row {other:?}"),
        })
        .collect()
}

/// Replace `pantry_item_raw` by a table of `columns` holding the same rows,
/// with the views over it dropped first.
fn rebuild_pantry_raw(conn: &Arc<turso_core::Connection>, columns: &str, copied: &str) {
    for (name, _) in pantry_views(conn).into_iter().rev() {
        exec(conn, &format!("DROP VIEW {name}"));
    }
    exec(conn, &format!("CREATE TABLE pantry_old ({columns})"));
    exec(
        conn,
        &format!("INSERT INTO pantry_old ({copied}) SELECT {copied} FROM pantry_item_raw"),
    );
    exec(conn, "DROP TABLE pantry_item_raw");
    exec(conn, &format!("CREATE TABLE pantry_item_raw ({columns})"));
    exec(
        conn,
        &format!("INSERT INTO pantry_item_raw ({copied}) SELECT {copied} FROM pantry_old"),
    );
    exec(conn, "DROP TABLE pantry_old");
}

const QUARANTINED: &str = "pantry_item_raw__quarantined";

/// Every stored row of `table`, each rendered with `quote()` and the storage
/// class of `quantity`, so equal output means byte-equal rows.
fn snapshot_sql(table: &str) -> String {
    format!(
        "SELECT quote(id) || ',' || quote(name) || ',' || quote(quantity) || ',' || \
         typeof(quantity) || ',' || quote(unit) || ',' || quote(opened_at) || ',' || \
         quote(best_before) || ',' || quote(properties) || ',' || quote(property_kinds) || \
         ',' || quote(product_id) AS r FROM {table} ORDER BY id"
    )
}

fn snapshot_of(conn: &Arc<turso_core::Connection>, table: &str) -> Vec<String> {
    let mut stmt = conn.prepare(snapshot_sql(table)).expect("prepare snapshot");
    stmt.run_collect_rows()
        .expect("snapshot rows")
        .into_iter()
        .map(|row| match row.as_slice() {
            [turso_core::Value::Text(r)] => r.as_str().to_string(),
            other => panic!("snapshot row {other:?}"),
        })
        .collect()
}

async fn stored_snapshot(booted: &Booted, table: &str) -> Vec<String> {
    sql(booted, &snapshot_sql(table))
        .await
        .iter()
        .map(|row| match row.get("r") {
            Some(Value::String(r)) => r.clone(),
            other => panic!("snapshot row {other:?}"),
        })
        .collect()
}

async fn tables_named(booted: &Booted, name: &str) -> usize {
    sql(
        booted,
        &format!("SELECT name FROM sqlite_schema WHERE type = 'table' AND name = '{name}'"),
    )
    .await
    .len()
}

const KEPT: &str = "id, name, quantity, unit, opened_at, best_before, properties, property_kinds";

#[tokio::test(flavor = "multi_thread")]
async fn a_declared_nullable_column_is_added_and_the_rows_survive() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_then_reshape(dir.path(), |conn| {
        rebuild_pantry_raw(
            conn,
            "\"id\" TEXT PRIMARY KEY NOT NULL, \"name\" TEXT NOT NULL, \"quantity\" REAL NOT \
             NULL, \"unit\" TEXT, \"opened_at\" TEXT, \"best_before\" TEXT, \"properties\" \
             TEXT, \"property_kinds\" TEXT",
            KEPT,
        );
        exec(
            conn,
            &format!("CREATE MATERIALIZED VIEW pantry_item AS SELECT {KEPT} FROM pantry_item_raw"),
        );
    })
    .await;

    let later = boot(dir.path(), LATER_DB).await;

    let rows = sql(&later, "SELECT id, product_id FROM pantry_item ORDER BY id").await;
    let read: Vec<(Option<&Value>, Option<&Value>)> = rows
        .iter()
        .map(|r| (r.get("id"), r.get("product_id")))
        .collect();
    assert_eq!(
        read,
        vec![
            (
                Some(&Value::String("pantry-item:flour".into())),
                Some(&Value::Null)
            ),
            (
                Some(&Value::String("pantry-item:salt".into())),
                Some(&Value::Null)
            ),
        ],
        "both stored rows must survive and read the added column as NULL"
    );

    stock(
        &later,
        "pantry-item:sugar",
        &[("product_id", Value::String("ean:123".into()))],
    )
    .await
    .expect("a write naming the added column must succeed");
    let sugar = sql(
        &later,
        "SELECT product_id FROM pantry_item WHERE id = 'pantry-item:sugar'",
    )
    .await;
    assert_eq!(
        sugar.first().and_then(|r| r.get("product_id")),
        Some(&Value::String("ean:123".into())),
        "the added column must round-trip through the matview"
    );

    let current = conditions(&later.bus);
    let shown = described(&current);
    let added = current
        .iter()
        .find(|c| c.condition_key().kind == "table-columns-added" && c.subject == "pantry_item")
        .unwrap_or_else(|| panic!("no info condition names the added column: {shown:#?}"));
    let text = described(std::slice::from_ref(added)).join("");
    assert!(
        text.contains("product_id") && text.contains("pantry_item_raw") && text.contains('2'),
        "the condition must name the column, the table and the row count: {text}"
    );
    assert!(
        !current
            .iter()
            .any(|c| c.condition_key().kind == "type-table-refused"),
        "an added nullable column refuses nothing: {shown:#?}"
    );
}

/// Seed a database whose `pantry_item_raw.quantity` is stored as TEXT while
/// the declaration says REAL, with the views over it restored, and return its
/// rows as [`snapshot_sql`] renders them.
async fn seed_refused_pantry(dir: &Path) -> Vec<String> {
    let all = format!("{KEPT}, product_id");
    let mut rows = Vec::new();
    seed_then_reshape(dir, |conn| {
        let views = pantry_views(conn);
        rebuild_pantry_raw(
            conn,
            "\"id\" TEXT PRIMARY KEY NOT NULL, \"name\" TEXT NOT NULL, \"quantity\" TEXT NOT \
             NULL, \"unit\" TEXT, \"opened_at\" TEXT, \"best_before\" TEXT, \"properties\" \
             TEXT, \"property_kinds\" TEXT, \"product_id\" TEXT",
            &all,
        );
        for (_, view_sql) in views {
            exec(conn, &view_sql);
        }
        rows = snapshot_of(conn, "pantry_item_raw");
    })
    .await;
    assert_eq!(rows.len(), 2, "the seed stores two pantry rows: {rows:#?}");
    rows
}

fn assert_refusal_disclosed(booted: &Booted) {
    let current = conditions(&booted.bus);
    let shown = described(&current);
    let refused = current
        .iter()
        .find(|c| c.condition_key().kind == "type-table-refused" && c.subject == "pantry_item")
        .unwrap_or_else(|| panic!("the drifted type is not disclosed: {shown:#?}"));
    let text = described(std::slice::from_ref(refused)).join("");
    for needle in ["pantry_item_raw", "quantity", "REAL", "TEXT", "2 rows"] {
        assert!(
            text.contains(needle),
            "the refusal must name the table, the difference and the row count (missing \
             {needle:?}): {text}"
        );
    }
}

/// No read path serves a row of the refused table, whatever SQL names it.
async fn assert_no_read_serves_the_refused_rows(booted: &Booted) {
    let mut served = Vec::new();

    let bypass = "SELECT CAST((SELECT quantity FROM pantry_item_raw WHERE id = \
                  'pantry-item:flour') AS TEXT) AS leaked";
    if let Ok(rows) = booted
        .engine
        .execute_query(bypass.to_string(), HashMap::new(), None)
        .await
    {
        served.push(format!("execute_query, CAST subquery: {rows:?}"));
    }

    if let Ok(result) = HolonService::new(booted.engine.clone())
        .execute_raw_sql("SELECT id, quantity FROM pantry_item_raw", HashMap::new())
        .await
    {
        served.push(format!("MCP execute_raw_sql: {:?}", result.rows));
    }

    if booted
        .engine
        .subscribe_sql("SELECT id, quantity FROM pantry_item_raw")
        .await
        .is_ok()
    {
        served.push("subscribe_sql opened a subscription".to_string());
    }

    let found = booted
        .engine
        .quick_open_search("flour")
        .await
        .expect("search keeps working while a type is refused");
    let hits = format!("{found:?}");
    if hits.contains("pantry-item:flour") {
        served.push(format!("quick-open search: {hits}"));
    }

    assert!(
        served.is_empty(),
        "read paths served the refused table: {served:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_column_type_refuses_only_that_type_until_the_user_drops_the_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stored = seed_refused_pantry(dir.path()).await;

    let later = boot(dir.path(), LATER_DB).await;
    assert_refusal_disclosed(&later);

    let person_sql = later
        .engine
        .compile_to_sql("from person", QueryLanguage::HolonPrql)
        .expect("compile from person");
    let people = later
        .engine
        .execute_query(person_sql, HashMap::new(), None)
        .await
        .expect("the rest of Holon must keep working");
    assert_eq!(people.len(), 1, "the person row must still be there");

    let write = stock(&later, "pantry-item:sugar", &[]).await;
    let error = write
        .expect_err("a write to the refused type must fail")
        .to_string();
    assert!(
        error.contains("pantry_item") && error.contains("type-table-refused"),
        "the write error must name the type and the condition: {error}"
    );

    let read = engine_read(&later, "from pantry_item").await;
    let error = read
        .expect_err("a read of the refused type must fail, not serve the undeclared shape")
        .to_string();
    assert!(
        error.contains("pantry_item") && error.contains("type-table-refused"),
        "the read error must name the type and the condition: {error}"
    );
    assert!(
        sql(
            &later,
            "SELECT name FROM sqlite_schema WHERE type = 'view' AND name = 'pantry_item'"
        )
        .await
        .is_empty(),
        "no stored view keeps serving the refused table"
    );
    assert_no_read_serves_the_refused_rows(&later).await;

    assert_eq!(
        stored_snapshot(&later, QUARANTINED).await,
        stored,
        "the refused rows are kept byte-equal in the quarantine table"
    );

    op(
        &later,
        "type_table",
        "drop_refused_table",
        params(&[("type", Value::String("pantry_item".into()))]),
    )
    .await
    .expect("the remedy must run");

    assert!(
        !conditions(&later.bus)
            .iter()
            .any(|c| c.condition_key().kind == "type-table-refused"),
        "the remedy must clear the condition"
    );
    assert_eq!(
        tables_named(&later, QUARANTINED).await,
        0,
        "the remedy drops the quarantined rows, as its label says"
    );
    assert!(
        sql(&later, "SELECT id FROM pantry_item_raw")
            .await
            .is_empty(),
        "the type is served from a new empty table"
    );
    stock(
        &later,
        "pantry-item:sugar",
        &[("unit", Value::String("g".into()))],
    )
    .await
    .expect("after the remedy the type must be writable");
    let sugar = engine_read(&later, "from pantry_item")
        .await
        .expect("after the remedy the type reads again");
    assert_eq!(
        sugar.first().and_then(|r| r.get("quantity")),
        Some(&Value::Float(1.0)),
        "the recreated table must have the declared shape: {sugar:?}"
    );
    op(
        &later,
        "pantry_item",
        "consume",
        params(&[
            ("id", Value::String("pantry-item:sugar".into())),
            ("quantity", Value::Float(0.25)),
            ("unit", Value::String("g".into())),
        ]),
    )
    .await
    .expect("the operations the type adds beside create come back with it");
    let consumed = sql(&later, "SELECT quantity FROM pantry_item").await;
    assert_eq!(
        consumed.first().and_then(|r| r.get("quantity")),
        Some(&Value::Float(0.75)),
        "consume ran against the recreated table: {consumed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_type_stays_quarantined_across_boots_until_the_remedy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stored = seed_refused_pantry(dir.path()).await;

    let first = boot(dir.path(), LATER_DB).await;
    assert_refusal_disclosed(&first);
    holon_app::shutdown_session(&first.injector)
        .await
        .expect("shut the first refusing session down");
    drop(first);

    let second = boot(dir.path(), LATER_DB).await;
    assert_refusal_disclosed(&second);
    assert_no_read_serves_the_refused_rows(&second).await;
    assert_eq!(
        tables_named(&second, "pantry_item_raw").await,
        0,
        "a second boot must not serve the refused type from a new empty table"
    );
    assert_eq!(
        stored_snapshot(&second, QUARANTINED).await,
        stored,
        "a second boot keeps the quarantined rows byte-equal, once"
    );

    op(
        &second,
        "type_table",
        "drop_refused_table",
        params(&[("type", Value::String("pantry_item".into()))]),
    )
    .await
    .expect("the remedy must run on a quarantined table");
    assert_eq!(tables_named(&second, QUARANTINED).await, 0);
    stock(&second, "pantry-item:sugar", &[])
        .await
        .expect("after the remedy the type must be writable");
    holon_app::shutdown_session(&second.injector)
        .await
        .expect("shut the remedied session down");
    drop(second);

    let third = boot(dir.path(), LATER_DB).await;
    let shown = described(&conditions(&third.bus));
    assert!(
        !shown
            .iter()
            .any(|c| c.starts_with("[type-table-refused]")
                || c.starts_with("[schema-module-failed]")),
        "a boot after the remedy refuses nothing: {shown:#?}"
    );
    let rows = engine_read(&third, "from pantry_item")
        .await
        .expect("a boot after the remedy reads the type");
    let ids: Vec<Option<&Value>> = rows.iter().map(|r| r.get("id")).collect();
    assert_eq!(
        ids,
        vec![Some(&Value::String("pantry-item:sugar".into()))],
        "the row written after the remedy survives the reboot"
    );
}

/// Run `statements` on the database between two sessions.
fn between_sessions(dir: &Path, statements: &[&str]) {
    let db = holon::storage::turso::TursoBackend::open_database(dir.join(LATER_DB))
        .expect("open between the sessions");
    let conn = db.connect().expect("connect");
    for statement in statements {
        exec(&conn, statement);
    }
}

/// The stored tables whose name starts with [`QUARANTINED`].
async fn quarantine_tables(booted: &Booted) -> Vec<String> {
    sql(
        booted,
        &format!(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND substr(name, 1, {}) = \
             '{QUARANTINED}' ORDER BY name",
            QUARANTINED.len()
        ),
    )
    .await
    .iter()
    .map(|row| match row.get("name") {
        Some(Value::String(name)) => name.clone(),
        other => panic!("table name {other:?}"),
    })
    .collect()
}

/// The pantry_item refusal names every table holding rows it keeps unread,
/// and nothing claims it failed in some other way.
async fn assert_held_disclosed(booted: &Booted) {
    let current = conditions(&booted.bus);
    let shown = described(&current);
    assert!(
        !shown
            .iter()
            .any(|c| c.starts_with("[schema-module-failed]") && c.contains("pantry_item")),
        "a held type is disclosed as refused, not as a failed setup: {shown:#?}"
    );
    let refused = current
        .iter()
        .find(|c| c.condition_key().kind == "type-table-refused" && c.subject == "pantry_item")
        .unwrap_or_else(|| panic!("the held type is not disclosed: {shown:#?}"));
    let text = described(std::slice::from_ref(refused)).join("");
    for table in quarantine_tables(booted).await {
        assert!(
            text.contains(&format!("{table} (")),
            "the refusal must name {table}, which holds rows it keeps unread: {text}"
        );
    }
}

async fn run_remedy_then_write(booted: &Booted) {
    op(
        booted,
        "type_table",
        "drop_refused_table",
        params(&[("type", Value::String("pantry_item".into()))]),
    )
    .await
    .expect("the remedy must run");
    assert!(
        !conditions(&booted.bus)
            .iter()
            .any(|c| c.condition_key().kind == "type-table-refused"),
        "the remedy must clear the condition"
    );
    assert_eq!(
        quarantine_tables(booted).await,
        Vec::<String>::new(),
        "the remedy drops every table its condition named"
    );
    stock(booted, "pantry-item:sugar", &[])
        .await
        .expect("after the remedy the type must be writable");
    let rows = engine_read(booted, "from pantry_item")
        .await
        .expect("after the remedy the type reads again");
    assert_eq!(
        rows.len(),
        1,
        "only the row written after the remedy: {rows:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_table_already_named_like_the_quarantine_does_not_let_the_refused_rows_be_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stored = seed_refused_pantry(dir.path()).await;
    between_sessions(
        dir.path(),
        &[
            &format!("CREATE TABLE {QUARANTINED} (mine TEXT)"),
            &format!("INSERT INTO {QUARANTINED} (mine) VALUES ('users own table')"),
        ],
    );

    let later = boot(dir.path(), LATER_DB).await;
    assert_no_read_serves_the_refused_rows(&later).await;
    assert_eq!(
        tables_named(&later, "pantry_item_raw").await,
        0,
        "the refused table is moved out of its name"
    );
    let held = quarantine_tables(&later).await;
    assert_eq!(held.len(), 2, "both tables are kept: {held:?}");
    let refused_copy = held
        .iter()
        .find(|t| t.as_str() != QUARANTINED)
        .expect("the refused rows are kept under a free name");
    assert_eq!(
        stored_snapshot(&later, refused_copy).await,
        stored,
        "the refused rows are kept byte-equal"
    );
    assert_eq!(
        sql(&later, &format!("SELECT mine FROM {QUARANTINED}"))
            .await
            .len(),
        1,
        "the table that held the name is untouched"
    );
    assert_held_disclosed(&later).await;
    run_remedy_then_write(&later).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_raw_table_recreated_beside_the_quarantine_is_not_served() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stored = seed_refused_pantry(dir.path()).await;
    let first = boot(dir.path(), LATER_DB).await;
    assert_refusal_disclosed(&first);
    holon_app::shutdown_session(&first.injector)
        .await
        .expect("shut the refusing session down");
    drop(first);
    between_sessions(
        dir.path(),
        &[
            "CREATE TABLE pantry_item_raw (id TEXT PRIMARY KEY, quantity REAL)",
            "INSERT INTO pantry_item_raw (id, quantity) VALUES ('pantry-item:flour', 2.0)",
        ],
    );

    let second = boot(dir.path(), LATER_DB).await;
    assert_no_read_serves_the_refused_rows(&second).await;
    assert_eq!(
        tables_named(&second, "pantry_item_raw").await,
        0,
        "neither table holding pantry rows is served"
    );
    assert_eq!(
        stored_snapshot(&second, QUARANTINED).await,
        stored,
        "the first quarantine keeps its rows byte-equal"
    );
    assert_eq!(quarantine_tables(&second).await.len(), 2);
    assert_held_disclosed(&second).await;
    run_remedy_then_write(&second).await;
}
