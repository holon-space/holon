//! Where the ~0.3–0.7 ms of ONE single-row read goes.
//!
//! The D156.b spike measured a per-entity marking at 13–36 ms for ~50 point
//! reads. That is only a verdict on the marking if the per-read cost is
//! understood, so this harness takes one read apart by SUBTRACTION, using
//! public APIs only and changing no production code.
//!
//! The ladder, each step adding exactly one term over the step below:
//!
//!   1. `SELECT 1` through `DbHandle::query`   — actor round trip + bind +
//!      prepare(trivial) + execute(trivial) + decode(1 narrow row). Every read
//!      pays this whatever it asks for.
//!   2. narrow point read                      — + index seek + a 3-column row
//!   3. HYDRATED point read                    — + a wider prepare and the four
//!      `json_group_array` edge subqueries. This is what
//!      `CacheBlockReader::get_block_authoritative` actually issues
//!      (`crates/holon-app/src/turso_seams.rs:70`, `:312`).
//!   4. the same through `TestEnvironment::query_sql` — + the query-language
//!      layer above `DbHandle`.
//!   5. step 2 again with concurrent readers   — the queue wait the sequential
//!      actor imposes under load.
//!
//! The ladder runs strictly in order and never interleaved, so step 2 warms
//! the exact row step 3 then reads. That biases the added terms DOWNWARD —
//! the direction that understates the read, so a conclusion drawn from these
//! terms is the conservative one.
//!
//! What this CANNOT separate: prepare from execute INSIDE one call. Both
//! happen in `TursoBackend::query_rows`
//! (`crates/holon-turso/src/turso.rs:2925`), behind the actor, with no seam a
//! test can time between them. Step 3 minus step 2 bounds the two together
//! for the hydrated statement; it does not split them.
//!
//! @pbt kind harness
//! @pbt covers single-read-attribution — the per-read terms behind the hop cost

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::storage::DbHandle;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

/// Copied from `crates/holon-app/src/turso_seams.rs:70` so this harness
/// prices the statement production issues, not a simplified stand-in. A
/// divergence here understates the read, which is the one direction that
/// would make the conclusion wrong.
const HYDRATED_BLOCK_COLUMNS: &str = "b.id, b.parent_id, b.sort_key, b.content, b.content_type, \
     b.source_language, b.source_name, b.properties, b.property_kinds, b.marks, b.collapsed, \
     b.widget_only, \
     b.completed, b.block_type, b.created_at, b.updated_at, \
     COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS \
     tags, \
     COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = b.id), \
     '[]') AS requires, \
     COALESCE((SELECT json_group_array(lesson_id) FROM advice_suppressed WHERE anchor_id = \
     b.id), '[]') AS advice_suppressed, \
     COALESCE((SELECT json_group_array(target_id) FROM block_contributes_to WHERE block_id = \
     b.id), '[]') AS contributes_to";

const LEVEL_LADDER: &[usize] = &[1, 2, 3, 2, 2, 3, 4, 3, 2, 1, 2, 3, 3, 2, 3];

fn docs() -> usize {
    env_usize("HOLON_ENABLEDNESS_DOCS", 12)
}

fn headlines_per_doc() -> usize {
    env_usize("HOLON_ENABLEDNESS_HEADLINES", 22)
}

fn runs() -> usize {
    env_usize("HOLON_ENABLEDNESS_RUNS", 200)
}

/// Concurrent readers for the contention step.
fn load() -> usize {
    env_usize("HOLON_READ_LOAD", 8)
}

fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse()
            .unwrap_or_else(|e| panic!("{key}='{raw}' is not a count: {e}")),
        Err(std::env::VarError::NotPresent) => default,
        Err(e) => panic!("{key} unreadable: {e}"),
    }
}

fn block_id(doc: usize, headline: usize) -> String {
    format!("attr-d{doc:04}-h{headline:03}")
}

fn synthesized_document(doc: usize, n: usize) -> String {
    let mut out = String::with_capacity(n * 160);
    out.push_str(&format!(
        "#+TITLE: Attribution Doc {doc}\n#+ID: attr-doc-{doc:04}\n"
    ));
    for i in 0..n {
        let level = LEVEL_LADDER[(doc + i) % LEVEL_LADDER.len()];
        out.push_str(&"*".repeat(level));
        out.push_str(&format!(
            " Node {doc}-{i}\n:PROPERTIES:\n:ID: {}\n:END:\n",
            block_id(doc, i)
        ));
    }
    out
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

fn pct(sorted: &[Duration], p: f64) -> f64 {
    assert!(!sorted.is_empty(), "no samples to take a percentile of");
    let ms: Vec<f64> = sorted.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    if ms.len() == 1 {
        return ms[0];
    }
    let rank = p * (ms.len() - 1) as f64;
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    ms[lo] + (ms[hi] - ms[lo]) * (rank - lo as f64)
}

fn report(step: &str, mut samples: Vec<Duration>) -> f64 {
    samples.sort();
    let (p50, p95, max) = (pct(&samples, 0.50), pct(&samples, 0.95), pct(&samples, 1.0));
    eprintln!(
        "[read-attr] {step:<40} n={:<4} p50={p50:8.3} p95={p95:8.3} max={max:8.3}  ms",
        samples.len()
    );
    p50
}

fn id_param(id: &str) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(id.to_string()));
    params
}

