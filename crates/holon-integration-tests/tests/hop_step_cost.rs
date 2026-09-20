//! What one correlated hop costs the EVALUATOR in the production
//! composition.
//!
//! The hop is the only genuinely new capability the extended arc language
//! needs, and the thing that must be timed is `enabledness::evaluate()`
//! itself, hopping through a [`Marking`] backed by the database. A harness
//! that hand-writes the hop as SQL beside a non-hopping evaluator measures
//! two things neither of which is the feature, so this one:
//!
//! - compiles a real `sibling(...)` guard into the transition it times,
//! - answers every `present`/`value`/`matching` out of the store,
//! - picks a subject with a real sibling fan-out and reports the vault's whole
//!   fan-out distribution beside the time,
//! - asserts non-vacuity: the timed evaluation reached siblings OTHER than the
//!   subject and paid a refinement read on them.
//!
//! Two markings are timed because the trait admits two honest
//! implementations, and the gap between them is the hop's real risk:
//!
//! - PREFETCH: `matching()` selects the reached rows' columns, which the lookup
//!   has to scan anyway, so the refinements that follow are already paid for.
//!   This is what a production marking should do.
//! - PER-ENTITY: `matching()` selects ids only and each refinement reads its
//!   own row. The pessimistic bound, and what a naive marking would cost.
//!
//! Timed against the D152.a rung, a p50 SUM of 5 ms. Wall clock is printed,
//! never asserted: `vault_scale_interaction_latency.rs` records the 4.7x
//! run-to-run spread on this hardware. The assertions cover scale and
//! non-vacuity.
//!
//! Scale is env-gated. `hop_step_cost` is the BINARY, so it selects nothing
//! as a filter — name the binary with `--test`:
//!
//!   HOLON_ENABLEDNESS_DOCS=140 cargo nextest run -p holon-integration-tests \
//!     --test hop_step_cost --no-capture
//!
//! @pbt kind harness
//! @pbt covers arc-hop-step-cost — one correlated hop at vault scale

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::ArcPlace;
use holon_api::ArcRelation;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::pattern::CmpOp;
use holon_api::pattern::FieldRef;
use holon_api::pattern::Guard;
use holon_api::pattern::Operand;
use holon_api::pattern::Pattern;
use holon_api::pattern::Subject;
use holon_api::widget_spec::DataRow;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_net::Analyzability;
use holon_net::CompiledNet;
use holon_net::NetEntity;
use holon_net::NetTransition;
use holon_net::TransitionSource;
use holon_net::enabledness::Offer;
use holon_net::enabledness::evaluate;
use holon_net::guards::classify_guard;
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

