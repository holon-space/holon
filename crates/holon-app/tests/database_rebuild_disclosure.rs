//! A database this binary cannot use is deleted and rebuilt at boot (Martin:
//! no migration). The org files rebuild the blocks, but some tables hold state
//! that no replica has. Their loss is disclosed: a condition that stays until
//! restart names each lost table with the rows it held.

use std::collections::HashSet;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const VAULT_ORG: &str = "\
* Rebuild probe page
:PROPERTIES:
:ID: rebuild-probe-page
:END:
** A child block
:PROPERTIES:
:ID: rebuild-probe-child
:END:
Some text to edit.
";

const DB_FILE: &str = "rebuild.db";
/// The first session's file as a later process finds it. A copy, because the
/// first session's engine stays alive in this process and a rebuild refuses to
/// delete a file another holder in the process has open.
const LATER_DB_FILE: &str = "later.db";

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    _session: Arc<FrontendSession>,
}

async fn boot(dir: &std::path::Path, db_file: &str) -> Booted {
    let config = HolonConfig {
        db_path: Some(dir.join(db_file)),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        std::sync::Arc::new(holon_api::ConditionBus::new()),
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
        _session: session,
    }
}

/// The table names a booted session holds, engine-internal ones excluded.
async fn tables(booted: &Booted) -> Vec<String> {
    booted
        .engine
        .db_handle()
        .query(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             AND name NOT LIKE '__turso%' ORDER BY name",
            Default::default(),
        )
        .await
        .expect("list tables")
        .iter()
        .map(|row| match row.get("name") {
            Some(Value::String(name)) => name.clone(),
            other => panic!("table name is {other:?}"),
        })
        .collect()
}

