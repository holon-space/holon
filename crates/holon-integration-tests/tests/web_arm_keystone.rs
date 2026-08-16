//! Web-arm increment 2: the dual oracle (design ruling D4.a) and the
//! hand-authored keystone replay, both against a real Chrome driving the
//! served dioxus-web app.
//!
//! Three tests, each proving one thing increment 1 could not:
//!
//! 1. [`web_arm_rule_engine_materializes_the_day_page`] — the browser's rule
//!    engine actually runs. This is the escape task #38 reported: the wasm
//!    worker's hand-rolled boot omitted `start_action_watchers`, the seeded
//!    `daily_journal` rule never fired, the journals feed stayed empty, and NO
//!    gate booted the stack deep enough to see it. Comment that call out of
//!    `frontends/holon-worker/src/lib.rs`, rebuild the worker wasm, and this
//!    goes red for exactly that reason.
//! 2. [`web_arm_dual_oracle_cross_checks_a_split`] — a real gesture, then both
//!    channels read and cross-checked (plus a third `block_raw` point).
//! 3. [`web_arm_replays_hand_authored_keystone_cases`] — the keystone corpus,
//!    loaded by the keystone's own loader, replayed as gestures.
//!
//! Run against a live server:
//!   (cd frontends/dioxus-web   && npm install)   # once — see below
//!   (cd frontends/holon-worker && npm install)   # once — see below
//!   node frontends/dioxus-web/serve.mjs          # PORT=8791
//!   cargo test -p holon-integration-tests --features web-arm,pbt \
//!     --test web_arm_keystone -- --ignored --nocapture --test-threads=1
//!
//! BOTH `npm install`s are required, and their absence does NOT say so. The
//! server resolves the worker's JS glue and its `@napi-rs`/`@emnapi` runtime
//! deps out of those two `node_modules` trees; without them the worker fails to
//! instantiate and EVERY test here dies at boot with the unnamed
//! `app boot ended in state "failed" — worker spawn: worker error`, which reads
//! like a broken driver and is not. Both trees are gitignored, so installing
//! them cannot contaminate a commit — a checkout simply arrives without them,
//! and the serve dependency is easy to miss until it fails this way.
//!
//! `--test-threads=1` is not optional either: the hub holds ONE `role=browser`
//! socket, so two cases sharing it would answer each other's tool calls.
#![cfg(all(feature = "web-arm", feature = "pbt"))]

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_frontend::user_driver::UserDriver;
use holon_integration_tests::pbt::hand_authored::load_cases;
use holon_integration_tests::web_arm;
use holon_integration_tests::web_relay_oracle::WebRelayOracle;
use holon_integration_tests::web_relay_oracle::hub_url;
use holon_integration_tests::web_user_driver::RenderedNode;
use holon_integration_tests::web_user_driver::WebUserDriver;

fn app_url() -> String {
    std::env::var("HOLON_WEB_URL").unwrap_or_else(|_| "http://127.0.0.1:8791/".to_string())
}

fn headless() -> bool {
    std::env::var("HOLON_WEB_HEADED").is_err()
}

/// Boot a case: fresh browser context (which is what empties OPFS) with the
/// relay oracle attached as the primary settled signal.
///
/// The oracle is passed in and outlives every case in the test — see
/// `WebRelayOracle::start` for why it is exactly one per test. Its liveness is
/// proven after the page boots, since a tool call only answers once the page is
/// on the hub as `role=browser`.
async fn boot(oracle: &Arc<WebRelayOracle>) -> Result<WebUserDriver> {
    WebUserDriver::launch_with_oracle(&app_url(), headless(), Arc::clone(oracle)).await
}

fn find_by_text<'a>(nodes: &'a [RenderedNode], role: &str, text: &str) -> Option<&'a RenderedNode> {
    nodes
        .iter()
        .find(|n| n.role == role && n.text.contains(text))
}