/// Children of the one FLAT document. The nested ladder tops out at a
/// handful of siblings, which would time the hop on a fan-out no sibling
/// predicate would ever be interesting on. A long flat page is the ordinary
/// PKM shape and the one that costs.
fn fanout() -> usize {
    env_usize("HOLON_HOP_FANOUT", 50)
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

/// The p95 sibling fan-out of a real vault, which D165.a makes the GATED
/// case. The measured distribution at this scale is min 1 / p50 2 / p95 5,
/// so five is the width an ordinary offer actually faces.
fn p95_fanout() -> usize {
    env_usize("HOLON_HOP_FANOUT_P95", 5)
}

fn family_id(prefix: &str, headline: usize) -> String {
    format!("eq-{prefix}-h{headline:03}")
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

/// One parent, `n` children, all at level 1. Only the child that sorts LAST
/// by id satisfies the timed refinement, so the evaluation walks the whole
/// family before it can answer — the hop's worst case, not its luckiest.
fn flat_document(label: &str, prefix: &str, n: usize) -> String {
    let mut out = String::with_capacity(n * 160);
    out.push_str(&format!(
        "#+TITLE: Enabledness {label}\n#+ID: eq-doc-{prefix}\n"
    ));
    for i in 0..n {
        out.push_str(&format!(
            "* {label} {i}\n:PROPERTIES:\n:ID: {}\n:END:\n",
            family_id(prefix, i)
        ));
    }
    out
}

/// The headline text of the last flat child — the one cell in the family
/// that the timed refinement matches.
fn last_content(label: &str, n: usize) -> String {
    format!("{label} {}", n - 1)
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

/// What the timed evaluation actually did, so a figure cannot come from an
/// evaluator that read nothing.
#[derive(Default, Debug)]
struct Traffic {
    /// Statements issued against the store.
    queries: usize,
    /// Distinct entities the hop reached, excluding the subject.
    siblings_reached: usize,
    /// Refinement reads served for an entity that is not the subject.
    refinement_reads_off_subject: usize,
}

/// A [`Marking`] answering out of the store, with the row cache a production
/// marking would keep for the span of one offer.
struct SqlMarking<'a> {
    handle: tokio::runtime::Handle,
    /// Borrowed, not shared: every marking here is built, timed and dropped
    /// inside one `block_in_place`, on one thread.
    env: &'a TestEnvironment,
    subject: EntityUri,
    /// Whether `matching` also materializes the reached rows' columns.
    prefetch: bool,
    rows: RefCell<HashMap<String, DataRow>>,
    traffic: RefCell<Traffic>,
}

impl<'a> SqlMarking<'a> {
    fn new(
        handle: tokio::runtime::Handle,
        env: &'a TestEnvironment,
        subject: EntityUri,
        prefetch: bool,
    ) -> Self {
        SqlMarking {
            handle,
            env,
            subject,
            prefetch,
            rows: RefCell::new(HashMap::new()),
            traffic: RefCell::new(Traffic::default()),
        }
    }

    /// Between timed runs: a cache surviving the loop would make every run
    /// after the first measure nothing.
    fn reset(&self) {
        self.rows.borrow_mut().clear();
        *self.traffic.borrow_mut() = Traffic::default();
    }

    fn query(&self, sql: &str) -> Vec<DataRow> {
        self.traffic.borrow_mut().queries += 1;
        self.handle
            .block_on(self.env.query_sql(sql))
            .unwrap_or_else(|e| panic!("marking query failed: {sql}: {e}"))
    }

    /// The subject's row, from the cache or the store.
    fn row(&self, entity: &EntityUri) -> Option<DataRow> {
        if let Some(row) = self.rows.borrow().get(entity.as_str()) {
            return Some(row.clone());
        }
        let sql = format!("SELECT * FROM block_raw WHERE id = '{entity}'");
        let row = self.query(&sql).into_iter().next()?;
        self.rows
            .borrow_mut()
            .insert(entity.to_string(), row.clone());
        Some(row)
    }
}

impl Marking for SqlMarking<'_> {
    fn present(&self, _: &ArcRelation, entity: &EntityUri) -> bool {
        self.row(entity).is_some()
    }

    fn value(&self, place: &ArcPlace, entity: &EntityUri) -> Option<Value> {
        if entity != &self.subject {
            self.traffic.borrow_mut().refinement_reads_off_subject += 1;
        }
        let row = self.row(entity)?;
        // The contract: an empty cell is `None`, never `Some(Value::Null)`.
        match row.get(&place.field) {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.clone()),
        }
    }

    fn matching(&self, to: &ArcPlace, value: &Value) -> Vec<EntityUri> {
        let literal = value.as_string().expect("a hop correlates on a text cell");
        let columns = if self.prefetch { "*" } else { "id" };
        let sql = format!(
            "SELECT {columns} FROM block_raw WHERE {} = '{literal}'",
            to.field
        );
        let mut rows = self.query(&sql);
        // Sorted HERE and not by SQL: `ORDER BY id` turns the indexed seek on
        // `parent_id` into a scan-and-sort over the whole table, which would
        // make the figure grow with the vault rather than with the family.
        // The order matters because the refinement short-circuits on the
        // first match.
        rows.sort_by_key(|r| {
            r.get("id")
                .and_then(Value::as_string)
                .unwrap_or_default()
                .to_string()
        });
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(id) = row.get("id").and_then(Value::as_string) else {
                continue;
            };
            let entity = EntityUri::parse(id).expect("block_raw stores schemed ids");
            if self.prefetch {
                self.rows.borrow_mut().insert(id.to_string(), row);
            }
            if entity != self.subject {
                self.traffic.borrow_mut().siblings_reached += 1;
            }
            out.push(entity);
        }
        out
    }
}

