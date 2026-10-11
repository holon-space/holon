//! A read the HARNESS issues to observe the system is not charged to the
//! transition whose measurement window it lands in — and a PRODUCTION read
//! that repeats still is.
//!
//! The keystone's `await_toggle_landed` re-asks one `block_raw` question every
//! 10 ms until the click shows in the projection. A slow-landing click pushed
//! that poll past `MAX_READ_REPEAT_PER_BINDING`, so the redundancy ratchet —
//! whose subject is production re-execution growth — reddened on the harness's
//! own waiting (bugfunnel
//! `2026-10-11-toggle-state-repeat-ratchet-counts-the-harness-landing-poll`).

use std::collections::HashMap;

use holon::storage::turso::TursoBackend;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use holon_integration_tests::test_tracing::harness_probe;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;

/// Above `MAX_READ_REPEAT_PER_BINDING` (64), so an unexcluded poll of this
/// length is exactly the false red the entry reports.
const POLLS: usize = 75;
const POLL_READ: &str = "SELECT json_extract(properties, '$.task_state') AS task_state \
                         FROM block_raw WHERE id = 'block:probe'";

#[test]
fn a_harness_poll_is_not_charged_to_the_window_while_a_production_repeat_is() {
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

        collector.reset();
        for _ in 0..POLLS {
            harness_probe("test::landing_poll", async {
                db.query(POLL_READ, HashMap::new())
                    .await
                    .expect("the harness landing poll")
            })
            .await;
        }
        db.query("SELECT 1 AS one", HashMap::new())
            .await
            .expect("the window's own read");

        let metrics = collector.snapshot();
        assert_eq!(
            metrics.sql_read_count,
            1,
            "the window issued exactly one read of its own; the {POLLS} harness polls inside it \
             must not count (reads by statement: {:?})",
            collector.sql_breakdown().reads,
        );
        // Two spans per poll: the probe itself and the `query` under it.
        assert_eq!(
            metrics.harness_probe_span_count,
            2 * POLLS,
            "every excluded poll must be disclosed as harness-probe spans"
        );
        assert_eq!(
            metrics.worst_read_repeat(),
            None,
            "no production read repeated, so the redundancy ratchet has nothing to report"
        );

        // The same statement WITHOUT the probe is production work and stays
        // ratcheted: the exclusion is structural, not a numeric allowance.
        collector.reset();
        for _ in 0..POLLS {
            db.query(POLL_READ, HashMap::new())
                .await
                .expect("a production read that repeats");
        }
        let metrics = collector.snapshot();
        assert_eq!(metrics.sql_read_count, POLLS);
        assert_eq!(
            metrics.worst_read_repeat().map(|(_, repeats)| repeats),
            Some(POLLS),
            "a production read re-executing {POLLS}x on one binding must still be caught"
        );
    });
}
