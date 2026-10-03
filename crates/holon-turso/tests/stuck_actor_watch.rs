//! Contract: a command that keeps the SQL actor busy past the hang bound
//! (`HOLON_ACTOR_HANG_MS`) is disclosed as `DatabaseStuck`, naming its kind
//! and its SQL with the literals blanked, and the disclosure clears when the
//! command finishes.
//!
//! The bound is read from the environment when a backend starts; nextest runs
//! every test in its own process.
//!
//! A spinning command never yields, so a runtime that owns it can never finish
//! a drop: each runtime is `ManuallyDrop` and ends with `shutdown_background`,
//! which a failed assertion skips instead of hanging in the drop.

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionChange;
use holon_api::ConditionKind;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tracing::Instrument;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// `blocks_with_paths` reduced to the columns the recursion needs.
const PATHS_VIEW: &str = "CREATE MATERIALIZED VIEW blocks_with_paths AS
WITH RECURSIVE paths AS (
    SELECT id, parent_id, '/' || id AS path, id AS root_id
    FROM block WHERE parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT b.id, b.parent_id, p.path || '/' || b.id AS path, p.root_id
    FROM block b INNER JOIN paths p ON b.parent_id = p.id
)
SELECT * FROM paths";

/// Moves `a` under its own child `b`: the stored parent cycle that makes the
/// IVM commit of `blocks_with_paths` spin forever.
const CYCLE_WRITE: &str = "UPDATE block SET parent_id = 'b' WHERE id = 'a'";

fn slow_count_sql(n: u64) -> String {
    format!(
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < {n}) \
         SELECT count(*) AS n FROM c"
    )
}

fn runtime() -> ManuallyDrop<tokio::runtime::Runtime> {
    ManuallyDrop::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime"),
    )
}

fn set_hang_bound(bound: Duration) {
    // SAFETY: no runtime is alive. The only other thread is the SQL actor
    // watchdog, which does not read the environment.
    unsafe { std::env::set_var("HOLON_ACTOR_HANG_MS", bound.as_millis().max(1).to_string()) };
}

fn open(rt: &tokio::runtime::Runtime, bus: &Arc<ConditionBus>) -> DbHandle {
    rt.block_on(async {
        let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
        // The actor lives as long as a handle does; the backend only owns the
        // database file, which an in-memory database does not have.
        std::mem::forget(backend);
        handle.disclose_stuck_commands_on(bus.clone());
        handle
    })
}

struct Stuck {
    command: String,
    running_secs: u64,
    report: String,
}

fn stuck_now(bus: &ConditionBus) -> Option<Stuck> {
    bus.current().into_iter().find_map(|c| match c.reason {
        ConditionKind::DatabaseStuck {
            command,
            running_secs,
            report,
        } => Some(Stuck {
            command,
            running_secs,
            report,
        }),
        _ => None,
    })
}