/// `sibling(block.content == <last child>)` — the containment shape, compiled
/// the way production would compile it. One column on one sibling, which is
/// the cheapest refinement the language has: anything richer would measure
/// the refinement rather than the hop.
fn sibling_hop_net(match_content: String) -> CompiledNet {
    let guard = Guard {
        subject: Subject::Block,
        body: Pattern::Sibling(Box::new(Pattern::Field {
            field: FieldRef::Column {
                relation: "block".to_string(),
                name: "content".to_string(),
            },
            op: CmpOp::Eq,
            rhs: Operand::Lit(Value::String(match_content)),
        })),
    };
    let classified =
        classify_guard(&guard, "op:block.hop_probe").expect("a one-step hop is within the bounds");
    assert!(
        classified.residue.is_empty(),
        "the timed transition must carry the hop as an ARC, not as residue: {:?}",
        classified.residue
    );
    assert_eq!(
        classified.modes.iter().map(|m| m.hops.len()).sum::<usize>(),
        1,
        "exactly one correlated group is what this harness times"
    );
    CompiledNet {
        transitions: vec![NetTransition {
            source: TransitionSource::Operation {
                entity: NetEntity::parse("block").expect("dotless"),
                op: "hop_probe".to_string(),
            },
            analyzability: Analyzability::Analyzable,
            modes: classified.modes,
            residue: classified.residue,
        }],
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

/// Returns the p50, which is what the D152.a rung scores; p95 is printed
/// beside it because the rung leaves it ungated.
fn report(step: &str, mut samples: Vec<Duration>) -> f64 {
    samples.sort();
    let (p50, p95, max) = (pct(&samples, 0.50), pct(&samples, 0.95), pct(&samples, 1.0));
    eprintln!(
        "[hop-cost] {step:<34} n={:<4} p50={p50:8.3} p95={p95:8.3} max={max:8.3}  ms",
        samples.len()
    );
    p50
}

/// Time `evaluate` over `net`, resetting the marking before every run so no
/// run is served from the previous one's cache.
fn time_evaluate(
    n: usize,
    net: &CompiledNet,
    marking: &SqlMarking<'_>,
    relation: &ArcRelation,
    subject: &EntityUri,
) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        marking.reset();
        let t = Instant::now();
        let _ = evaluate(net, marking, relation, subject);
        samples.push(t.elapsed());
    }
    samples
}

