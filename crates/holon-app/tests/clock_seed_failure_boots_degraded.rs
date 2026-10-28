//! A `clock` table the scheduler cannot seed must not stop the boot.
//!
//! The create leg is covered by `core_schema_failure_boots_degraded`: there the
//! core schema cannot make `clock` a table at all. Here the table IS there and
//! holds the schema's placeholder row, but the scheduler's seeding write is
//! refused, so the day row keeps the 1970 placeholder. The boot must degrade,
//! disclose the clock on the condition bus with the fix it offers, and come up.
//!
//! @pbt kind harness
//! @pbt covers clock-seed-failure-boots-degraded — a refused clock seed is
//! disclosed with its fix, not fatal
//! @pbt overlaps core-schema-failure-boots-degraded — kept: there the clock
//! relation cannot be created, here it exists and only the seed fails

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_conditions::Condition;
use holon_conditions::ConditionBus;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const DB_FILE: &str = "clock-seed.db";

/// A `clock` table in the declared shape whose `epoch_day` may only ever be
/// the placeholder `0`: the schema's placeholder INSERT lands, every seeding
/// UPDATE the scheduler issues is refused.
const FROZEN_CLOCK: &str = "CREATE TABLE clock (grain TEXT PRIMARY KEY NOT NULL, today TEXT NOT \
                            NULL, epoch_day INTEGER NOT NULL CHECK (epoch_day = 0), updated_at \
                            TEXT NOT NULL)";

fn seed(dir: &Path, statements: &[&str]) {
    let db = holon::storage::turso::TursoBackend::open_database(dir.join(DB_FILE))
        .expect("open the database to seed");
    let conn = db.connect().expect("connect");
    for sql in statements {
        conn.execute(*sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

/// Boot the shipped wiring over `dir`; hand back the conditions in effect and
/// the `day` grain's stored `epoch_day` once the session is up.
async fn boot(dir: &Path) -> (Vec<Condition>, i64) {
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
    .expect("a refused clock seed must boot a degraded session, never abort");
    let current = bus.subscribe().current;
    let rows = engine
        .db_handle()
        .query(
            "SELECT epoch_day FROM clock WHERE grain = 'day'",
            HashMap::new(),
        )
        .await
        .expect("read the clock day row");
    let epoch_day = match rows.first().and_then(|row| row.get("epoch_day")) {
        Some(holon_api::Value::Integer(day)) => *day,
        other => panic!("clock.epoch_day {other:?}"),
    };
    drop(session);
    (current, epoch_day)
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
async fn a_clock_seed_the_database_refuses_is_disclosed_with_its_fix_and_the_boot_continues() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(dir.path(), &[FROZEN_CLOCK]);

    let (conditions, epoch_day) = boot(dir.path()).await;
    let described = described(&conditions);

    let clock = described
        .iter()
        .find(|c| c.starts_with("[clock-not-running] clock"))
        .unwrap_or_else(|| panic!("the clock that did not seed is disclosed: {described:#?}"));
    assert!(
        clock.contains("next Holon start"),
        "the disclosure offers the fix: {clock}"
    );
    assert!(
        !described
            .iter()
            .any(|c| c.starts_with("[schema-module-failed] core")),
        "the core schema adopted the stored clock table — only its seed failed: {described:#?}"
    );
    assert_eq!(
        epoch_day, 0,
        "the refused seed leaves the placeholder day row in place"
    );
}
