//! Without the Loro store (`crdt.enabled = false`) the database is the only
//! durable copy of every row the org files do not hold. A binary with another
//! scalar-function set (an ordinary upgrade) must keep those rows.

use std::collections::HashSet;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::CrdtPreferences;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const VAULT_ORG: &str = "\
* Upgrade probe page
:PROPERTIES:
:ID: upgrade-probe-page
:END:
** A child block
:PROPERTIES:
:ID: upgrade-probe-child
:END:
Some text.
";

const BLOCK: &str = "block:upgrade-probe-child";
/// Write-back erases `_`-prefixed property keys from the org file, so this
/// value lives only in the database.
const SQL_ONLY_KEY: &str = "_holon_upgrade_probe";
const DB_FILE: &str = "upgrade.db";
/// The first session's file as a later process finds it. A copy, because the
/// first session's engine stays alive in this process.
const LATER_DB_FILE: &str = "later.db";

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    _session: Arc<FrontendSession>,
}

async fn boot_sql_only(dir: &std::path::Path, db_file: &str) -> Booted {
    let config = HolonConfig {
        db_path: Some(dir.join(db_file)),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        crdt: CrdtPreferences {
            enabled: Some(false),
            ..Default::default()
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(holon_api::ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await
    .expect("a SqlOnly session must boot");
    Booted {
        engine,
        injector,
        _session: session,
    }
}

async fn op(booted: &Booted, entity: &str, op: &str, params: &[(&str, &str)]) {
    let params: holon_api::StorageEntity = params
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String(v.to_string())))
        .collect();
    booted
        .engine
        .execute_operation(
            &holon_api::EntityName::from(entity),
            op,
            params,
            OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("{entity}.{op}: {e:#}"));
}

/// The SQL-only facts this test follows: the probe property's stored value,
/// and the navigation entries that name the probe block.
async fn sql_only_facts(booted: &Booted) -> (Option<Value>, i64) {
    let db = booted.engine.db_handle();
    let properties = db
        .query(
            &format!("SELECT properties FROM block_raw WHERE id = '{BLOCK}'"),
            Default::default(),
        )
        .await
        .expect("read the probe block");
    let properties = match properties.as_slice() {
        [row] => row.get("properties").cloned(),
        other => panic!("{BLOCK} has {} rows in block_raw: {other:?}", other.len()),
    };
    let nav = db
        .query(
            &format!("SELECT count(*) AS n FROM navigation_history WHERE block_id = '{BLOCK}'"),
            Default::default(),
        )
        .await
        .expect("read the navigation history");
    let nav = match nav.as_slice() {
        [row] => match row.get("n") {
            Some(Value::Integer(n)) => *n,
            other => panic!("navigation count is {other:?}"),
        },
        other => panic!("navigation count returned {} rows", other.len()),
    };
    (properties, nav)
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlonly_rows_not_in_org_files_survive_a_function_set_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let org_path = dir.path().join("probe.org");
    std::fs::write(&org_path, VAULT_ORG).expect("write the probe vault");

    let before = {
        let first = boot_sql_only(dir.path(), DB_FILE).await;
        op(
            &first,
            "block",
            "set_field",
            &[("id", BLOCK), ("field", SQL_ONLY_KEY), ("value", "kept")],
        )
        .await;
        op(
            &first,
            "navigation",
            "focus",
            &[("region", "main"), ("block_id", BLOCK)],
        )
        .await;
        let facts = sql_only_facts(&first).await;
        holon_app::shutdown_session(&first.injector)
            .await
            .expect("shut the first session down");
        facts
    };
    assert!(
        format!("{:?}", before.0).contains(SQL_ONLY_KEY) && before.1 > 0,
        "the first session must hold both SQL-only facts, or this test proves nothing: {before:?}"
    );
    let org = std::fs::read_to_string(&org_path).expect("read the org file back");
    assert!(
        !org.contains(SQL_ONLY_KEY),
        "the probe key reached the org file, so it is not SQL-only and proves nothing:\n{org}"
    );

    for suffix in ["", "-wal"] {
        let from = dir.path().join(format!("{DB_FILE}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, dir.path().join(format!("{LATER_DB_FILE}{suffix}")))
                .expect("copy the first session's database");
        }
    }
    // An older binary with another function set built this file.
    {
        let db = holon::storage::turso::TursoBackend::open_database(dir.path().join(LATER_DB_FILE))
            .expect("open between the sessions");
        let conn = db.connect().expect("connect");
        for sql in [
            "CREATE TABLE IF NOT EXISTS holon_db_fn_set (signature TEXT NOT NULL)",
            "DELETE FROM holon_db_fn_set",
            "INSERT INTO holon_db_fn_set (signature) VALUES ('older_fn/1/v1')",
        ] {
            conn.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        }
    }

    let second = boot_sql_only(dir.path(), LATER_DB_FILE).await;
    let after = sql_only_facts(&second).await;
    assert_eq!(
        after, before,
        "a function-set change lost rows that only the SqlOnly database held"
    );
    let raised: Vec<String> = second
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current
        .iter()
        .map(|c| c.condition_key().kind.to_string())
        .collect();
    assert!(
        !raised.iter().any(|kind| kind == "database-moved-aside"),
        "an upgrade must not move the database aside: {raised:?}"
    );
}
