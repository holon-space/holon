//! Contract: in the real DI wiring, a SQL command that keeps the actor busy
//! past `HOLON_ACTOR_HANG_MS` reaches the container's `ConditionBus` as
//! `DatabaseStuck`, and the disclosure clears when the command finishes.

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use fluxdi::Injector;
use holon::di::DbHandleProvider;
use holon::di::register_core_services_with_backend;
use holon::storage::turso::TursoBackend;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use tokio::sync::RwLock;

/// About one to three seconds in the test profile.
const SLOW_QUERY: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE \
                          x < 60000) SELECT count(*) AS n FROM c";

fn stuck_command(bus: &ConditionBus) -> Option<(String, String)> {
    bus.current().into_iter().find_map(|c| match c.reason {
        ConditionKind::DatabaseStuck {
            command, report, ..
        } => Some((command, report)),
        _ => None,
    })
}

fn wait_until(within: Duration, what: &str, bus: &ConditionBus, done: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} within {within:?}; conditions in effect: {:?}",
            bus.current()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_command_past_the_hang_bound_reaches_the_containers_condition_bus() {
    // SAFETY: no other thread exists yet that reads the environment.
    unsafe { std::env::set_var("HOLON_ACTOR_HANG_MS", "250") };
    let rt = ManuallyDrop::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime"),
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let injector = Injector::root();
    let db = rt.block_on(async {
        let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
        register_core_services_with_backend(
            &injector,
            dir.path().join("unused.db"),
            Arc::new(RwLock::new(backend)),
            handle,
        )
        .expect("core services");
        injector.resolve::<dyn DbHandleProvider>().handle()
    });
    let bus: Arc<ConditionBus> = (*injector.resolve::<Arc<ConditionBus>>()).clone();
    wait_until(
        Duration::from_secs(2),
        "no DatabaseStuck before the slow query",
        &bus,
        || stuck_command(&bus).is_none(),
    );

    let query = rt.spawn(async move {
        db.query(SLOW_QUERY, Default::default())
            .await
            .expect("slow count query");
    });
    wait_until(
        Duration::from_secs(10),
        "a DatabaseStuck naming the slow query",
        &bus,
        || {
            stuck_command(&bus).is_some_and(|(command, report)| {
                command == "Query" && report.contains("WITH RECURSIVE c(x)")
            })
        },
    );
    rt.block_on(query).expect("slow query task");
    wait_until(
        Duration::from_secs(2),
        "the DatabaseStuck cleared after the slow query finished",
        &bus,
        || stuck_command(&bus).is_none(),
    );
    ManuallyDrop::into_inner(rt).shutdown_background();
}
