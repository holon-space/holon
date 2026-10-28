//! A stored core relation the core schema cannot adopt must not stop the boot.
//!
//! A stored `block_raw` with a row its declaration cannot hold fails
//! `CoreSchemaModule`; a stored `clock` view in place of the `clock` table
//! fails it and the clock scheduler too. Either way the stored rows stay
//! where they are (D-db-keep.a), each failure is disclosed on the condition
//! bus, and the session boots.
//!
//! @pbt kind harness
//! @pbt covers core-schema-failure-boots-degraded — a stored block_raw or
//! clock the core schema refuses is disclosed, not fatal
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone always boots a
//! database its own schema created

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_conditions::Condition;
use holon_conditions::ConditionBus;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const DB_FILE: &str = "foreign.db";

fn seed(dir: &Path, statements: &[&str]) {
    let db = holon::storage::turso::TursoBackend::open_database(dir.join(DB_FILE))
        .expect("open the database to seed");
    let conn = db.connect().expect("connect");
    for sql in statements {
        conn.execute(*sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

/// Boot the shipped wiring over `dir`; hand back the conditions in effect and
/// the ids `block_raw` holds once the session is up.
async fn boot(dir: &Path) -> (Vec<Condition>, Vec<String>) {
    let config = HolonConfig {
        db_path: Some(dir.join(DB_FILE)),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let bus = Arc::new(ConditionBus::new());
    let (session, engine, ()) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        bus.clone(),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |_| (),
    )
    .await
    .expect("a core schema failure must boot a degraded session, never abort");
    let current = bus.subscribe().current;
    let ids = engine
        .db_handle()
        .query("SELECT id FROM block_raw ORDER BY id", HashMap::new())
        .await
        .expect("read block_raw")
        .into_iter()
        .map(|row| match row.get("id") {
            Some(holon_api::Value::String(id)) => id.clone(),
            other => panic!("block_raw.id {other:?}"),
        })
        .collect();
    drop(session);
    (current, ids)
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

#[tokio::test(flavor = "multi_thread")]
async fn a_block_raw_the_core_schema_refuses_is_disclosed_and_the_boot_continues() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(
        dir.path(),
        &[
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, sort_key TEXT, content \
             TEXT)",
            "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES ('block:kept', \
             NULL, NULL, 'a row the declaration cannot hold')",
        ],
    );

    let (conditions, ids) = boot(dir.path()).await;
    let conditions = described(&conditions);

    assert!(
        conditions
            .iter()
            .any(|c| c.starts_with("[schema-module-failed] core") && c.contains("block_raw")),
        "the core schema failure is disclosed naming block_raw: {conditions:#?}"
    );
    assert_eq!(
        ids,
        vec!["block:kept".to_string()],
        "the stored rows stay where they were"
    );
    assert!(
        !conditions
            .iter()
            .any(|c| c.starts_with("[clock-not-running]")),
        "the clock does not depend on block_raw: {conditions:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clock_relation_the_core_schema_cannot_create_is_disclosed_and_the_boot_continues() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(dir.path(), &["CREATE VIEW clock AS SELECT 1 AS grain"]);

    let (conditions, _) = boot(dir.path()).await;
    let conditions = described(&conditions);

    assert!(
        conditions
            .iter()
            .any(|c| c.starts_with("[schema-module-failed] core") && c.contains("clock")),
        "the core schema failure is disclosed naming clock: {conditions:#?}"
    );
    assert!(
        conditions
            .iter()
            .any(|c| c.starts_with("[clock-not-running] clock")),
        "the clock that did not start is disclosed: {conditions:#?}"
    );
}
