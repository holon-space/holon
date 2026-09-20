//! D156.b risk spike: what the destination-capability clause costs, and
//! whether it can be a 2-step hop at all.
//!
//! D156.b (PROVISIONAL) models `MoveGuard`'s destination-capability clause as
//! a static capability place reached by a 2-step hop, `block → document home
//! → format`. Two things have to be true for that: the shape must be
//! EXPRESSIBLE under D158.a's 2-step bound, and it must be AFFORDABLE under
//! D152.a's 5 ms p50 SUM. This harness measures the second and the module
//! docs of `expressibility` below record what the code says about the first.
//!
//! Method is Inc 1's: real work, counted, never a shape-only stand-in. The
//! numbers come from `[cap-hop]` lines in the log, never retyped.
//!
//! Scale is env-gated. `capability_hop_cost` is the BINARY, so name it with
//! `--test`:
//!
//!   HOLON_ENABLEDNESS_DOCS=140 cargo nextest run -p holon-integration-tests \
//!     --test capability_hop_cost --no-capture
//!
//! @pbt kind harness
//! @pbt covers d156b-capability-hop-cost — the destination-capability clause at
//! vault scale

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::api::net_guard::Confirmation;
use holon::api::net_guard::NetGuard;
use holon::api::net_guard::NetGuardOp;
use holon::api::net_guard::NetVerdict;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::live_data::home_by::HomeAuthority;
use holon_app::move_guard::MoveGuard;
use holon_capability::EntityKind;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::sync_ports::BlockReader;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::HomeBurstMemo;

/// Headline ladder, shared with `hop_step_cost.rs` so the two harnesses
/// stress the same vault shape and their figures are comparable.
const LEVEL_LADDER: &[usize] = &[1, 2, 3, 2, 2, 3, 4, 3, 2, 1, 2, 3, 3, 2, 3];

/// Depth of the one DEEP document. The ladder tops out at four levels, and
/// step 1 of the D156.b hop is an ancestor WALK, so a harness that only ever
/// walks four levels would report the cost of the shallowest vault anyone
/// has. Twenty is an ordinary outline depth.
fn deep_chain() -> usize {
    env_usize("HOLON_CAP_DEPTH", 20)
}

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
    format!("cap-d{doc:04}-h{headline:03}")
}

fn deep_id(level: usize) -> String {
    format!("cap-deep-l{level:03}")
}

