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

fn slow_query(rows: u64) -> String {
    format!(
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < {rows}) \
         SELECT count(*) AS n FROM c"
    )
}

/// The rows whose query runs for at least 200 ms in each of three runs, and
/// the fastest of those runs. A bound of a quarter of it, against a query with
/// four times the rows, absorbs load that changes between the calibration and
/// the test.
fn calibrate_slow_query() -> (u64, Duration) {
    const SLOW: Duration = Duration::from_millis(200);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("calibration runtime");
    let (_backend, handle) = rt
        .block_on(TursoBackend::new_in_memory())
        .expect("in-memory db");
    let run = |rows: u64| {
        rt.block_on(async {
            let t0 = Instant::now();
            handle
                .query(&slow_query(rows), Default::default())
                .await
                .expect("calibration query");
            t0.elapsed()
        })
    };
    let mut rows = 10_000;
    loop {
        let fastest = (0..3).map(|_| run(rows)).min().expect("three runs");
        if fastest >= SLOW {
            return (rows, fastest);
        }
        rows *= 2;
    }
}

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
    let (rows, fastest) = calibrate_slow_query();
    let bound = fastest / 4;
    // SAFETY: the calibration runtime is dropped, so no other thread reads the
    // environment.
    unsafe { std::env::set_var("HOLON_ACTOR_HANG_MS", bound.as_millis().to_string()) };
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
        db.query(&slow_query(rows * 4), Default::default())
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
