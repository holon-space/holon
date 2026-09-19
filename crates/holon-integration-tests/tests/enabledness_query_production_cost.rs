//! Wall-clock cost of one subject's enabledness query, with Loro as the write
//! authority and Turso as the projection — the composition the native
//! frontends boot.
//!
//! Three steps are timed separately: deriving the net, reading the subject's
//! marking, and evaluating the net against it. The latency rung in
//! `docs/Testing/latency-ceilings.txt` is scored against these figures.
//!
//! The marking is read from the SQL projection. An authorization-relevant
//! predicate must instead read the write authority
//! (`docs/adr/0032-petri-net-execution-semantics.md` §3), which costs more
//! under Loro; this file does not measure that.
//!
//! Wall clock is printed, never asserted. `vault_scale_interaction_latency.rs`
//! records the measured 4.7x run-to-run spread on this hardware that makes any
//! bound here meaningless. The assertions cover scale and catalog
//! non-emptiness, so a green run cannot be vacuous.
//!
//! `catalog_analyzability_census.rs` boots the same composition, so the
//! transition count behind both files is one number.
//!
//! Scale is env-gated:
//!
//!   HOLON_ENABLEDNESS_DOCS=140 cargo nextest run -p holon-integration-tests \
//!     enabledness_query_production_cost --no-capture
//!
//! @pbt kind harness
//! @pbt covers enabledness-query-production-cost — the per-focus enabledness
//! query's cost with Loro as the write authority

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::ArcRelation;
use holon_api::EntityUri;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_net::enabledness::Offer;
use holon_net::enabledness::evaluate;
use holon_net::marking::Marking;

/// Headline ladder, copied from `vault_scale_interaction_latency.rs` so the
/// two harnesses stress the same vault shape.
const LEVEL_LADDER: &[usize] = &[1, 2, 3, 2, 2, 3, 4, 3, 2, 1, 2, 3, 3, 2, 3];

/// Documents in the synthesized vault. 140 x 22 headlines ≈ 3080 blocks. The
/// default is a fast sweep.
fn docs() -> usize {
    env_usize("HOLON_ENABLEDNESS_DOCS", 12)
}

fn headlines_per_doc() -> usize {
    env_usize("HOLON_ENABLEDNESS_HEADLINES", 22)
}

fn runs() -> usize {
    env_usize("HOLON_ENABLEDNESS_RUNS", 200)
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
    format!("eq-d{doc:04}-h{headline:03}")
}

fn synthesized_document(doc: usize, n: usize) -> String {
    let mut out = String::with_capacity(n * 160);
    out.push_str(&format!(
        "#+TITLE: Enabledness Doc {doc}\n#+ID: eq-doc-{doc:04}\n"
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

async fn block_count(env: &TestEnvironment) -> usize {
    env.query_sql("SELECT id FROM block_raw")
        .await
        .expect("count block_raw")
        .len()
}

/// One read per subject, shared across every transition.
struct RowMarking {
    present: bool,
}

impl Marking for RowMarking {
    fn present(&self, _: &ArcRelation, _: &EntityUri) -> bool {
        self.present
    }
}

/// Linear-interpolated percentile, matching `scripts/measure_latency.py::pct`.
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
        "[enabledness-prod] {step:<26} n={:<4} p50={p50:8.3} p95={p95:8.3} max={max:8.3}  ms",
        samples.len()
    );
    p95
}

#[test]
fn enabledness_query_cost_in_the_production_composition() {
    let (d, h, n) = (docs(), headlines_per_doc(), runs());
    holon_integration_tests::test_tracing::SpanCollector::global();

    let rt = runtime();
    rt.clone().block_on(async move {
        let mut builder = TestEnvironmentBuilder::new();
        for doc in 0..d {
            builder = builder.with_org_file(
                format!("Enabledness/Doc{doc:04}.org"),
                synthesized_document(doc, h),
            );
        }

        let t_boot = Instant::now();
        let env = builder.build(rt.clone()).await.expect("boot the vault");
        let blocks = block_count(&env).await;
        eprintln!(
            "[enabledness-prod] boot: {d} files / {blocks} blocks in {:?} (Loro authority ON)",
            t_boot.elapsed()
        );
        assert!(
            blocks > d,
            "only {blocks} blocks landed from {d} files — the corpus never ingested, so any \
             figure below would be at the wrong scale"
        );

        env.wait_for_loro_quiescence(Duration::from_secs(120)).await;
        env.wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;

        let engine = env.engine();
        let subject_raw = block_id(d / 2, h / 2);
        // ALLOW(entity_uri_from_raw): a test-authored bare org id; `block_raw`
        // stores the schemed form.
        let subject = EntityUri::from_raw(&subject_raw);
        let sql = format!("SELECT * FROM block_raw WHERE id = '{subject}'");
        let relation = ArcRelation::block();

        // Recomputed per call and held nowhere, by ADR 0032 §2.
        let mut derive = Vec::with_capacity(n);
        let mut transitions = 0usize;
        for _ in 0..n {
            let t = Instant::now();
            let net = engine.derived_net().expect("derive the net");
            derive.push(t.elapsed());
            transitions = net.transitions.len();
        }
        assert!(
            transitions > 0,
            "an empty net would make every figure vacuous"
        );
        eprintln!("[enabledness-prod] catalog: {transitions} transitions");
        let p95_derive = report("derived_net()", derive);

        let mut read = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let rows = env.query_sql(&sql).await.expect("marking read");
            assert_eq!(rows.len(), 1, "the subject {subject} must be in the store");
            read.push(t.elapsed());
        }
        let p95_read = report("subject marking read", read);

        let net = engine.derived_net().expect("derive the net");
        let marking = RowMarking { present: true };
        let mut eval = Vec::with_capacity(n);
        let mut offers = Vec::new();
        for _ in 0..n {
            let t = Instant::now();
            offers = evaluate(&net, &marking, &relation, &subject);
            eval.push(t.elapsed());
        }
        let p95_eval = report("enabledness::evaluate()", eval);

        let refused = offers
            .iter()
            .filter(|o| matches!(o.offer, Offer::Refused { .. }))
            .count();
        let unknown = offers
            .iter()
            .filter(|o| matches!(o.offer, Offer::Unknown { .. }))
            .count();
        let enabled = offers.iter().filter(|o| o.offer == Offer::Enabled).count();
        eprintln!(
            "[enabledness-prod] offers for a PRESENT subject: enabled={enabled} unknown={unknown} \
             refused={refused} of {}",
            offers.len()
        );

        // A subject the store does not hold.
        let ghost = evaluate(&net, &RowMarking { present: false }, &relation, &subject);
        let ghost_refused = ghost
            .iter()
            .filter(|o| matches!(o.offer, Offer::Refused { .. }))
            .count();
        eprintln!(
            "[enabledness-prod] offers for an ABSENT subject: refused={ghost_refused} of {}",
            ghost.len()
        );

        eprintln!(
            "[enabledness-prod] SUM OF p95 STEPS = {:.3} ms  ",
            p95_derive + p95_read + p95_eval
        );
    });
}
