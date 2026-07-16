//! `#[ignore]`d micro-benchmark for the C2b history write-path overhead
//! (plan INC 1, amendment A4). The ONLY work history adds to the dispatch
//! critical path is building the per-delta events (pure, negligible) plus one
//! `record_batch` call, so timing `record_batch` against a file-backed Turso
//! db measures the added per-interaction latency directly.
//!
//! Run on a quiet machine (never concurrently with another benchmarking
//! lane):
//! `cargo test -p holon --test history_overhead_bench -- --ignored
//!  --nocapture`
//!
//! Acceptance (plan INC 1): p95 added latency < 5ms per interaction.

use std::time::Instant;

use holon::api::TursoHistoryStore;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::HistoryEvent;
use holon_api::HistoryStore;
use holon_turso::schema_modules::HistorySchemaModule;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn ev(i: i64, field: &str) -> HistoryEvent {
    HistoryEvent {
        entity_name: "block".to_string(),
        block_id: format!("block:{i}"),
        op_name: "set_field".to_string(),
        origin: "user".to_string(),
        transition_id: None,
        session_id: None,
        tool_call_id: None,
        effect_id: None,
        field: Some(field.to_string()),
        old_value: Some("todo".to_string()),
        new_value: Some("doing".to_string()),
        at_millis: 1_784_203_200_000 + i,
        op_group: None,
    }
}

fn percentile(sorted_micros: &[u128], p: f64) -> u128 {
    let idx = ((sorted_micros.len() as f64 - 1.0) * p).round() as usize;
    sorted_micros[idx]
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "micro-benchmark: run explicitly on a quiet machine"]
async fn history_record_batch_overhead() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("history_bench.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    HistorySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("history schema");
    let store = TursoHistoryStore::new(handle);

    const WARMUP: i64 = 20;
    const ITERS: i64 = 300;

    for i in 0..WARMUP {
        store.record_batch(vec![ev(i, "status")]).await.unwrap();
    }

    for (label, batch_size) in [("batch=1 (set_field)", 1), ("batch=3 (update)", 3)] {
        let mut micros: Vec<u128> = Vec::with_capacity(ITERS as usize);
        for i in 0..ITERS {
            let events: Vec<HistoryEvent> = (0..batch_size)
                .map(|j| ev(i, &format!("field_{j}")))
                .collect();
            let t0 = Instant::now();
            store.record_batch(events).await.unwrap();
            micros.push(t0.elapsed().as_micros());
        }
        micros.sort_unstable();
        let p50 = percentile(&micros, 0.50);
        let p95 = percentile(&micros, 0.95);
        let max = micros.last().copied().unwrap();
        eprintln!(
            "history overhead {label}: p50={p50}us p95={p95}us max={max}us over {ITERS} iters \
             (file-backed Turso)"
        );
        assert!(
            p95 < 5_000,
            "{label}: p95 {p95}us breaches the 5ms INC1 acceptance bound"
        );
    }
}
