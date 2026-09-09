//! Interaction latency at REAL-VAULT scale — the rung the funnel was missing.
//!
//! Two OPEN bugfunnel entries name the same defect from opposite sides:
//! `2026-09-02-projection-misses-the-slo-on-the-real-vault` (gap ENVIRONMENT —
//! the budget holds at fixture scale and fails at vault scale) and
//! `2026-09-03-navigate-latency-is-twenty-seconds-at-real-vault-scale` (gap
//! ORACLE — `inv-settle-budget` / `inv-sql-budget` are class-3 temporal checks
//! `run_self_checks` skips on the live app, so nothing scores end-to-end
//! latency at that scale).
//!
//! `single_large_document_scale.rs` covers the OTHER scale axis: one document
//! with thousands of blocks. Martin's vault is the opposite shape — 121 files
//! holding 2621 headlines, ~22 apiece — and the two shapes stress different
//! code. This file pins the many-documents axis and, unlike that one, measures
//! POST-BOOT INTERACTION latency rather than whether boot completes at all.
//!
//! WHAT IS MEASURED
//! `set_field` here is dispatch -> the edited content readable in `block_raw`,
//! the projection sink. That is the same endpoint the prod `stage=e2e`
//! correlator closes on (crates/holon-api/src/latency_e2e.rs "SLO endpoint"):
//! projection-visible, deliberately not GPU frame-present. `navigate` is a
//! cold `watch_ui` on a document root through to its first delivered structure
//! — the work a page switch actually pays.
//!
//! WHAT IS ASSERTED VERSUS WHAT IS REPORTED
//! The 200 ms SLO is REPORTED, not asserted: both rungs still miss it and the
//! two entries above stay open until they do not. Asserting it would mean
//! landing a permanently-red test. What IS asserted is one load-insensitive
//! property — a cold boot takes at most two full-document projection walks —
//! — see [`MAX_FULL_PASSES`]. The wall-clock figures are printed, never
//! asserted; the constant beside `SLO` records the measured spread that made
//! any bound on them meaningless here.
//!
//! SCALE IS ENV-GATED
//! A default sweep pays a small vault; the vault rung is asked for explicitly:
//!
//!   HOLON_VAULT_SCALE_DOCS=120 cargo nextest run -p holon-integration-tests \
//!     vault_scale_interaction_latency --no-capture
//!
//! `HOLON_LATENCY_VAULT_DIR=<dir>` replaces the synthesized corpus with the
//! `.org` files under that directory. It exists so a measurement can be taken
//! against a COPY of a real vault without that content ever entering the repo;
//! no gate sets it.
//!
//! @pbt kind harness
//! @pbt covers vault-scale-interaction-latency — a keystroke edit and a cold
//! page switch stay inside the interaction budget on a many-document vault

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;

/// The project SLO (CLAUDE.md): interaction -> projection-visible.
const SLO: Duration = Duration::from_millis(200);

// NO WALL-CLOCK RUNG IS ASSERTED HERE, and that is a measured decision rather
// than caution. Six runs of the SAME tree at 2793 blocks on this shared
// machine produced navigate p50s of 301, 322, 333, 401, 466 and 1407 ms — a
// 4.7x spread with the code held constant, and boot times from 42 s to 151 s
// over the same runs. A bound loose enough not to flap at 1407 ms would be
// past 3 s, which no longer distinguishes a regression from the defect this
// file was written against. So the wall-clock figures are PRINTED for a human
// (and for `scripts/measure_latency.py`, which parses the same run's stage
// events), and the one assertion is `MAX_FULL_PASSES` — a ratio machine load
// cannot move.

/// The teeth for the defect this file was written against, and the one
/// assertion machine load cannot move: a cold boot needs exactly ONE
/// full-document reseed walk to seed the incremental base. One per ingested
/// file is the quadratic — measured 121 walks for a 120-file boot before the
/// fix. The slack of one covers a legitimate second seed (an unsettled or
/// orphan reseed) without admitting a per-file regime.
const MAX_FULL_PASSES: u64 = 2;

/// Headline-level ladder distilled from Martin's vault: shallow files with a
/// tail to level 4. Every step rises by at most one level, so the emitted
/// outline is valid org.
const LEVEL_LADDER: &[usize] = &[1, 2, 3, 2, 2, 3, 4, 3, 2, 1, 2, 3, 3, 2, 3];

/// Documents in the synthesized vault. The default is a fast sweep; the vault
/// rung (121 files / 2621 headlines, measured on Martin's vault 2026-09-08) is
/// asked for by env.
fn docs() -> usize {
    env_usize("HOLON_VAULT_SCALE_DOCS", 12)
}

