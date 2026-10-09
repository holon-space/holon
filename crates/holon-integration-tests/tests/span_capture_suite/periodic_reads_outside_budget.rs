//! A periodic background task's SQL is not charged to the transition whose
//! measurement window it happens to land in.
//!
//! The clock scheduler ticks every 30 s in production and in the keystone; a
//! tick that lands inside a tolerance-0 budget window used to add one
//! `SELECT DISTINCT grain FROM clock_reader` read to that transition. Here the
//! real scheduler runs at a short interval so ticks land in the window every
//! time, next to one read the window itself issues.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::storage::turso::TursoBackend;
use holon_api::clock::TestClock;
use holon_api::lifecycle::SessionShutdown;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;

const TICK_READ: &str = "SELECT DISTINCT grain FROM clock_reader";
const TICKS_IN_WINDOW: usize = 3;

#[test]
fn clock_ticks_inside_a_window_are_not_counted_as_its_reads() {
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = builder.build().expect("tokio runtime");
    runtime.block_on(async move {
        let (_backend, db) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory store");
        CoreSchemaModule
            .ensure_schema(&db)
            .await
            .expect("core schema");
        let shutdown = SessionShutdown::new();
        let _scheduler = holon::sync::clock_scheduler::spawn_clock_scheduler(
            db.clone(),
            Arc::new(TestClock::new(1_780_000_000_000)),
            Duration::from_millis(20),
            &shutdown,
        )
        .await
        .expect("clock scheduler");

        collector.reset();
        db.query("SELECT 1 AS one", HashMap::new())
            .await
            .expect("the window's own read");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let ticks = collector
                .spans_named("query")
                .iter()
                .filter(|s| {
                    s.attributes
                        .iter()
                        .any(|kv| kv.key.as_str() == "sql" && kv.value.as_str() == TICK_READ)
                })
                .count();
            if ticks >= TICKS_IN_WINDOW {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "only {ticks} clock ticks landed in the window within 10 s"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let metrics = collector.snapshot();
        assert_eq!(
            metrics.sql_read_count,
            1,
            "the window issued exactly one read of its own; the clock ticks inside it must \
             not count (reads by statement: {:?})",
            collector.sql_breakdown().reads,
        );
        shutdown
            .shutdown(Duration::from_secs(5))
            .await
            .expect("scheduler stops");
    });
}