/// Time `n` sequential `DbHandle::query` calls, asserting each returned the
/// row count the step is supposed to produce.
async fn time_handle(
    db: &DbHandle,
    n: usize,
    sql: &str,
    params: HashMap<String, Value>,
    expect_rows: usize,
) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let rows = db
            .query(sql, params.clone())
            .await
            .unwrap_or_else(|e| panic!("attribution read failed: {sql}: {e}"));
        samples.push(t.elapsed());
        assert_eq!(
            rows.len(),
            expect_rows,
            "the timed statement must return the rows the step prices: {sql}"
        );
    }
    samples
}

#[test]
fn where_one_single_row_read_goes() {
    let (d, h, n) = (docs(), headlines_per_doc(), runs());
    holon_integration_tests::test_tracing::SpanCollector::global();

    let rt = runtime();
    rt.clone().block_on(async move {
        let mut builder = TestEnvironmentBuilder::new();
        for doc in 0..d {
            builder = builder.with_org_file(
                format!("Attribution/Doc{doc:04}.org"),
                synthesized_document(doc, h),
            );
        }
        let world = builder.build(rt.clone()).await.expect("boot the vault");
        let blocks = world
            .query_sql("SELECT id FROM block_raw")
            .await
            .expect("count block_raw")
            .len();
        eprintln!("[read-attr] boot: {d} files / {blocks} blocks");
        assert!(blocks > d, "the corpus never ingested: {blocks} blocks");
        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;

        let injector = world
            .injector()
            .expect("the Turso-backed environment exposes its injector")
            .clone();
        let db = injector
            .resolve::<dyn holon::di::DbHandleProvider>()
            .handle();

        let subject = format!("block:{}", block_id(d / 2, h / 2));

        // Warm-up, unmeasured: boot and quiescence leave the first reads
        // paying page-cache and actor-startup costs that belong to no rung.
        time_handle(&db, n.min(20), "SELECT 1", HashMap::new(), 1).await;

        // 1 — the floor every read pays.
        let p50_floor = report(
            "1 SELECT 1 (actor + prepare + decode)",
            time_handle(&db, n, "SELECT 1", HashMap::new(), 1).await,
        );

        // 2 — + index seek, narrow row.
        let narrow = "SELECT id, parent_id, content_type FROM block_raw WHERE id = $id";
        let p50_narrow = report(
            "2 narrow point read",
            time_handle(&db, n, narrow, id_param(&subject), 1).await,
        );

        // 3 — + the hydration production actually issues.
        let hydrated = format!("SELECT {HYDRATED_BLOCK_COLUMNS} FROM block_raw b WHERE b.id = $id");
        let p50_hydrated = report(
            "3 HYDRATED point read (production)",
            time_handle(&db, n, &hydrated, id_param(&subject), 1).await,
        );

        // 4 — + the query-language layer above DbHandle. Same statement TEXT
        // and same bound parameter as step 3, so the subtraction isolates the
        // layer rather than a different statement and a different binding
        // path.
        let mut lang = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let rows = world
                .test_ctx()
                .query(
                    hydrated.clone(),
                    QueryLanguage::HolonSql,
                    id_param(&subject),
                )
                .await
                .expect("language-layer read");
            lang.push(t.elapsed());
            assert_eq!(rows.len(), 1, "the language-layer read must find the row");
        }
        let p50_lang = report("4 same, through the query language", lang);

        // 5 — step 2 again while the actor is busy. The reads are issued from
        // other tasks on the same runtime, so what grows is the queue this
        // read waits in, not the work it asks for.
        let loaders = load();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = Vec::with_capacity(loaders);
        for _ in 0..loaders {
            let db = db.clone();
            let stop = stop.clone();
            let id = subject.clone();
            handles.push(tokio::spawn(async move {
                let mut issued = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = db
                        .query(
                            "SELECT id, parent_id FROM block_raw WHERE id = $id",
                            id_param(&id),
                        )
                        .await;
                    issued += 1;
                }
                issued
            }));
        }
        let p50_contended = report(
            "5 narrow point read, actor under load",
            time_handle(&db, n, narrow, id_param(&subject), 1).await,
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut competing = 0u64;
        for handle in handles {
            competing += handle.await.expect("a loader task");
        }
        assert!(
            competing > 0,
            "no competing read landed, so step 5 measured an idle actor"
        );
        eprintln!(
            "[read-attr] step 5 ran against {loaders} loader task(s), {competing} competing \
             read(s) issued"
        );

        eprintln!("[read-attr] --- attribution of ONE read, by subtraction ---");
        eprintln!(
            "[read-attr] floor (actor round trip + prepare + decode) = {p50_floor:.3} ms  \
             ({:.0}% of the hydrated read)",
            100.0 * p50_floor / p50_hydrated
        );
        eprintln!(
            "[read-attr] index seek + narrow row                     = {:.3} ms",
            p50_narrow - p50_floor
        );
        eprintln!(
            "[read-attr] hydration (wider prepare + 4 subqueries)    = {:.3} ms",
            p50_hydrated - p50_narrow
        );
        eprintln!(
            "[read-attr] query-language layer above DbHandle         = {:.3} ms",
            p50_lang - p50_hydrated
        );
        eprintln!(
            "[read-attr] queue wait added by {loaders} concurrent readers    = {:.3} ms  \
             ({:.1}x the idle read)",
            p50_contended - p50_narrow,
            p50_contended / p50_narrow
        );
    });
}
