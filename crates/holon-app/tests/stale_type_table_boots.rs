//! A stored `<type>_raw` table whose shape differs from the type's declaration
//! does not stop the boot.
//!
//! A column the declaration added (nullable, or with a default) is added to the
//! stored table, the rows survive, and an info condition says so. Any other
//! difference refuses only that type: the rest of Holon works, a condition
//! names the type, the table, the difference and the row count, writes to the
//! type fail naming that condition, and no row is deleted until the user runs
//! the condition's remedy.
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
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::EntityName;
use holon_api::OpOrigin;
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

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_column_type_refuses_only_that_type_until_the_user_drops_the_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let all = format!("{KEPT}, product_id");
    seed_then_reshape(dir.path(), |conn| {
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
    })
    .await;

    let later = boot(dir.path(), LATER_DB).await;

    let current = conditions(&later.bus);
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

    let kept = sql(&later, "SELECT id FROM pantry_item_raw").await;
    assert_eq!(kept.len(), 2, "a refusal must not delete a row");

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
    assert!(
        sql(&later, "SELECT id FROM pantry_item_raw")
            .await
            .is_empty(),
        "the remedy drops the stored rows, as its label says"
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
