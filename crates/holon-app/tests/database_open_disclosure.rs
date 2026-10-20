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
    boot_with_crdt(dir, db_file, true).await
}

async fn boot_with_crdt(dir: &std::path::Path, db_file: &str, crdt_enabled: bool) -> Booted {
    let mut config = HolonConfig {
        db_path: Some(dir.join(db_file)),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    config.crdt.enabled = Some(crdt_enabled);
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
    let current = upgraded
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current;
    let dropped = current
        .iter()
        .find(|c| c.condition_key().kind == "database-views-dropped")
        .unwrap_or_else(|| {
            panic!(
                "views dropped at boot were not disclosed: {:?}",
                raised_kinds(&upgraded)
            )
        });
    let detail = dropped.reason.detail(&dropped.subject);
    let rendered = format!("{} {}", detail.headline, detail.body.join(" | "));
    assert!(
        rendered.contains("older_fn/1/v1") && rendered.contains("every table row is kept"),
        "the text Martin reads must say why and that no row is lost: {rendered}"
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

/// A damaged write-ahead log beside an intact database: only the log moves
/// aside, and the boot says so.
#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_write_ahead_log_is_moved_aside_and_disclosed() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let db_path = dir.path().join(DB_FILE);
    drop(holon::storage::turso::TursoBackend::open_database(&db_path).expect("create"));
    let garbage = b"a write-ahead log nobody can read".repeat(50);
    std::fs::write(dir.path().join(format!("{DB_FILE}-wal")), &garbage).expect("damage the log");

    let booted = boot(dir.path(), DB_FILE).await;

    let current = booted
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current;
    let moved = current
        .iter()
        .find(|c| c.condition_key().kind == "database-log-moved-aside")
        .unwrap_or_else(|| {
            panic!(
                "a log moved aside at boot was not disclosed: {:?}",
                raised_kinds(&booted)
            )
        });
    let detail = moved.reason.detail(&moved.subject);
    let [backup] = detail.body.as_slice() else {
        panic!(
            "the detail must carry the moved log's path, got {:?}",
            detail.body
        );
    };
    assert_eq!(
        std::fs::read(backup).expect("read the moved log"),
        garbage,
        "the disclosed path must hold the damaged log"
    );
    let rendered = format!("{} {}", detail.headline, detail.body.join(" | "));
    assert!(
        rendered.contains("The database file itself was kept")
            && !rendered.contains("came back from the vault"),
        "the main file was kept, not rebuilt from the vault: {rendered}"
    );
    assert!(
        !raised_kinds(&booted).contains(&"database-moved-aside".to_string()),
        "the intact database must not be moved aside"
    );
}

/// A damaged shared-WAL coordination file beside an intact database: only it
/// moves aside, and the boot says so.
#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_coordination_file_is_moved_aside_and_disclosed() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let db_path = dir.path().join(DB_FILE);
    drop(holon::storage::turso::TursoBackend::open_database(&db_path).expect("create"));
    let garbage = b"a coordination file nobody can read".repeat(50);
    std::fs::write(dir.path().join(format!("{DB_FILE}-tshm")), &garbage)
        .expect("damage the coordination file");

    let booted = boot(dir.path(), DB_FILE).await;

    let current = booted
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current;
    let moved = current
        .iter()
        .find(|c| c.condition_key().kind == "database-coordination-file-moved-aside")
        .unwrap_or_else(|| {
            panic!(
                "a coordination file moved aside at boot was not disclosed: {:?}",
                raised_kinds(&booted)
            )
        });
    let detail = moved.reason.detail(&moved.subject);
    let [backup] = detail.body.as_slice() else {
        panic!(
            "the detail must carry the moved file's path, got {:?}",
            detail.body
        );
    };
    assert_eq!(
        std::fs::read(backup).expect("read the moved coordination file"),
        garbage,
        "the disclosed path must hold the damaged coordination file"
    );
    let kinds = raised_kinds(&booted);
    assert!(
        !kinds.contains(&"database-moved-aside".to_string())
            && !kinds.contains(&"database-log-moved-aside".to_string()),
        "the intact database and its log must stay: {kinds:?}"
    );
}

/// The test re-execs itself with this variable set to hold the database from a
/// second process: a lock only another PROCESS can hold.
const CHILD_HOLD_DB_PATH: &str = "HOLON_DISCLOSURE_TEST_CHILD_HOLD_DB_PATH";
const HELD_DB_FILE: &str = "held.db";
const HOLD_TEST: &str = "a_boot_that_waited_for_the_lock_discloses_the_wait_to_the_first_window";

fn run_as_holder(path: &str) -> ! {
    let _db = holon_turso::turso::TursoBackend::open_database(path).expect("the holder opens");
    println!("CHILD-HOLDING");
    std::io::Write::flush(&mut std::io::stdout()).expect("flush");
    std::thread::sleep(std::time::Duration::from_secs(60));
    std::process::exit(0);
}

/// A second process holding `path`, already past its open. `test` is the
/// test the child runs, which becomes the holder.
fn spawn_holder(path: &std::path::Path, test: &str) -> std::process::Child {
    // `--exact` matches the test's path below the crate root.
    let (_crate_root, module) = module_path!().split_once("::").expect("a test module");
    let exact = format!("{module}::{test}");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", &exact, "--nocapture", "--test-threads=1"])
        .env(CHILD_HOLD_DB_PATH, path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the holder");
    // The harness prints its own lines first, so read until the holder speaks.
    let mut out = std::io::BufReader::new(child.stdout.take().expect("holder stdout"));
    let mut line = String::new();
    while !line.contains("CHILD-HOLDING") {
        line.clear();
        let read = std::io::BufRead::read_line(&mut out, &mut line).expect("read the holder");
        assert!(read > 0, "the holder never reported that it holds the file");
    }
    child
}

#[tokio::test(flavor = "multi_thread")]
async fn a_boot_that_waited_for_the_lock_discloses_the_wait_to_the_first_window() {
    if let Ok(path) = std::env::var(CHILD_HOLD_DB_PATH) {
        run_as_holder(&path);
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let held = dir.path().join(HELD_DB_FILE);
    let mut holder = spawn_holder(&held, HOLD_TEST);

    // The boot blocks on the lock, so the hold must end from elsewhere.
    let released = tokio::task::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        holder.kill().expect("kill the holder");
        holder.wait().expect("reap the holder");
    });
    // Without the CRDT store the database is the durable store, so the boot
    // waits for it.
    let booted = boot_with_crdt(dir.path(), HELD_DB_FILE, false).await;
    released.await.expect("the holder's release");

    // A window opens only after the boot, so it sees the notice through the
    // snapshot the bus replays to a late subscriber.
    let current = booted
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .subscribe()
        .current;
    let waited = current
        .iter()
        .find(|c| c.condition_key().kind == "database-lock-waited")
        .unwrap_or_else(|| {
            panic!(
                "a boot that waited for the database lock did not disclose the wait: {:?}",
                raised_kinds(&booted)
            )
        });
    let profile = waited.reason.profile();
    assert_eq!(
        profile.all_clear(),
        holon_api::AllClear::UntilRestart,
        "the wait is a one-shot notice about this boot"
    );
    assert_ne!(
        profile.severity(),
        holon_api::ConditionSeverity::Info,
        "a boot the user had to wait through is not an info-level notice"
    );
    let detail = waited.reason.detail(&waited.subject);
    let rendered = format!("{} {}", detail.headline, detail.body.join(" | "));
    for needle in ["waited", "s for the database lock held by another process"] {
        assert!(
            rendered.contains(needle),
            "the text Martin reads must hold {needle:?}: {rendered}"
        );
    }

    let backups: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
        .expect("list the vault")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{HELD_DB_FILE}.unusable-")))
        })
        .collect();
    assert!(
        backups.is_empty(),
        "a wait for a held file must not be mistaken for damage: {backups:?}"
    );
}