#[test]
fn one_hop_cost_in_the_production_composition() {
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
        builder = builder.with_org_file(
            "Enabledness/Wide.org".to_string(),
            flat_document("Wide", "wide", fanout()),
        );
        builder = builder.with_org_file(
            "Enabledness/Narrow.org".to_string(),
            flat_document("Narrow", "narrow", p95_fanout()),
        );

        let t_boot = Instant::now();
        let world = builder.build(rt.clone()).await.expect("boot the vault");
        let blocks = block_count(&world).await;
        eprintln!(
            "[hop-cost] boot: {} files / {blocks} blocks in {:?} (Loro authority ON)",
            d + 1,
            t_boot.elapsed()
        );
        assert!(
            blocks > d,
            "only {blocks} blocks landed from {d} files - the corpus never ingested, so any \
             figure below would be at the wrong scale"
        );

        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;

        // The fan-out the hop actually faces, reported rather than assumed.
        let families = world
            .query_sql(
                "SELECT parent_id, COUNT(*) AS n FROM block_raw WHERE parent_id IS NOT NULL \
                 GROUP BY parent_id",
            )
            .await
            .expect("fan-out distribution");
        let mut widths: Vec<usize> = families
            .iter()
            .filter_map(|r| r.get("n").and_then(Value::as_f64))
            .map(|n| n as usize)
            .collect();
        widths.sort_unstable();
        let width_at = |p: f64| widths[(p * (widths.len() - 1) as f64).round() as usize];
        eprintln!(
            "[hop-cost] sibling fan-out over {} parents: min={} p50={} p95={} max={}",
            widths.len(),
            widths[0],
            width_at(0.50),
            width_at(0.95),
            widths[widths.len() - 1]
        );

        let engine = world.engine();
        let relation = ArcRelation::block();

        let production = engine.derived_net().expect("derive the net");
        eprintln!(
            "[hop-cost] catalog: {} transitions",
            production.transitions.len()
        );

        let mut derive = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let _ = engine.derived_net().expect("derive the net");
            derive.push(t.elapsed());
        }
        let p50_derive = report("derived_net()", derive);

        // Blocking SQL from inside the sync `Marking` methods; the runtime is
        // multi-threaded, so the worker hands its other tasks off first.
        let (p50_prod, narrow, wide) = tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();

            // ALLOW(entity_uri_from_raw): test-authored bare org ids;
            // `block_raw` stores the schemed form.
            let any_subject = EntityUri::from_raw(&family_id("wide", fanout() / 2));
            let prod_marking = SqlMarking::new(handle.clone(), &world, any_subject.clone(), true);
            let p50_prod = report(
                "evaluate(), production net",
                time_evaluate(n, &production, &prod_marking, &relation, &any_subject),
            );

            let narrow = time_family(
                &handle,
                &world,
                &relation,
                n,
                "Narrow",
                "narrow",
                p95_fanout(),
            );
            let wide = time_family(&handle, &world, &relation, n, "Wide", "wide", fanout());
            (p50_prod, narrow, wide)
        });

        eprintln!(
            "[hop-cost] SUM OF p50 STEPS, no hop                  = {:.3} ms",
            p50_derive + p50_prod
        );
        let gated = p50_derive + narrow.prefetch;
        eprintln!(
            "[hop-cost] SUM p50, p95 fan-out ({:>2}), prefetch      = {gated:.3} ms   \
             (D165.a GATE: {D165A_GATE_MS} ms)",
            p95_fanout()
        );
        eprintln!(
            "[hop-cost] SUM p50, p95 fan-out ({:>2}), per-entity    = {:.3} ms",
            p95_fanout(),
            p50_derive + narrow.per_entity
        );
        eprintln!(
            "[hop-cost] SUM p50, wide fan-out ({:>2}), prefetch     = {:.3} ms   \
             (D165.a: printed + ratcheted)",
            fanout(),
            p50_derive + wide.prefetch
        );
        eprintln!(
            "[hop-cost] SUM p50, wide fan-out ({:>2}), per-entity   = {:.3} ms",
            fanout(),
            p50_derive + wide.per_entity
        );

        // D165.a: the p95 fan-out case is the RUNG, and it is gated. The wide
        // case is printed and ratcheted by the report, not asserted — the
        // harness header records the 4.7x run-to-run spread that makes a wall
        // clock a poor assertion, and five siblings leaves enough headroom to
        // survive it while fifty does not.
        assert!(
            gated <= D165A_GATE_MS,
            "the p95 fan-out case must fit the D152.a rung with a conforming (prefetching) \
             marking: {gated:.3} ms > {D165A_GATE_MS} ms"
        );
    });
}

/// The D152.a rung, as D165.a scopes it: the p95 fan-out case is gated, the
/// wide case is printed.
const D165A_GATE_MS: f64 = 5.0;