fn wait_for_stuck(bus: &ConditionBus, within: Duration) -> Stuck {
    let deadline = Instant::now() + within;
    loop {
        if let Some(stuck) = stuck_now(bus) {
            return stuck;
        }
        assert!(
            Instant::now() < deadline,
            "no DatabaseStuck condition within {within:?} while the actor is busy past the bound; \
             conditions in effect: {:?}",
            bus.current()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The watchdog clears a finished command's disclosure on its next tick.
const CLEAR_WITHIN: Duration = Duration::from_secs(2);

fn wait_for_clear(bus: &ConditionBus, within: Duration) {
    let deadline = Instant::now() + within;
    while stuck_now(bus).is_some() {
        assert!(
            Instant::now() < deadline,
            "the condition is still in effect {within:?} after the slow command finished: {:?}",
            bus.current()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_slow_query(rt: &tokio::runtime::Runtime, handle: &DbHandle, rows: u64) -> Duration {
    rt.block_on(async {
        let t0 = Instant::now();
        handle
            .query(&slow_count_sql(rows), Default::default())
            .await
            .expect("slow count query");
        t0.elapsed()
    })
}

/// A slow query on this machine and build. A bound below it is a quarter of
/// its fastest run, for a query with four times its rows; a bound above it is
/// four times its slowest run. The margins absorb load that changes between
/// the calibration and the test.
struct Calibration {
    rows: u64,
    fastest: Duration,
    slowest: Duration,
}

/// The rows whose query runs for at least 200 ms in each of three runs.
fn calibrate_slow_query() -> Calibration {
    const SLOW: Duration = Duration::from_millis(200);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("calibration runtime");
    let (_backend, handle) = rt
        .block_on(TursoBackend::new_in_memory())
        .expect("in-memory db");
    let mut rows = 10_000;
    loop {
        let first = run_slow_query(&rt, &handle, rows);
        if first >= SLOW {
            let runs = [
                first,
                run_slow_query(&rt, &handle, rows),
                run_slow_query(&rt, &handle, rows),
            ];
            let fastest = *runs.iter().min().expect("three runs");
            if fastest >= SLOW {
                eprintln!("calibration: {rows} rows ran for {runs:?}");
                return Calibration {
                    rows,
                    fastest,
                    slowest: *runs.iter().max().expect("three runs"),
                };
            }
        }
        rows *= 2;
    }
}

/// The `DatabaseStuck` changes so far, a repeated raise once.
fn stuck_changes(changes: &mut tokio::sync::broadcast::Receiver<ConditionChange>) -> Vec<String> {
    let mut seen: Vec<String> = std::iter::from_fn(|| changes.try_recv().ok())
        .filter_map(|change| match change {
            ConditionChange::Raised(Condition {
                reason: ConditionKind::DatabaseStuck { command, .. },
                ..
            }) => Some(format!("raised {command}")),
            ConditionChange::Cleared(key) if key.kind == ConditionKind::DATABASE_STUCK => {
                Some("cleared".to_string())
            }
            _ => None,
        })
        .collect();
    seen.dedup();
    seen
}

#[test]
fn a_spinning_commit_is_disclosed_with_its_command_kind_and_redacted_sql() {
    set_hang_bound(Duration::from_millis(500));
    let rt = runtime();
    let bus = Arc::new(ConditionBus::new());
    let handle = open(&rt, &bus);
    rt.block_on(async {
        handle
            .execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, parent_id TEXT)")
            .await
            .expect("create block");
        handle.execute_ddl(PATHS_VIEW).await.expect("create view");
        handle
            .execute_values(
                "INSERT INTO block VALUES ('a', 'sentinel:no_parent'), ('b', 'a')",
                vec![],
            )
            .await
            .expect("seed a tree");
    });

    let spinning = handle.clone();
    let started = Instant::now();
    rt.spawn(async move {
        let outcome = spinning.execute_values(CYCLE_WRITE, vec![]).await;
        panic!("the cycle write returned ({outcome:?}); this test needs it to spin");
    });

    let stuck = wait_for_stuck(&bus, Duration::from_secs(10));
    let waited = started.elapsed();
    assert_eq!(stuck.command, "Execute");
    assert!(
        stuck.report.contains("UPDATE block SET parent_id ="),
        "the report names the stuck statement: {}",
        stuck.report
    );
    assert!(
        !stuck.report.contains("'b'") && !stuck.report.contains("'a'"),
        "the report carries no SQL literal: {}",
        stuck.report
    );
    assert!(
        waited >= Duration::from_millis(500),
        "raised after {waited:?}, before the 500 ms bound"
    );
    eprintln!(
        "raised after {waited:?}, running_secs {}; report:\n{}",
        stuck.running_secs, stuck.report
    );
    ManuallyDrop::into_inner(rt).shutdown_background();
}

#[test]
fn a_slow_healthy_command_past_the_bound_is_disclosed_then_cleared() {
    let slow = calibrate_slow_query();
    let bound = slow.fastest / 4;
    set_hang_bound(bound);
    let rt = runtime();
    let bus = Arc::new(ConditionBus::new());
    let handle = open(&rt, &bus);
    let mut changes = bus.subscribe().changes;

    let took = run_slow_query(&rt, &handle, slow.rows * 4);
    wait_for_clear(&bus, CLEAR_WITHIN);

    assert_eq!(
        stuck_changes(&mut changes),
        ["raised Query", "cleared"],
        "a query that ran {took:?} against a bound of {bound:?}"
    );
    ManuallyDrop::into_inner(rt).shutdown_background();
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log poisoned").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The subscriber is shaped like the app's: global, with the filter on the
/// fmt layer.
#[test]
fn the_stuck_report_is_logged_in_the_span_of_the_code_that_sent_the_command() {
    let log = Log::default();
    let writer = log.clone();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .with_ansi(false)
                .with_filter(EnvFilter::new("info")),
        )
        .init();
    let slow = calibrate_slow_query();
    set_hang_bound(slow.fastest / 4);
    let rt = runtime();
    let bus = Arc::new(ConditionBus::new());
    let handle = open(&rt, &bus);

    rt.block_on(
        async move {
            handle
                .query(&slow_count_sql(slow.rows * 4), Default::default())
                .await
                .expect("slow count query");
        }
        .instrument(tracing::info_span!("sender_of_the_slow_query")),
    );
    wait_for_clear(&bus, CLEAR_WITHIN);

    let text = String::from_utf8(log.0.lock().expect("log poisoned").clone()).expect("utf-8 log");
    let line = text
        .lines()
        .find(|l| l.contains("ERROR") && l.contains("SQL actor stuck"))
        .unwrap_or_else(|| panic!("no ERROR line for the stuck command in the log:\n{text}"));
    eprintln!("{line}");
    assert!(
        line.contains("sender_of_the_slow_query"),
        "the ERROR line does not carry the sender's span: {line}"
    );
    ManuallyDrop::into_inner(rt).shutdown_background();
}

#[test]
fn a_slow_healthy_command_under_the_bound_is_not_disclosed() {
    let slow = calibrate_slow_query();
    let bound = slow.slowest * 4;
    set_hang_bound(bound);
    let rt = runtime();
    let bus = Arc::new(ConditionBus::new());
    let handle = open(&rt, &bus);
    let mut changes = bus.subscribe().changes;

    let took = run_slow_query(&rt, &handle, slow.rows);

    assert!(
        changes.try_recv().is_err(),
        "a query that ran {took:?} against a bound of {bound:?} raised a condition: {:?}",
        bus.current()
    );
    ManuallyDrop::into_inner(rt).shutdown_background();
}