/// The rule engine runs in the browser: navigating to Journals materializes a
/// day page for today, in the engine AND in the DOM.
///
/// RED-FIRST TARGET. `start_action_watchers` in the wasm worker's boot is what
/// makes the seeded `daily_journal` rule fire. Without it the engine holds no
/// day page and the feed renders empty — which is precisely the escape that
/// reached production and is why this arm exists.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_rule_engine_materializes_the_day_page() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let driver = boot(&oracle).await?;

    let journals = find_by_text(&driver.snapshot(), "selectable", "Journals")
        .cloned()
        .context("no sidebar row labelled 'Journals' in the rendered set")?;
    let journals_uri = EntityUri::parse(&journals.id)?;
    driver.click_entity(&journals_uri, "sidebar").await?;

    // ENGINE CHANNEL — the authoritative read. A day page is a document block
    // titled with today's date; the rule mints it, nothing else does.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let engine = oracle.engine_snapshot().await?;
    let day_blocks: Vec<(&String, &String)> = engine
        .block_content
        .iter()
        .filter(|(_, content)| content.contains(&today))
        .collect();
    if day_blocks.is_empty() {
        bail!(
            "the browser engine holds NO block carrying today's date ({today}) after opening \
             Journals — the `daily_journal` rule never fired, so the rule engine is dead in this \
             build. Engine holds {} blocks: {:?}",
            engine.block_ids.len(),
            engine.block_content.values().collect::<Vec<_>>()
        );
    }

    // DOM CHANNEL — and the user can actually see it.
    let body = driver.body_text().await?;
    if !body.contains(&today) {
        bail!(
            "the engine minted a day page for {today} ({day_blocks:?}) but the journals feed does \
             not render it — a projection the user never sees. Page text: {body:?}"
        );
    }

    println!("[web-arm] rule engine OK — day page {today} present in engine and DOM");
    driver.close().await?;
    Ok(())
}

