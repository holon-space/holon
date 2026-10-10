//! What a boot does with a database file it cannot use as it is. A file the
//! engine cannot open is moved aside and disclosed, never deleted; a file built
//! by another binary keeps its rows and gets its views built again. Every
//! table is classified, so a drifted one is reshaped or rebuilt on purpose.

use std::collections::HashSet;
use std::sync::Arc;

use holon::api::BackendEngine;
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

/// The view names a booted session holds.
async fn views(booted: &Booted) -> Vec<String> {
    names(
        booted,
        "SELECT name FROM sqlite_schema WHERE type = 'view' ORDER BY name",
    )
    .await
}

async fn names(booted: &Booted, sql: &str) -> Vec<String> {
    booted
        .engine
        .db_handle()
        .query(sql, Default::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|row| match row.get("name") {
            Some(Value::String(name)) => name.clone(),
            other => panic!("{sql}: name is {other:?}"),
        })
        .collect()
}

fn raised_kinds(booted: &Booted) -> Vec<String> {
    booted
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current
        .iter()
        .map(|c| c.condition_key().kind.to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_database_the_engine_cannot_open_is_moved_aside_and_disclosed() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let garbage = b"not a database, but the only copy of something".repeat(100);
    std::fs::write(dir.path().join(DB_FILE), &garbage).expect("write an unopenable file");

    let booted = boot(dir.path(), DB_FILE).await;

    let backups: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
        .expect("list the vault")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{DB_FILE}.unusable-")))
        })
        .collect();
    let backup = match backups.as_slice() {
        [backup] => backup.clone(),
        other => panic!("expected one moved-aside file, found {other:?}"),
    };
    assert_eq!(
        std::fs::read(&backup).expect("read the backup"),
        garbage,
        "the moved-aside file must hold the old bytes"
    );

    let current = booted
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current;
    let moved = current
        .iter()
        .find(|c| c.condition_key().kind == "database-moved-aside")
        .unwrap_or_else(|| {
            panic!(
                "a database moved aside at boot was not disclosed: {:?}",
                raised_kinds(&booted)
            )
        });
    let profile = moved.reason.profile();
    assert_ne!(
        profile.severity(),
        holon_api::ConditionSeverity::Info,
        "the move must not be an info-level notice"
    );
    assert_eq!(profile.all_clear(), holon_api::AllClear::UntilRestart);
    let detail = moved.reason.detail(&moved.subject);
    let rendered = format!("{} {}", detail.headline, detail.body.join(" | "));
    for needle in [
        backup.display().to_string(),
        "only in the database is in the moved file".to_string(),
    ] {
        assert!(
            rendered.contains(&needle),
            "the text Martin reads must hold {needle:?}: {rendered}"
        );
    }

    let blocks = names(
        &booted,
        "SELECT id AS name FROM block WHERE id = 'block:rebuild-probe-child'",
    )
    .await;
    assert_eq!(
        blocks,
        vec!["block:rebuild-probe-child".to_string()],
        "the fresh database must hold the vault's blocks"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_builds_the_same_views_as_a_fresh_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");

    let fresh_views = {
        let first = boot(dir.path(), DB_FILE).await;
        let views = views(&first).await;
        holon_app::shutdown_session(&first.injector)
            .await
            .expect("shut the first session down");
        views
    };
    assert!(!fresh_views.is_empty(), "a fresh boot built no view");

    for suffix in ["", "-wal"] {
        let from = dir.path().join(format!("{DB_FILE}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, dir.path().join(format!("{LATER_DB_FILE}{suffix}")))
                .expect("copy the first session's database");
        }
    }
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

    let upgraded = boot(dir.path(), LATER_DB_FILE).await;
    assert_eq!(
        views(&upgraded).await,
        fresh_views,
        "the boot after an upgrade must build the views a fresh boot builds"
    );
    assert_eq!(
        holon_turso::dbsp_state::left_behind_in(upgraded.engine.db_handle()).await,
        Vec::<String>::new(),
        "a dropped view must not leave DBSP state of any circuit version behind"
    );
    assert!(
        !raised_kinds(&upgraded)
            .iter()
            .any(|k| k == "database-moved-aside"),
        "an upgrade must keep the file in place"
    );
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