fn synthesized_document(doc: usize, n: usize) -> String {
    let mut out = String::with_capacity(n * 160);
    out.push_str(&format!(
        "#+TITLE: Capability Doc {doc}\n#+ID: cap-doc-{doc:04}\n"
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

/// One straight chain, each headline a child of the one above it, so the
/// deepest block sits `depth` parents below the page.
fn deep_document(depth: usize) -> String {
    let mut out = String::with_capacity(depth * 160);
    out.push_str("#+TITLE: Capability Deep\n#+ID: cap-doc-deep\n");
    for level in 1..=depth {
        out.push_str(&"*".repeat(level));
        out.push_str(&format!(
            " Deep {level}\n:PROPERTIES:\n:ID: {}\n:END:\n",
            deep_id(level)
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
        "[cap-hop] {step:<38} n={:<4} p50={p50:8.3} p95={p95:8.3} max={max:8.3}  ms",
        samples.len()
    );
    p50
}

fn move_params(id: &str, destination: &str) -> HashMap<Arc<str>, Value> {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert(Arc::from("id"), Value::String(id.to_string()));
    params.insert(
        Arc::from("parent_id"),
        Value::String(destination.to_string()),
    );
    params
}

/// The ancestor walk of D156.b step 1, served from an in-memory snapshot
/// instead of the store — Martin's `LiveData` alternative, costed.
///
/// Deliberately the SAME algorithm as
/// `holon_filesystem::sync_ports::nearest_page_ancestor`: only the read
/// changes, so the difference between the two figures is the read and
/// nothing else.
/// Every kind a move can carry.
///
/// `EntityKind` offers no iteration, so the exhaustive `match` is the guard: a
/// new variant fails to compile here rather than silently narrowing the pin
/// below to the kinds whoever wrote this list remembered.
fn every_movable_kind() -> BTreeSet<EntityKind> {
    let listed = [EntityKind::Block, EntityKind::Page, EntityKind::Program];
    for kind in listed {
        match kind {
            EntityKind::Block | EntityKind::Page | EntityKind::Program => {}
        }
    }
    listed.into_iter().collect()
}

fn walk_home_in_memory(rows: &HashMap<String, Block>, start: &EntityUri) -> Option<String> {
    let root = EntityUri::no_parent();
    let mut cur = start.clone();
    for _ in 0..100 {
        if cur == root {
            return None;
        }
        let block = rows.get(cur.as_str())?;
        if block.is_page() {
            return Some(block.id.to_string());
        }
        cur = block.parent_id.clone();
    }
    None
}

#[test]
fn destination_capability_clause_cost() {
    let (d, h, n) = (docs(), headlines_per_doc(), runs());
    holon_integration_tests::test_tracing::SpanCollector::global();

    let rt = runtime();
    rt.clone().block_on(async move {
        let mut builder = TestEnvironmentBuilder::new();
        for doc in 0..d {
            builder = builder.with_org_file(
                format!("Capability/Doc{doc:04}.org"),
                synthesized_document(doc, h),
            );
        }
        builder = builder.with_org_file(
            "Capability/Deep.org".to_string(),
            deep_document(deep_chain()),
        );

        let t_boot = Instant::now();
        let world = builder.build(rt.clone()).await.expect("boot the vault");
        let blocks = block_count(&world).await;
        eprintln!(
            "[cap-hop] boot: {} files / {blocks} blocks in {:?} (Loro authority ON)",
            d + 1,
            t_boot.elapsed()
        );
        assert!(
            blocks > d,
            "only {blocks} blocks landed from {d} files - the corpus never ingested"
        );
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
        let registry = Arc::new(
            holon_capability::registry::shipped_profiles()
                .expect("the shipped capability profiles must parse"),
        );

        // The destination: the deepest block of the deep chain, so step 1's
        // walk is the full chain rather than one hop that happens to land.
        // ALLOW(entity_uri_from_raw): test-authored bare org ids.
        let destination = EntityUri::from_raw(&deep_id(deep_chain()));
        let subject = EntityUri::from_raw(&block_id(d / 2, h / 2));

        // ---------------------------------------------------- expressibility
        //
        // Measured facts about the SHAPE, printed beside the cost because a
        // cheap shape that cannot be written is not a result.
        let reader = injector.resolve_async::<dyn BlockReader>().await;
        let ordering = injector.resolve_async::<dyn BlockOrdering>().await;
        let authority = BlockHomeAuthority::new(reader.clone(), ordering.clone());
        authority.reset_locate_reads();
        let mut memo = HomeBurstMemo::default();
        let placement = authority
            .locate(destination.as_str(), &mut memo)
            .await
            .expect("locating the destination")
            .expect("the destination is a block the store holds");
        let walk_reads = authority.locate_reads();
        eprintln!(
            "[cap-hop] step 1 is a WALK: locating one destination cost {walk_reads} \
             authoritative point read(s); home = {:?}",
            placement.doc
        );
        assert!(
            walk_reads > 2,
            "the deep destination must exercise a multi-level walk, not a single lookup: \
             {walk_reads} read(s)"
        );

        // ---------------------------------------------------------- the cost
        let guard = MoveGuard::new(injector.clone(), registry.clone());
        let params = move_params(subject.as_str(), destination.as_str());
        let confirmation = Confirmation::parse(&params).expect("confirmation parses");

        let mut whole = Vec::with_capacity(n);
        let mut verdict_was_reached = false;
        for _ in 0..n {
            let t = Instant::now();
            let verdict = guard
                .check(&NetGuardOp {
                    entity_name: "block",
                    op_name: "move_block",
                    params: &params,
                    confirmation,
                })
                .await
                .expect("the guard reaches a verdict");
            whole.push(t.elapsed());
            verdict_was_reached = matches!(verdict, NetVerdict::Confirm | NetVerdict::Refuse(_));
        }
        assert!(verdict_was_reached, "the timed guard produced no verdict");
        let p50_guard = report("MoveGuard::check(), whole clause", whole);

        // Step 1 alone, one FRESH memo per run: a memo is per-offer, so a
        // memo shared across runs would price the second offer, not the first.
        let mut step1 = Vec::with_capacity(n);
        for _ in 0..n {
            let mut memo = HomeBurstMemo::default();
            let t = Instant::now();
            let _ = authority
                .locate(destination.as_str(), &mut memo)
                .await
                .expect("locating the destination");
            step1.push(t.elapsed());
        }
        let p50_step1 = report("step 1: home walk over the store", step1);

        // The same walk over an in-memory snapshot of every block — the
        // `LiveData` alternative. The snapshot is built ONCE, outside the
        // timed loop, exactly as a live mirror would already be resident.
        let snapshot: HashMap<String, Block> = {
            let mut rows = HashMap::new();
            for (doc, doc_blocks) in reader
                .iter_documents_with_blocks()
                .await
                .expect("snapshot every document")
            {
                for block in doc_blocks {
                    rows.insert(block.id.to_string(), block);
                }
                // The page block itself is the document KEY, not one of its
                // blocks, and it is exactly where the walk terminates — a
                // snapshot without it would end every walk in a miss.
                if let Some(page) = reader
                    .get_block_authoritative(&doc)
                    .await
                    .expect("the document's own row")
                {
                    rows.insert(page.id.to_string(), page);
                }
            }
            rows
        };
        eprintln!(
            "[cap-hop] in-memory snapshot holds {} blocks",
            snapshot.len()
        );
        let mut step1_mem = Vec::with_capacity(n);
        let mut reached_home = None;
        for _ in 0..n {
            let t = Instant::now();
            reached_home = walk_home_in_memory(&snapshot, &destination);
            step1_mem.push(t.elapsed());
        }
        assert!(
            reached_home.is_some(),
            "the in-memory walk must reach the same home the store walk did, or the two \
             figures are not of the same work"
        );
        let p50_step1_mem = report("step 1: home walk in memory", step1_mem);

        // Step 2, the static capability place: profile lookup + hosted kinds.
        let home =
            holon_capability::profile_of(Some(holon_api::live_data::home_by::DurableFormat::Org));
        let mut step2 = Vec::with_capacity(n);
        let mut hosted = 0usize;
        for _ in 0..n {
            let t = Instant::now();
            let profile = registry.get(&home).expect("the org profile is shipped");
            hosted = profile.hosted_entity_kinds().len();
            step2.push(t.elapsed());
        }
        let p50_step2 = report("step 2: static capability place", step2);

        // Non-vacuity of the static place — and the answer is NO.
        //
        // `destination_hosts_kind` can only ever refuse if SOME reachable
        // home declines SOME movable kind, and every registered profile
        // declares them all, so whichever home the walk reaches, the clause
        // confirms. Pinned rather
        // than asserted away, because the pin is the finding: the day one
        // registered profile stops hosting one kind, this reds and D156.b
        // becomes a live question instead of a hypothetical one.
        let movable = every_movable_kind();
        let native = holon_capability::profile_of(None);
        let org_kinds = registry
            .get(&home)
            .expect("the org profile is shipped")
            .hosted_entity_kinds();
        let native_kinds = registry
            .get(&native)
            .expect("the holon-native profile is shipped")
            .hosted_entity_kinds();
        eprintln!(
            "[cap-hop] capability place: `{home}` hosts {hosted} kind(s) {org_kinds:?}; \
             `{native}` hosts {native_kinds:?}"
        );
        let registered: Vec<_> = registry.ids().cloned().collect();
        assert!(
            !registered.is_empty(),
            "an empty registry would satisfy the pin below vacuously"
        );
        for id in &registered {
            let kinds = registry
                .get(id)
                .expect("an id the registry just listed")
                .hosted_entity_kinds();
            let declined: Vec<_> = movable.difference(kinds).collect();
            assert!(
                declined.is_empty(),
                "registered profile `{id}` no longer hosts {declined:?} — the \
                 destination-capability clause has gained a false case, so D156.b must be \
                 re-costed against it"
            );
        }

        eprintln!(
            "[cap-hop] SUM OF p50, store walk    = {:.3} ms   (D152.a rung: 5 ms)",
            p50_step1 + p50_step2
        );
        eprintln!(
            "[cap-hop] SUM OF p50, in-memory walk = {:.3} ms   (D152.a rung: 5 ms)",
            p50_step1_mem + p50_step2
        );
        eprintln!(
            "[cap-hop] MoveGuard::check() p50     = {:.3} ms   (D152.a rung: 5 ms)",
            p50_guard
        );
    });
}