/// One real gesture, both channels read, cross-checked, plus the third
/// `block_raw` point. Exercises the differential the arm is for.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_dual_oracle_cross_checks_a_split() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let driver = boot(&oracle).await?;
    let before = web_arm::read_and_cross_check(&driver, "boot").await?;
    println!(
        "[web-arm] boot: {} engine blocks, {} block_raw rows, {} rendered nodes",
        before.engine.block_ids.len(),
        before.raw_block_rows,
        before.rendered.len()
    );

    let target = driver
        .snapshot()
        .into_iter()
        .find(|n| n.role == "rendered-text" && !n.text.trim().is_empty())
        .context("no non-empty rendered block to split")?;
    let target_uri = EntityUri::parse(&target.id)?;

    let started = Instant::now();
    driver.click_entity(&target_uri, "main").await?;
    driver.send_raw_keystroke("end", &[]).await?;
    driver.send_raw_keystroke("enter", &[]).await?;
    let split_wall = started.elapsed();

    let after = web_arm::read_and_cross_check(&driver, "after-split").await?;

    // The split must be visible in BOTH channels. Either one alone would pass
    // against a defect the other sees: an engine-only assertion misses a
    // renderer that never mounted the new row, a DOM-only assertion misses a
    // row rendered from stale view state with no block behind it.
    if after.engine.block_ids.len() <= before.engine.block_ids.len() {
        bail!(
            "enter did not create a block in the ENGINE: {} → {}",
            before.engine.block_ids.len(),
            after.engine.block_ids.len()
        );
    }
    let rendered_bodies = |nodes: &[RenderedNode]| {
        nodes
            .iter()
            .filter(|n| n.role == "rendered-text" || n.role == "editor-cell")
            .map(|n| n.id.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let dom_before = rendered_bodies(&before.rendered);
    let dom_after = rendered_bodies(&after.rendered);
    if dom_after.len() <= dom_before.len() {
        bail!(
            "the engine gained a block but the DOM did not render it: {:?} → {:?}",
            dom_before,
            dom_after
        );
    }
    if after.raw_block_rows <= before.raw_block_rows {
        bail!(
            "block_raw did not gain a row ({} → {}) although the block query did — the \
             projection reports a block its own storage never received",
            before.raw_block_rows,
            after.raw_block_rows
        );
    }

    println!(
        "[web-arm] dual oracle agreed on the split: engine {} → {}, DOM {} → {}, block_raw {} → \
         {} (wall {:?}, engine convergence {:?})",
        before.engine.block_ids.len(),
        after.engine.block_ids.len(),
        dom_before.len(),
        dom_after.len(),
        before.raw_block_rows,
        after.raw_block_rows,
        split_wall,
        driver.last_engine_wait(),
    );
    driver.close().await?;
    Ok(())
}

/// Replay the hand-authored keystone corpus in the browser.
///
/// Cases are loaded by the keystone's OWN loader, so the schema guards, the
/// name filters and the quarantine list all behave identically to the headless
/// replay. A case the DOM cannot express is reported with its reason (a
/// declared cap) rather than skipped in silence, and every replayed case gets
/// the dual-oracle cross-check after every transition.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_replays_hand_authored_keystone_cases() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let cases = load_cases();
    let mut replayed = 0usize;
    let mut capped: Vec<(String, String)> = Vec::new();
    let mut latencies: Vec<(String, Duration, Option<Duration>)> = Vec::new();

    for case in &cases {
        let caps = web_arm::case_caps(case);
        if let Some(reason) = caps.first() {
            capped.push((case.name.clone(), reason.clone()));
            continue;
        }
        println!(
            "[web-arm] replaying case {:?} ({} transitions)",
            case.name,
            case.transitions.len()
        );
        // Fresh context per case, exactly as the design's reset rule requires:
        // a case must never inherit the previous one's OPFS.
        let driver = boot(&oracle).await?;
        if let Some(reason) = web_arm::unaddressable(&driver, case) {
            capped.push((case.name.clone(), reason));
            driver.close().await?;
            continue;
        }
        web_arm::read_and_cross_check(&driver, &format!("{}:boot", case.name)).await?;
        for (i, transition) in case.transitions.iter().enumerate() {
            let started = Instant::now();
            web_arm::apply(&driver, transition)
                .await
                .with_context(|| format!("case {:?} transition {i} ({transition:?})", case.name))?;
            latencies.push((
                format!("{}:{i}", case.name),
                started.elapsed(),
                driver.last_engine_wait(),
            ));
            web_arm::read_and_cross_check(&driver, &format!("{}:after-{i}", case.name))
                .await
                .with_context(|| {
                    format!(
                        "case {:?} diverged after transition {i} ({transition:?})",
                        case.name
                    )
                })?;
        }
        driver.close().await?;
        replayed += 1;
        println!("[web-arm] PASSED case {:?}", case.name);
    }

    println!(
        "\n[web-arm] declared caps ({} cases not replayable):",
        capped.len()
    );
    for (name, reason) in &capped {
        println!("  {name}: {reason}");
    }
    let wall: Vec<u128> = latencies.iter().map(|l| l.1.as_millis()).collect();
    if !wall.is_empty() {
        let mut sorted = wall.clone();
        sorted.sort_unstable();
        let pick = |q: f64| sorted[((sorted.len() as f64 - 1.0) * q).round() as usize];
        println!(
            "\n[web-arm] per-transition wall over {} ops: p50={}ms p95={}ms max={}ms",
            sorted.len(),
            pick(0.5),
            pick(0.95),
            sorted[sorted.len() - 1]
        );
    }

    // Every cap must be one of the two KNOWN classes. A case that becomes
    // unreplayable for a new reason goes red here rather than joining a
    // growing pile of silent skips — that is what makes a zero-replay run
    // meaningful instead of vacuous.
    let unexplained: Vec<&(String, String)> = capped
        .iter()
        .filter(|(_, reason)| {
            !reason.contains("CreateBlockUnderFocus pins")
                && !reason.contains("has no pointer/keyboard realization")
                && !reason.contains("names block(s) the browser does not render")
        })
        .collect();
    if !unexplained.is_empty() {
        bail!(
            "case(s) capped for a reason outside the two declared classes — the web arm's cap \
             taxonomy has drifted and the caps are no longer tracked: {unexplained:#?}"
        );
    }

    if replayed == 0 {
        // DISCLOSED DEGRADED RUN. Loud on purpose: this test currently locks
        // the loader reuse and the cap taxonomy, and asserts NO application
        // behaviour. It starts asserting the moment the blocker below lifts,
        // because the replay loop above is live — nothing is stubbed out.
        eprintln!(
            "\n[web-arm] ################ ZERO CASES REPLAYED ################\n\
             [web-arm] The arm asserted NO application behaviour this run.\n\
             [web-arm] BLOCKER: the corpus is authored over the wide seed\n\
             [web-arm]   (block:parent / c1 / c2), which the browser has only\n\
             [web-arm]   behind the `reset_vault` tool. That tool rebuilds the\n\
             [web-arm]   worker's engine on a fresh in-memory DB but leaves the\n\
             [web-arm]   live page bound to the torn-down one — measured: after\n\
             [web-arm]   reset_vault the engine holds block:structural-page and\n\
             [web-arm]   the sidebar still shows only Welcome/Journals, through\n\
             [web-arm]   a 3s wait AND a navigation click. Until the page\n\
             [web-arm]   rebinds on reset, no keystone case is gesture-reachable.\n\
             [web-arm] #####################################################\n"
        );
    }
    assert!(
        !capped.is_empty() || replayed > 0,
        "the corpus loaded but produced neither a replay nor a cap — the loader returned nothing"
    );
    println!("[web-arm] replayed {replayed} of {} cases", cases.len());
    Ok(())
}