/// Headlines per document. 22 is the measured mean of Martin's vault.
fn headlines_per_doc() -> usize {
    env_usize("HOLON_VAULT_SCALE_HEADLINES", 22)
}

/// Interactions timed per rung. Enough that a p50 is not one sample.
fn samples() -> usize {
    env_usize("HOLON_VAULT_SCALE_SAMPLES", 20)
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
    format!("vs-d{doc:04}-h{headline:03}")
}

/// One org document: `n` `:ID:`-bearing headlines over the depth ladder, each
/// carrying a body line so the block has content to re-write.
fn synthesized_document(doc: usize, n: usize) -> String {
    let mut out = String::with_capacity(n * 160);
    out.push_str(&format!(
        "#+TITLE: Scale Doc {doc}\n#+ID: vs-doc-{doc:04}\n"
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

fn synthesized_vault(docs: usize, per_doc: usize) -> Vec<(String, String)> {
    (0..docs)
        .map(|d| {
            (
                format!("Scale/Doc{d:04}.org"),
                synthesized_document(d, per_doc),
            )
        })
        .collect()
}

/// The corpus and, when it is synthesized, the ids this test may edit. Reading
/// a directory yields no editable ids: the measurement mode reports boot and
/// navigate only, because it cannot know which headlines carry an `:ID:`.
fn corpus() -> (Vec<(String, String)>, Vec<String>) {
    match std::env::var("HOLON_LATENCY_VAULT_DIR") {
        Ok(dir) => (read_vault_dir(std::path::Path::new(&dir)), Vec::new()),
        Err(std::env::VarError::NotPresent) => {
            let (d, h) = (docs(), headlines_per_doc());
            let ids = (0..d).flat_map(|doc| (0..h).map(move |i| block_id(doc, i)));
            (synthesized_vault(d, h), ids.collect())
        }
        Err(e) => panic!("HOLON_LATENCY_VAULT_DIR unreadable: {e}"),
    }
}

fn read_vault_dir(root: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {} : {e}", dir.display()))
            .flatten()
        {
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "org") {
                let rel = path
                    .strip_prefix(root)
                    .expect("walked path is under root")
                    .to_string_lossy()
                    .into_owned();
                out.push((
                    rel,
                    std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("read {} : {e}", path.display())),
                ));
            }
        }
    }
    assert!(
        !out.is_empty(),
        "HOLON_LATENCY_VAULT_DIR={} holds no .org files",
        root.display()
    );
    out.sort();
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

/// Dispatch one keystroke-shaped edit and return the wall time until the new
/// content is readable in the projection sink. 1 ms polling: a 50 ms poll
/// would quantize a 200 ms budget into four buckets.
async fn set_field_visible(env: &TestEnvironment, id: &str, content: &str) -> Duration {
    // ALLOW(entity_uri_from_raw): test-supplied bare org id entering the read-back
    // query; `block_raw` stores the schemed form.
    let schemed = EntityUri::from_raw(id).to_string();
    let sql = format!("SELECT content FROM block_raw WHERE id = '{schemed}'");
    let t0 = Instant::now();
    env.update_block_content(id, content)
        .await
        .unwrap_or_else(|e| panic!("set_field on {id}: {e}"));
    loop {
        let rows = env.query_sql(&sql).await.expect("read back edited block");
        if rows
            .iter()
            .any(|r| r.get("content").and_then(|v| v.as_string()) == Some(content))
        {
            return t0.elapsed();
        }
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "edit to {id} never reached block_raw within 60s — the projection is wedged, not slow"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// A cold page switch: bind a UI watch on a document root and wait for the
/// first structure it delivers.
async fn navigate_visible(env: &TestEnvironment, doc_uri: &EntityUri) -> Duration {
    let t0 = Instant::now();
    env.watch_ui_first_structure(doc_uri)
        .await
        .unwrap_or_else(|e| panic!("navigate to {doc_uri}: {e}"));
    t0.elapsed()
}

/// Linear-interpolated percentile, matching `scripts/measure_latency.py::pct`
/// so a number printed here is comparable to one printed there.
fn pct(sorted: &[Duration], p: f64) -> f64 {
    assert!(!sorted.is_empty(), "no samples to take a percentile of");
    let ms: Vec<f64> = sorted.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    if ms.len() == 1 {
        return ms[0];
    }
    let rank = p * (ms.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    ms[lo] + (ms[hi] - ms[lo]) * (rank - lo as f64)
}

/// Print one rung's distribution and hand back its p50 for the SLO-gap line.
fn report(action: &str, mut samples: Vec<Duration>) -> f64 {
    samples.sort();
    let (p50, p95) = (pct(&samples, 0.50), pct(&samples, 0.95));
    let max = pct(&samples, 1.0);
    eprintln!(
        "[vault-scale] {action:<10} n={:<3} p50={p50:8.1} p95={p95:8.1} max={max:8.1}  ms \
         (SLO {}ms)",
        samples.len(),
        SLO.as_millis(),
    );
    p50
}

#[test]
fn a_keystroke_and_a_page_switch_stay_inside_the_budget_at_vault_scale() {
    let (files, editable) = corpus();
    let file_count = files.len();
    let n = samples();

    // Installs the RUST_LOG-driven subscriber, so `holon_latency` stage events
    // reach the test output and `scripts/measure_latency.py` can parse them.
    holon_integration_tests::test_tracing::SpanCollector::global();

    let rt = runtime();
    rt.clone().block_on(async move {
        let mut builder = TestEnvironmentBuilder::new();
        for (name, content) in files {
            builder = builder.with_org_file(name, content);
        }

        let t_boot = Instant::now();
        let env = builder.build(rt.clone()).await.expect("boot scale vault");
        let blocks = block_count(&env).await;
        eprintln!(
            "[vault-scale] boot: {file_count} files / {blocks} blocks in {:?}",
            t_boot.elapsed()
        );
        assert!(
            blocks > file_count,
            "only {blocks} blocks landed from {file_count} files — the corpus never ingested, so \
             any latency measured below would be at the wrong scale"
        );

        let stats = holon_loro::loro_sync_controller::projection_stats::snapshot();
        eprintln!(
            "[vault-scale] projection passes: {} total, {} full ({} ops, {}ms in snapshots)",
            stats.passes, stats.full_passes, stats.ops, stats.snapshot_ms,
        );
        assert!(
            stats.full_passes <= MAX_FULL_PASSES,
            "boot took {} full-document reseed walks for {file_count} files. A cold boot needs \
             ONE; a walk per file is quadratic in vault size, because each walks the whole \
             ACCUMULATED tree. The incremental fast path is fed by the Loro `subscribe_root` \
             queue, so this count going up means the scan's flushes are again finding that queue \
             empty.",
            stats.full_passes,
        );

        // Steady state: every rung below measures a warm pipeline, not the
        // tail of boot's reseed.
        env.wait_for_loro_quiescence(Duration::from_secs(120)).await;
        env.wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;

        // Targets come from the INGESTED page rows, not from file names: a
        // vault file whose name holds a space percent-encodes into an
        // `EntityUri` that `resolve_page_uri` cannot look up, so name-derived
        // targets work only for a corpus this file generated.
        let pages: Vec<EntityUri> = env
            .query_sql("SELECT block_id FROM block_tags WHERE tag = 'Page' ORDER BY block_id")
            .await
            .expect("list ingested pages")
            .iter()
            .filter_map(|r| r.get("block_id").and_then(|v| v.as_string()))
            // ALLOW(entity_uri_from_raw): `block_tags.block_id` is already
            // schemed; `from_raw` is idempotent for schemed input.
            .map(EntityUri::from_raw)
            .collect();
        // One sample per page: a second navigate to the same page is a WARM
        // switch and measures something else, so a small corpus yields fewer
        // than `n` samples rather than repeating one.
        const MIN_NAVIGATE_SAMPLES: usize = 8;
        assert!(
            pages.len() >= MIN_NAVIGATE_SAMPLES,
            "only {} page(s) ingested from {file_count} files — too few for a median",
            pages.len(),
        );
        let mut navigates = Vec::new();
        for page in pages.iter().take(n) {
            navigates.push(navigate_visible(&env, page).await);
        }
        let nav_p50 = report("navigate", navigates);

        if editable.is_empty() {
            eprintln!(
                "[vault-scale] set_field skipped — HOLON_LATENCY_VAULT_DIR corpora carry no \
                 test-known block ids"
            );
            return;
        }

        // Spread the edits across documents: editing one block N times would
        // measure a single hot subtree, and the defect under test is about
        // cost scaling with the WHOLE vault.
        let stride = (editable.len() / n).max(1);
        let mut edits = Vec::with_capacity(n);
        for (k, id) in editable.iter().step_by(stride).take(n).enumerate() {
            edits.push(set_field_visible(&env, id, &format!("edited {k}")).await);
        }
        let edit_p50 = report("set_field", edits);

        let slo_ms = SLO.as_millis() as f64;
        eprintln!(
            "[vault-scale] SLO gap: navigate p50 {:.2}x, set_field p50 {:.2}x of {slo_ms}ms",
            nav_p50 / slo_ms,
            edit_p50 / slo_ms,
        );
    });
}