/// Create the cache tables of every bundled integration through the path a
/// connection takes, with no network.
async fn create_integration_caches(booted: &Booted) {
    let db = booted.engine.db_handle().clone();
    let factory: Arc<dyn holon_core::entity_cache::CacheFactory> =
        Arc::new(holon::di::DbHandleCacheFactory::new(db.clone()));
    for bundled in holon_mcp_client::BUNDLED_SIDECARS {
        let sidecar = holon_mcp_client::McpSidecar::from_yaml(bundled.yaml)
            .unwrap_or_else(|e| panic!("{}: {e:#}", bundled.source_path));
        holon_mcp_client::mcp_integration::build_entity_caches(
            &sidecar,
            bundled.provider,
            &factory,
            &db,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {e:#}", bundled.source_path));
    }
}

/// The row count of every table in `tables` the database holds.
fn row_counts(conn: &Arc<turso_core::Connection>, tables: &[&str]) -> Vec<(String, i64)> {
    let query = |sql: &str| {
        conn.prepare(sql)
            .and_then(|mut stmt| stmt.run_collect_rows())
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    };
    tables
        .iter()
        .filter(|table| {
            !query(&format!(
                "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '{table}'"
            ))
            .is_empty()
        })
        .map(|table| {
            let rows = query(&format!("SELECT count(*) FROM {table}"));
            match rows.as_slice() {
                [row] => match row.as_slice() {
                    [turso_core::Value::Numeric(turso_core::Numeric::Integer(n))] => {
                        (table.to_string(), *n)
                    }
                    other => panic!("count of {table} is {other:?}"),
                },
                other => panic!("count of {table} returned {} rows", other.len()),
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rebuild_discloses_every_lost_table_with_its_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");

    {
        let first = boot(dir.path(), DB_FILE).await;
        let mut params: holon_api::StorageEntity = std::collections::HashMap::new();
        params.insert(
            "id".into(),
            Value::String("block:rebuild-probe-child".into()),
        );
        params.insert("field".into(), Value::String("content".into()));
        params.insert("value".into(), Value::String("edited before".into()));
        first
            .engine
            .execute_operation(
                &holon_api::EntityName::from("block"),
                "set_field",
                params,
                OpOrigin::User,
            )
            .await
            .expect("a user edit, so the journal holds a row");
        create_integration_caches(&first).await;
        for id in ["p1", "p2"] {
            first
                .engine
                .db_handle()
                .execute_values(
                    "INSERT INTO todoist_projects (id, name, color, isFavorite, isShared, \
                     inboxProject, viewStyle, childOrder, parentId) VALUES (?, 'a project', \
                     'red', 0, 0, 0, 'list', 0, '')",
                    vec![Value::String(id.into())],
                )
                .await
                .expect("a synced cache row");
        }
        holon_app::shutdown_session(&first.injector)
            .await
            .expect("shut the first session down");
    }

    for suffix in ["", "-wal"] {
        let from = dir.path().join(format!("{DB_FILE}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, dir.path().join(format!("{LATER_DB_FILE}{suffix}")))
                .expect("copy the first session's database");
        }
    }

    let lost_names: Vec<&str> = holon::storage::table_classes::lost_tables()
        .iter()
        .map(|(table, _)| *table)
        .collect();
    // A binary with another function set built this file, so this binary
    // cannot use it. It also holds a table no classification names.
    let expected_lost = {
        let db = holon::storage::turso::TursoBackend::open_database(dir.path().join(LATER_DB_FILE))
            .expect("open between the sessions");
        let conn = db.connect().expect("connect");
        let counts = row_counts(&conn, &lost_names);
        conn.execute("CREATE TABLE IF NOT EXISTS holon_db_fn_set (signature TEXT NOT NULL)")
            .expect("create the record");
        conn.execute("DELETE FROM holon_db_fn_set")
            .expect("empty the record");
        conn.execute("INSERT INTO holon_db_fn_set (signature) VALUES ('foreign_fn/1/v1')")
            .expect("record a foreign set");
        conn.execute("CREATE TABLE rebuild_sentinel (x INTEGER)")
            .expect("create an unclassified table");
        conn.execute("INSERT INTO rebuild_sentinel (x) VALUES (1), (2), (3)")
            .expect("fill it");
        counts
    };
    assert!(
        expected_lost.iter().any(|(_, n)| *n > 0),
        "the first session left no lost-class row, so the counts prove nothing: {expected_lost:?}"
    );

    let second = boot(dir.path(), LATER_DB_FILE).await;
    assert!(
        !tables(&second)
            .await
            .contains(&"rebuild_sentinel".to_string()),
        "the database must have been rebuilt, or this test proves nothing"
    );

    let bus = second.injector.resolve::<Arc<holon_api::ConditionBus>>();
    let current = bus.subscribe().current;
    let raised: Vec<(String, String)> = current
        .iter()
        .map(|c| {
            (
                c.condition_key().kind.to_string(),
                format!("{:?}", c.reason),
            )
        })
        .collect();
    let rebuilt = raised
        .iter()
        .find(|(kind, _)| kind == "database-rebuilt-at-boot")
        .unwrap_or_else(|| {
            panic!("a rebuild deleted state no replica holds and disclosed nothing: {raised:?}")
        });
    // The Debug payload above pins what the engine COMPUTED; this pins what
    // Martin actually READS — the rendered banner `ConditionKind::detail()`
    // builds, which is a separate hand-written formatter that can drift from
    // the payload it renders.
    let rebuilt_condition = current
        .iter()
        .find(|c| c.condition_key().kind == "database-rebuilt-at-boot")
        .expect("the same condition, by identity");
    let rendered_detail = {
        let detail = rebuilt_condition.reason.detail(&rebuilt_condition.subject);
        format!("{} {}", detail.headline, detail.body.join(" | "))
    };
    for (table, rows) in &expected_lost {
        let what = holon::storage::table_classes::lost_tables()
            .iter()
            .find(|(name, _)| name == table)
            .expect("a lost table")
            .1;
        let entry = format!("LostTableRows {{ table: {table:?}, what: {what:?}, rows: {rows} }}");
        assert!(
            rebuilt.1.contains(&entry),
            "the disclosure must name {table} with its {rows} rows: {}",
            rebuilt.1
        );
        let rendered_entry = format!("{table}: {what} ({rows} rows)");
        assert!(
            rendered_detail.contains(&rendered_entry),
            "the text Martin reads must name {table} with its {rows} rows: {rendered_detail}"
        );
    }
    for entry in [
        "LostTableRows { table: \"rebuild_sentinel\", what: \"a table no classification names\", \
         rows: 3 }",
        "ClearedCacheRows { table: \"todoist_projects\", provider: \"todoist\", rows: 2 }",
    ] {
        assert!(
            rebuilt.1.contains(entry),
            "the disclosure must hold {entry}: {}",
            rebuilt.1
        );
    }
    for rendered_entry in [
        "rebuild_sentinel: a table no classification names (3 rows)",
        "todoist_projects: todoist cache, re-syncing (2 rows)",
    ] {
        assert!(
            rendered_detail.contains(rendered_entry),
            "the text Martin reads must hold {rendered_entry:?}: {rendered_detail}"
        );
    }
}

/// Every table a booted session holds, with every bundled integration's caches
/// created, is classified: rebuilt from a replica, an integration cache, or
/// lost at a rebuild. A new table cannot be added without a decision.
#[tokio::test(flavor = "multi_thread")]
async fn every_table_is_classified_as_rebuilt_or_lost() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let booted = boot(dir.path(), DB_FILE).await;
    create_integration_caches(&booted).await;
    let caches: std::collections::HashMap<String, String> = booted
        .engine
        .db_handle()
        .query(
            "SELECT table_name, provider FROM integration_cache",
            Default::default(),
        )
        .await
        .expect("read integration_cache")
        .iter()
        .map(|row| match (row.get("table_name"), row.get("provider")) {
            (Some(Value::String(table)), Some(Value::String(provider))) => {
                (table.clone(), provider.clone())
            }
            other => panic!("integration_cache row is {other:?}"),
        })
        .collect();
    assert!(
        caches.contains_key("cc_message") && caches.contains_key("todoist_projects"),
        "creating the bundled caches recorded {caches:?}"
    );
    let all_tables = tables(&booted).await;
    assert!(
        !all_tables.is_empty(),
        "a booted session with no tables classifies vacuously — this pins nothing"
    );
    let unclassified: Vec<String> = all_tables
        .into_iter()
        .filter(|table| holon::storage::table_classes::class_of(table, &caches).is_none())
        .collect();
    assert!(
        unclassified.is_empty(),
        "classify these tables in crates/holon-turso/src/table_classes.rs: {unclassified:?}"
    );
}
