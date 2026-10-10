//! A binary with another scalar-function set opens a file-backed database: the
//! views and the `block_derived` cache are dropped, every table row stays, and
//! the next boot builds the same views and refills the cache with what the
//! computations give now.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::ConditionBus;
use holon_api::Value;
use holon_api::computation::ArithOp;
use holon_api::computation::Computation;
use holon_api::computation::DerivedField;
use holon_api::computation::FieldIdent;
use holon_turso::db_open::OpenOutcome;
use holon_turso::derived_reconciler::DerivedFieldReconcilerHandle;
use holon_turso::derived_reconciler::spawn_derived_field_reconciler;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockDerivedSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const SOURCE_SQL: &str = "SELECT id, priority FROM task";
const PRIORITIES: &[(&str, i64)] = &[("t1", 3), ("t2", 1)];

fn boosted() -> DerivedField {
    DerivedField::new(
        FieldIdent::parse("boosted").expect("identifier"),
        Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("priority".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(2))),
        },
    )
}

fn expected_json(priority: i64) -> String {
    let ctx = HashMap::from([("priority".to_string(), Value::Integer(priority))]);
    serde_json::to_string(&boosted().computation.eval(&ctx).expect("eval")).expect("json")
}

async fn names(handle: &DbHandle, sql: &str) -> BTreeSet<String> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|row| match row.get("name") {
            Some(Value::String(name)) => name.clone(),
            other => panic!("{sql}: name is {other:?}"),
        })
        .collect()
}

async fn views(handle: &DbHandle) -> BTreeSet<String> {
    names(handle, "SELECT name FROM sqlite_schema WHERE type = 'view'").await
}

async fn count(handle: &DbHandle, table: &str) -> i64 {
    let rows = handle
        .query(
            &format!("SELECT count(*) AS n FROM {table}"),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("count {table}: {e}"));
    match rows.first().and_then(|r| r.get("n")) {
        Some(Value::Integer(n)) => *n,
        other => panic!("count {table} is {other:?}"),
    }
}

async fn await_derived_filled(handle: &DbHandle) {
    for _ in 0..250 {
        let rows = handle
            .query(
                "SELECT block_id, value_json FROM block_derived WHERE field_name = 'boosted'",
                HashMap::new(),
            )
            .await
            .expect("read block_derived");
        let got: BTreeSet<(String, String)> = rows
            .iter()
            .map(|r| match (r.get("block_id"), r.get("value_json")) {
                (Some(Value::String(id)), Some(Value::String(v))) => (id.clone(), v.clone()),
                other => panic!("block_derived row is {other:?}"),
            })
            .collect();
        let want: BTreeSet<(String, String)> = PRIORITIES
            .iter()
            .map(|(id, p)| (id.to_string(), expected_json(*p)))
            .collect();
        if got == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("block_derived never held the computed values of {PRIORITIES:?}");
}

struct Boot {
    _backend: TursoBackend,
    handle: DbHandle,
    _reconciler: DerivedFieldReconcilerHandle,
}

async fn boot(db: Arc<turso_core::Database>) -> Boot {
    let (cdc_tx, _) = tokio::sync::broadcast::channel(64);
    let (backend, handle) = TursoBackend::new(db, cdc_tx).expect("backend");
    handle.transition_to_ready().await.expect("ready");
    handle
        .execute_ddl("CREATE TABLE IF NOT EXISTS task (id TEXT PRIMARY KEY, priority INTEGER)")
        .await
        .expect("create task");
    BlockDerivedSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_derived");
    let manager = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let reconciler = spawn_derived_field_reconciler(
        &manager,
        handle.clone(),
        SOURCE_SQL,
        vec![boosted()],
        Arc::new(ConditionBus::new()),
    )
    .await
    .expect("spawn the reconciler");
    Boot {
        _backend: backend,
        handle,
        _reconciler: reconciler,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_rebuilds_the_views_and_refills_block_derived() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("holon.db");

    let first_views = {
        let first = boot(TursoBackend::open_database(&path).expect("open boot 1")).await;
        for (id, priority) in PRIORITIES {
            first
                .handle
                .execute(
                    "INSERT INTO task (id, priority) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(id.to_string()),
                        turso::Value::Integer(*priority),
                    ],
                )
                .await
                .expect("insert task");
        }
        await_derived_filled(&first.handle).await;
        let views = views(&first.handle).await;
        assert!(
            !views.is_empty(),
            "boot 1 built no view, so this proves nothing"
        );
        first.handle.shutdown().await.expect("shut boot 1 down");
        views
    };

    {
        let db = TursoBackend::open_database(&path).expect("open between boots");
        let conn = db.connect().expect("connect");
        for sql in [
            "DELETE FROM holon_db_fn_set",
            "INSERT INTO holon_db_fn_set (signature) VALUES ('older_fn/1/v1')",
        ] {
            conn.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        }
    }

    let (db, outcome) =
        TursoBackend::open_database_reporting_outcome(&path).expect("open after the upgrade");
    match &outcome {
        OpenOutcome::ViewsDropped { views, .. } => assert_eq!(
            views.iter().cloned().collect::<BTreeSet<_>>(),
            first_views,
            "the upgrade must drop exactly the views boot 1 built"
        ),
        other => panic!("an upgrade must drop the views, got {other:?}"),
    }
    {
        let conn = db.connect().expect("connect");
        let rows = conn
            .prepare("SELECT count(*) FROM block_derived")
            .and_then(|mut stmt| stmt.run_collect_rows())
            .expect("count block_derived");
        assert_eq!(
            format!("{rows:?}"),
            format!("{:?}", vec![vec![turso_core::Value::from_i64(0)]]),
            "the upgrade must empty block_derived, or its refill proves nothing"
        );
    }
    let second = boot(db).await;
    assert_eq!(count(&second.handle, "task").await, PRIORITIES.len() as i64);
    await_derived_filled(&second.handle).await;
    assert_eq!(
        views(&second.handle).await,
        first_views,
        "the boot after an upgrade must build the same views as a fresh boot"
    );
    assert_eq!(
        holon_turso::dbsp_state::left_behind_in(&second.handle).await,
        Vec::<String>::new(),
        "a dropped view must not leave DBSP state of any circuit version behind"
    );
    second.handle.shutdown().await.expect("shut boot 2 down");
}