/// One family's two figures.
struct FamilyCost {
    prefetch: f64,
    per_entity: f64,
}

/// Time both markings against one family, with the conformance check after
/// each. Returns the two p50s.
fn time_family(
    handle: &tokio::runtime::Handle,
    world: &TestEnvironment,
    relation: &ArcRelation,
    n: usize,
    label: &str,
    prefix: &str,
    width: usize,
) -> FamilyCost {
    // ALLOW(entity_uri_from_raw): a test-authored bare org id; `block_raw`
    // stores the schemed form.
    let subject = EntityUri::from_raw(&family_id(prefix, width / 2));
    let net = sibling_hop_net(last_content(label, width));

    let prefetch_marking = SqlMarking::new(handle.clone(), world, subject.clone(), true);
    let prefetch = report(
        &format!("evaluate() + hop, {label} ({width}), prefetch"),
        time_evaluate(n, &net, &prefetch_marking, relation, &subject),
    );
    let prefetch_reads = assert_hop_was_real(&prefetch_marking, &net, relation, &subject, width);

    let per_entity_marking = SqlMarking::new(handle.clone(), world, subject.clone(), false);
    let per_entity = report(
        &format!("evaluate() + hop, {label} ({width}), per-entity"),
        time_evaluate(n, &net, &per_entity_marking, relation, &subject),
    );
    let per_entity_reads =
        assert_hop_was_real(&per_entity_marking, &net, relation, &subject, width);

    // D165.a's contract, checked mechanically: a conforming `matching()`
    // materializes the reached rows, so ONE hop costs exactly two statements
    // whatever the family's width. The non-conforming marking is detected by
    // the same counter, and the gap is what the contract buys.
    assert_eq!(
        prefetch_reads, 2,
        "a conforming marking answers one hop in two statements ({label}, width {width})"
    );
    assert_eq!(
        per_entity_reads,
        width + 1,
        "the violating marking must be detected by its read count ({label}, width {width})"
    );
    eprintln!(
        "[hop-cost] D165.a conformance, {label} ({width}): prefetch {prefetch_reads} \
         statement(s) vs per-entity {per_entity_reads}"
    );

    FamilyCost {
        prefetch,
        per_entity,
    }
}

/// The figures above are worth nothing unless the timed evaluation hopped.
/// Re-runs the same evaluation once and returns the statements it issued.
fn assert_hop_was_real(
    marking: &SqlMarking<'_>,
    net: &CompiledNet,
    relation: &ArcRelation,
    subject: &EntityUri,
    width: usize,
) -> usize {
    marking.reset();
    let offers = evaluate(net, marking, relation, subject);
    let traffic = marking.traffic.borrow();
    eprintln!("[hop-cost] traffic of one timed evaluation: {traffic:?}");
    assert!(
        traffic.queries >= 2,
        "the evaluator must have read the store: {traffic:?}"
    );
    assert!(
        traffic.siblings_reached > 0,
        "the hop reached no entity other than the subject, so the figure measures an empty \
         lookup: {traffic:?}"
    );
    // The whole family, not a lucky early hit: the matching sibling sorts
    // last, so a smaller count means the walk short-circuited and the timing
    // above is of less work than a sibling predicate can cost.
    assert_eq!(
        traffic.refinement_reads_off_subject,
        width - 1,
        "the refinement must have been tested against every reached sibling: {traffic:?}"
    );
    assert_eq!(
        offers.len(),
        1,
        "the hop net carries exactly one transition"
    );
    assert_eq!(
        offers[0].offer,
        Offer::Enabled,
        "the family's last child carries the matched content, so the hop must find it; the rows \
         it saw were {:?}",
        marking
            .rows
            .borrow()
            .values()
            .filter_map(|r| r.get("content").cloned())
            .collect::<Vec<_>>()
    );
    traffic.queries
}
