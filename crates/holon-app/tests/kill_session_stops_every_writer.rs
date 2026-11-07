//! A killed session ends as a killed process does: no task of it runs after
//! `kill_session` returns, so a write-back it still owed never lands, and the
//! next writer opens the same vault and database at once.
//!
//! @pbt kind harness
//! @pbt covers kill-session-stops-every-writer — after the kill no old task
//! writes the vault, and the next boot on the same vault and DB waits for no
//! lock

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const PAGE_ORG: &str = "* Killed page\n:PROPERTIES:\n:ID: killed-page\n:END:\n";

struct Booted {
    engine: Arc<holon::api::BackendEngine>,
    injector: fluxdi::Injector,
    _session: Arc<holon_frontend::FrontendSession>,
}

async fn boot(vault: &Path, state: &Path) -> Booted {
    let holon_config = HolonConfig {
        db_path: Some(state.join("holon.db")),
        vault: VaultConfig {
            root: Some(vault.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(
            holon_api::UiInfo::permissive(),
            Arc::new(holon_api::SystemClock),
        ),
        state.to_path_buf(),
        HashSet::new(),
        Arc::new(holon_conditions::ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring boots a session");
    Booted {
        engine,
        injector,
        _session: session,
    }
}

fn raised_kinds(injector: &fluxdi::Injector) -> Vec<String> {
    injector
        .resolve::<Arc<holon_conditions::ConditionBus>>()
        .subscribe()
        .current
        .iter()
        .map(|c| c.condition_key().kind.to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_killed_session_writes_nothing_after_the_kill_and_frees_its_locks() {
    let vault = tempfile::tempdir().expect("create the vault dir");
    let dir = vault.path().join("killed");
    std::fs::create_dir(&dir).expect("create the page's dir");
    let page = dir.join("killed.org");
    std::fs::write(&page, PAGE_ORG).expect("write the page");
    let state = tempfile::tempdir().expect("state dir");

    let a = boot(vault.path(), state.path()).await;
    // The atomic write cannot create its temp file beside the page, so the
    // write-back of the edit stays owed.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
        .expect("make the page's dir read-only");
    let mut params: StorageEntity = std::collections::HashMap::new();
    params.insert("id".into(), Value::String("block:owed-edit".into()));
    params.insert(
        "parent_id".into(),
        Value::String("block:killed-page".into()),
    );
    params.insert("content".into(), Value::String("owed-edit".into()));
    a.engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::User)
        .await
        .expect("create the edit whose write-back is owed");

    let Booted {
        engine,
        injector,
        _session,
    } = a;
    let killed = holon_app::kill_session(&injector).await;
    drop((engine, _session));
    let stragglers = injector
        .resolve::<holon_api::lifecycle::SessionShutdown>()
        .registered();
    let at_kill = std::fs::read_to_string(&page).expect("read the page");
    assert!(
        !at_kill.contains("owed-edit"),
        "the write-back was not refused, so this test proves nothing:\n{at_kill}"
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
        .expect("make the page's dir writable again");
    killed.expect("the kill stops every task of the session");
    assert!(
        stragglers.is_empty(),
        "tasks of the killed session were never joined: {stragglers:?}"
    );

    // A surviving write-back retries on its own timer, so it gets the window
    // a dead process would not.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let on_disk = std::fs::read_to_string(&page).expect("read the page");
    assert_eq!(
        on_disk, at_kill,
        "a task of the killed session wrote the page after the kill"
    );

    let b = boot(vault.path(), state.path()).await;
    let kinds = raised_kinds(&b.injector);
    assert!(
        !kinds.iter().any(|k| k == "database-lock-waited"),
        "the next boot waited for a lock the killed session kept: {kinds:?}"
    );
    holon_app::shutdown_session(&b.injector)
        .await
        .expect("the next writer brings the file up to its store and shuts down");
    let on_disk = std::fs::read_to_string(&page).expect("read the page");
    assert!(
        on_disk.contains(":ID: owed-edit"),
        "the store lost the edit the killed session acknowledged:\n{on_disk}"
    );
}