const HOLD_LORO_TEST: &str =
    "a_loro_boot_on_a_held_database_runs_on_memory_and_says_when_the_database_is_free";

fn database_held_text(booted: &Booted) -> Option<String> {
    condition(booted, "database-held").map(|(_, text)| text)
}

fn condition(booted: &Booted, kind: &str) -> Option<(holon_api::ConditionSeverity, String)> {
    booted
        .injector
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .find(|c| c.condition_key().kind == kind)
        .map(|c| {
            let detail = c.reason.detail(&c.subject);
            (
                c.reason.profile().severity(),
                format!("{} {}", detail.headline, detail.body.join(" | ")).to_lowercase(),
            )
        })
}

/// In a Loro mode the database is only the projection, so a held file does not
/// stop the boot: it runs on an in-memory projection at once, says so, and says
/// when the file is free again.
#[tokio::test(flavor = "multi_thread")]
async fn a_loro_boot_on_a_held_database_runs_on_memory_and_says_when_the_database_is_free() {
    if let Ok(path) = std::env::var(CHILD_HOLD_DB_PATH) {
        run_as_holder(&path);
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let held = dir.path().join(HELD_DB_FILE);
    let mut holder = spawn_holder(&held, HOLD_LORO_TEST);

    let booted = boot_with_crdt(dir.path(), HELD_DB_FILE, true).await;
    assert!(
        holder.try_wait().expect("poll the holder").is_none(),
        "the boot must not outlast the holder"
    );
    let text = database_held_text(&booted).unwrap_or_else(|| {
        panic!(
            "a boot on a held database did not disclose it: {:?}",
            raised_kinds(&booted)
        )
    });
    assert!(
        text.contains("another process holds the database") && text.contains("in memory"),
        "the text Martin reads must say why and what Holon does instead: {text}"
    );
    let blocks = names(
        &booted,
        "SELECT id AS name FROM block WHERE id = 'block:rebuild-probe-child'",
    )
    .await;
    assert_eq!(
        blocks,
        vec!["block:rebuild-probe-child".to_string()],
        "the in-memory projection must hold the vault's blocks"
    );

    holder.kill().expect("kill the holder");
    holder.wait().expect("reap the holder");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let text = database_held_text(&booted).expect("the condition stays for the session");
        if text.contains("the database is free again — restart holon to use it") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the condition never said the database is free again: {text}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        std::fs::read_dir(dir.path())
            .expect("list the vault")
            .all(|e| !e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{HELD_DB_FILE}.unusable-"))),
        "a held file must not be mistaken for damage"
    );
}

const HOLD_LORO_PROBE_FAILS_TEST: &str =
    "a_held_database_whose_probe_fails_says_the_watch_stopped_and_why";

/// When the session can no longer tell whether the held file is free, it no
/// longer says "held": it raises an Error that the watch stopped, why, and that
/// a restart retries.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_database_whose_probe_fails_says_the_watch_stopped_and_why() {
    if let Ok(path) = std::env::var(CHILD_HOLD_DB_PATH) {
        run_as_holder(&path);
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let held = dir.path().join(HELD_DB_FILE);
    let mut holder = spawn_holder(&held, HOLD_LORO_PROBE_FAILS_TEST);

    let booted = boot_with_crdt(dir.path(), HELD_DB_FILE, true).await;
    database_held_text(&booted).expect("the boot discloses the held database");

    // A directory where the file was: the probe's open fails, and not as Locked.
    std::fs::remove_file(&held).expect("unlink the held file");
    std::fs::create_dir(&held).expect("put a directory in its place");
    let error = holon_turso::db_open::is_held(&held)
        .expect_err("probing a directory must fail")
        .to_string()
        .to_lowercase();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let (severity, text) = loop {
        if let Some(stopped) = condition(&booted, "database-watch-stopped") {
            break stopped;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the watch never stopped ({error}): {:?}",
            raised_kinds(&booted)
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(severity, holon_api::ConditionSeverity::Error, "{text}");
    assert!(
        text.contains("holon stopped watching the database")
            && text.contains("restart holon to retry")
            && text.contains(&error),
        "the text Martin reads must say the watch stopped, what to do, and why ({error}): {text}"
    );
    assert_eq!(
        database_held_text(&booted),
        None,
        "a stopped watch cannot claim the database is still held"
    );
    holder.kill().expect("kill the holder");
    holder.wait().expect("reap the holder");
}
