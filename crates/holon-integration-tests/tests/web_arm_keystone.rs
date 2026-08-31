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
//! 3. [`web_arm_reset_vault_rebinds_the_live_page`] — swapping the worker's
//!    engine under a live page re-points what the page renders, and
//!    [`web_arm_superseded_bind_cannot_kill_the_rebound_page`] — a swap during
//!    a rebind leaves nothing behind that can later overwrite the live page.
//! 4. [`web_arm_replays_hand_authored_keystone_cases`] — the keystone corpus,
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
use holon_integration_tests::web_user_driver::BROWSER_TZ;
use holon_integration_tests::web_user_driver::BROWSER_UTC_OFFSET_SECONDS;
use holon_integration_tests::web_user_driver::RenderedNode;
use holon_integration_tests::web_user_driver::WebUserDriver;

/// `YYYY-MM-DD` anywhere in a block's content.
static DATE_LIKE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\d{4}-\d{2}-\d{2}").expect("valid regex"));

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

/// The corpus cases the web arm replays today. A ratchet, not a target: the
/// cap taxonomy ends in a wildcard over every transition variant, so a case
/// that stops being gesture-reachable would otherwise be absorbed as a
/// "declared cap" and the run would stay green having asserted less. Adding a
/// newly-replayable case here is welcome; removing one needs a comment saying
/// what became unreachable and why that is acceptable.
const REPLAYED_CASES: [&str; 9] = [
    "outdent-page-child-refusal-sqlonly",
    "split-at-caret-zero-then-enter-sqlonly",
    "caret-zero-split-reseats-the-caret-in-the-already-open-editor",
    "delete-backward-merges-previous-block-budget",
    "backspace-at-document-start-is-a-noop",
    "backspace-at-document-start-twice-stays-a-noop",
    "backspace-at-document-start-then-enter-splits-above",
    "split-then-type-caret-follows-typed-text",
    "split-then-two-backspaces-caret-follows-second-join",
];

/// The blocks the hand-authored corpus is authored over. They exist only in the
/// wide seed, which a browser can reach only through `reset_vault`.
const WIDE_SEED_PAGE: &str = "block:structural-page";
const WIDE_SEED_BLOCKS: [&str; 3] = ["block:parent", "block:c1", "block:c2"];

/// Boot a case onto the WIDE seed, the way the corpus expects to find the
/// vault.
///
/// `boot` alone opens the OPFS boot vault, which `seed_default_layout` fills —
/// it has no `block:parent`/`c1`/`c2`, so every corpus case was unaddressable
/// and the replay rung asserted nothing. Two steps fix that, and both are
/// load-bearing: `reset_vault` installs the wide seed, and the navigation
/// mounts its page, because a gesture can only reach a block the renderer has
/// actually mounted. The recipe is `web_arm_reset_vault_rebinds_the_live_page`,
/// which is also what pins the rebind this depends on.
async fn boot_wide_seed(oracle: &Arc<WebRelayOracle>) -> Result<WebUserDriver> {
    let driver = boot(oracle).await?;

    oracle
        .call("reset_vault", serde_json::json!({ "files": [] }))
        .await
        .context("reset_vault failed in the worker — the wide seed was never installed")?;

    // ENGINE CHANNEL first: a missing DOM row after a failed seed would read as
    // a binding bug, which is a different and much longer hunt.
    let engine = oracle.engine_snapshot().await?;
    for expected in std::iter::once(WIDE_SEED_PAGE).chain(WIDE_SEED_BLOCKS) {
        if !engine.block_ids.iter().any(|id| id == expected) {
            bail!(
                "reset_vault reported success but the engine does not hold {expected:?}. \
                 Engine holds: {:?}",
                engine.block_ids
            );
        }
    }

    // DOM CHANNEL: the page re-binds to the new engine on its own; poll rather
    // than await one signal so this does not depend on how that is implemented.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut rendered = driver.refresh_snapshot().await?;
    while Instant::now() < deadline && !rendered.iter().any(|n| n.id == WIDE_SEED_PAGE) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        rendered = driver.refresh_snapshot().await?;
    }
    if !rendered.iter().any(|n| n.id == WIDE_SEED_PAGE) {
        bail!(
            "the engine holds the wide seed but the page still renders the torn-down engine's \
             vault: {:?}",
            rendered.iter().map(|n| &n.id).collect::<Vec<_>>()
        );
    }

    let page_uri = EntityUri::parse(WIDE_SEED_PAGE)?;
    driver.click_entity(&page_uri, "sidebar").await?;

    let reachable: std::collections::BTreeSet<String> = driver
        .refresh_snapshot()
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect();
    let missing: Vec<&str> = WIDE_SEED_BLOCKS
        .into_iter()
        .filter(|id| !reachable.contains(*id))
        .collect();
    if !missing.is_empty() {
        bail!(
            "opening {WIDE_SEED_PAGE} renders none of {missing:?}, so the corpus's blocks are \
             still unaddressable. Rendered: {reachable:?}"
        );
    }
    Ok(driver)
}

fn find_by_text<'a>(nodes: &'a [RenderedNode], role: &str, text: &str) -> Option<&'a RenderedNode> {
    nodes
        .iter()
        .find(|n| n.role == role && n.text.contains(text))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_browser_runs_in_a_fixed_non_utc_zone() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let driver = boot(&oracle).await?;

    // `boot` already fails if TZ did not take — that only proves the browser
    // matches the DECLARED constant. This rung asserts the property the
    // constant exists for: the zone is not UTC-equivalent, so a date assertion
    // here cannot pass by coinciding with UTC. Setting both constants to UTC/0
    // is a one-line "simplification" that would otherwise leave every gate
    // green and restore the two-week mask.
    let offset = driver
        .evaluate_json("new Date().getTimezoneOffset()")
        .await?
        .as_i64()
        .context("getTimezoneOffset() did not return a number")?;
    if offset != -(i64::from(BROWSER_UTC_OFFSET_SECONDS) / 60) {
        bail!("browser reports getTimezoneOffset()={offset}, expected {BROWSER_TZ}");
    }

    // Strongest available evidence: a calendar date that ALREADY disagrees with
    // UTC's. The two share a date for part of every day even at +14, so when
    // they agree the offset carries the guarantee instead — never neither.
    let browser_day = driver
        .evaluate_json("new Date().toLocaleDateString('en-CA')")
        .await?
        .as_str()
        .context("toLocaleDateString did not return a string")?
        .to_string();
    let utc_day = chrono::Utc::now().format("%Y-%m-%d").to_string();

    if browser_day == utc_day {
        if offset == 0 {
            bail!(
                "the browser is at UTC (offset 0) and its calendar date {browser_day} is UTC's — \
                 this arm asserts dates, and in a UTC-equivalent zone those assertions pass \
                 whether or not the code under test honours a zone at all. BROWSER_TZ is \
                 {BROWSER_TZ:?}; it must name a zone that is not UTC-equivalent."
            );
        }
        println!(
            "[web-arm] browser zone OK — {BROWSER_TZ}, offset {offset}min; shares UTC's date \
             {browser_day} at this instant, discriminating at every other"
        );
    } else {
        println!(
            "[web-arm] browser zone OK — {BROWSER_TZ}: browser day {browser_day} vs UTC day \
             {utc_day} — date assertions here cannot coincide with UTC"
        );
    }

    driver.close().await?;
    Ok(())
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
    // The day page belongs to the VIEWER, so the oracle is the browser's
    // calendar date, not this process's. Reading the host zone here passed only
    // while the two happened to agree, which is the same vacuity the browser bug
    // hid behind.
    let today = (chrono::Utc::now()
        + chrono::Duration::seconds(i64::from(BROWSER_UTC_OFFSET_SECONDS)))
    .format("%Y-%m-%d")
    .to_string();
    let engine = oracle.engine_snapshot().await?;
    let day_blocks: Vec<(&String, &String)> = engine
        .block_content
        .iter()
        .filter(|(_, content)| content.contains(&today))
        .collect();
    if day_blocks.is_empty() {
        // A rule that fired on the WRONG date looks identical to one that never
        // fired if the message only reports the date it looked for. Naming the
        // dated blocks that DO exist separates a dead engine from a mis-zoned
        // one — the distinction this arm's fixed zone exists to expose.
        let dated: Vec<&String> = engine
            .block_content
            .values()
            .filter(|c| DATE_LIKE.is_match(c))
            .collect();
        bail!(
            "the browser engine holds NO block carrying the viewer's date ({today}, {BROWSER_TZ}) \
             after opening Journals. Date-shaped blocks it DOES hold: {dated:?} — if that names \
             another date the rule fired in the wrong zone; if it is empty the rule never fired. \
             Engine holds {} blocks: {:?}",
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

/// `reset_vault` swaps the worker's engine; the live page must follow it.
///
/// The tool tears the old engine down and rebuilds on a fresh in-memory DB, so
/// the page's `watch_view` subscription dies with the engine that owned it.
/// Unless the page re-subscribes against the new one it renders the torn-down
/// vault forever, and the wide seed the keystone corpus is authored over never
/// becomes gesture-reachable.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_reset_vault_rebinds_the_live_page() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let driver = boot(&oracle).await?;

    let before: std::collections::BTreeSet<String> =
        driver.snapshot().into_iter().map(|n| n.id).collect();
    if before.contains("block:structural-page") {
        bail!(
            "the boot vault already renders block:structural-page, so this test cannot tell a \
             rebind from a no-op. Rendered: {before:?}"
        );
    }

    let reset = oracle
        .call("reset_vault", serde_json::json!({ "files": [] }))
        .await
        .context("reset_vault failed in the worker")?;
    println!("[web-arm] reset_vault: {reset}");

    // ENGINE CHANNEL first: unless the worker really did rebuild and seed, a
    // missing DOM row is the tool's failure, not the page's.
    let engine = oracle.engine_snapshot().await?;
    for expected in [
        "block:structural-page",
        "block:parent",
        "block:c1",
        "block:c2",
    ] {
        if !engine.block_ids.iter().any(|id| id == expected) {
            bail!(
                "reset_vault reported success but the engine does not hold {expected:?} — the \
                 rebuild/seed is broken, not the page binding. Engine holds: {:?}",
                engine.block_ids
            );
        }
    }

    // DOM CHANNEL: the page must re-subscribe and render the NEW vault. Polled
    // rather than awaited on one signal so the assertion does not depend on how
    // the rebind is implemented.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut rendered = driver.refresh_snapshot().await?;
    while Instant::now() < deadline && !rendered.iter().any(|n| n.id == "block:structural-page") {
        tokio::time::sleep(Duration::from_millis(200)).await;
        rendered = driver.refresh_snapshot().await?;
    }
    let rendered_ids: std::collections::BTreeSet<String> =
        rendered.iter().map(|n| n.id.clone()).collect();
    if !rendered_ids.contains("block:structural-page") {
        bail!(
            "the engine was rebuilt and seeded ({} blocks incl. block:structural-page) but the \
             page still renders the torn-down engine's vault: {rendered_ids:?} (before the reset: \
             {before:?}). Nothing rebound the page's subscription to the new engine.",
            engine.block_ids.len()
        );
    }

    // The reason the rebind matters: the seeded blocks become gesture-reachable.
    let page_uri = EntityUri::parse("block:structural-page")?;
    driver.click_entity(&page_uri, "sidebar").await?;
    let reachable: std::collections::BTreeSet<String> = driver
        .refresh_snapshot()
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect();
    let missing: Vec<&str> = ["block:parent", "block:c1", "block:c2"]
        .into_iter()
        .filter(|id| !reachable.contains(*id))
        .collect();
    if !missing.is_empty() {
        bail!(
            "opening the seeded page renders none of {missing:?} — the corpus's blocks stay \
             unaddressable. Rendered: {reachable:?}"
        );
    }

    println!("[web-arm] rebind OK — seeded vault rendered and block:parent/c1/c2 addressable");
    driver.close().await?;
    Ok(())
}

/// The page's own boot watchdog, mirrored from `WATCH_READY_TIMEOUT_MS` in
/// frontends/dioxus-web/src/main.rs. A superseded bind's watchdog fires this
/// long after it armed, so a run that ends sooner cannot see it fire.
const PAGE_WATCH_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Reset twice, then idle past the page's boot watchdog, and require the page
/// to be alive and bound to the newest engine.
///
/// `spacing` between the resets is the whole experiment: at zero the second
/// reset lands while the first rebind is still waiting for its first
/// projection, which is what leaves a superseded bind's continuations running.
async fn assert_survives_two_resets(
    oracle: &Arc<WebRelayOracle>,
    driver: &WebUserDriver,
    spacing: Duration,
) -> Result<()> {
    for i in 1..=2 {
        oracle
            .call("reset_vault", serde_json::json!({ "files": [] }))
            .await
            .with_context(|| format!("reset_vault #{i} failed in the worker"))?;
        if i == 1 {
            tokio::time::sleep(spacing).await;
        }
    }

    tokio::time::sleep(PAGE_WATCH_READY_TIMEOUT + Duration::from_secs(3)).await;

    let state = driver.boot_state().await?;
    let rendered = driver.refresh_snapshot().await?;
    if state != "ready" || rendered.is_empty() {
        let engine = oracle.engine_snapshot().await?;
        bail!(
            "after two resets {spacing:?} apart the page is boot-state {state:?} with {} rendered \
             nodes, while the engine is healthy and holds {} blocks. A bind the page has moved \
             past is still writing to it. Page text: {:?}",
            rendered.len(),
            engine.block_ids.len(),
            driver.body_text().await.unwrap_or_default() /* ALLOW(ok): best-effort diagnostic
                                                          * enrichment inside a bail! that is
                                                          * already failing loudly */
        );
    }
    Ok(())
}

/// Back-to-back resets must not leave a superseded bind able to kill the page.
///
/// RED-FIRST TARGET. Each `bind_root_view` arms a watchdog that fails the boot
/// if no projection arrives. Reset the vault again before a rebind's first
/// envelope and that bind never delivers — so unless its continuations are
/// inert once superseded, its watchdog fires ten seconds later and replaces a
/// healthy, rebound page with the B3 recovery card.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_superseded_bind_cannot_kill_the_rebound_page() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let driver = boot(&oracle).await?;
    assert_survives_two_resets(&oracle, &driver, Duration::ZERO).await?;
    println!("[web-arm] overlapping resets OK — page still ready past the watchdog window");
    driver.close().await?;
    Ok(())
}

/// The same two resets, spaced far enough apart that each rebind completes
/// before the next reset. Pairs with
/// [`web_arm_superseded_bind_cannot_kill_the_rebound_page`] to show the overlap
/// is what matters, not the number of resets.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_spaced_resets_keep_the_page_alive() -> Result<()> {
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));
    let driver = boot(&oracle).await?;
    assert_survives_two_resets(&oracle, &driver, Duration::from_millis(300)).await?;
    println!("[web-arm] spaced resets OK");
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
    let mut replayed: Vec<String> = Vec::new();
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
        // a case must never inherit the previous one's OPFS — then onto the
        // wide seed the corpus is authored over.
        let driver = boot_wide_seed(&oracle).await?;
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
        replayed.push(case.name.clone());
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

    // RATCHET. The cap taxonomy's last arm is a wildcard over every transition
    // variant, present and future, so a case that stops being replayable is
    // absorbed as a declared cap and the run stays green with less coverage
    // than before. The wildcard stays — it is honest about what the browser can
    // realize — and this list is what makes losing a case loud.
    let missing: Vec<&str> = REPLAYED_CASES
        .into_iter()
        .filter(|name| !replayed.iter().any(|r| r == name))
        .collect();
    if !missing.is_empty() {
        bail!(
            "case(s) {missing:?} left the replay set — either restore them, or update \
             REPLAYED_CASES with a comment saying what stopped being reachable and why that is \
             acceptable. Replayed this run: {replayed:?}"
        );
    }
    assert!(
        replayed.len() >= REPLAYED_CASES.len(),
        "replayed {} cases but the pinned floor is {} — coverage went backwards",
        replayed.len(),
        REPLAYED_CASES.len()
    );

    if replayed.is_empty() {
        // DISCLOSED DEGRADED RUN. Loud on purpose: this test currently locks
        // the loader reuse and the cap taxonomy, and asserts NO application
        // behaviour. It starts asserting the moment the blocker below lifts,
        // because the replay loop above is live — nothing is stubbed out.
        eprintln!(
            "\n[web-arm] ################ ZERO CASES REPLAYED ################\n\
             [web-arm] The arm asserted NO application behaviour this run.\n\
             [web-arm] BLOCKER: the corpus is authored over the wide seed\n\
             [web-arm]   (block:parent / c1 / c2), which the browser has only\n\
             [web-arm]   behind the `reset_vault` tool, under a page this boot\n\
             [web-arm]   never opens. `boot` must reset the vault and then\n\
             [web-arm]   navigate to block:structural-page before any case is\n\
             [web-arm]   gesture-reachable — the recipe is in\n\
             [web-arm]   web_arm_reset_vault_rebinds_the_live_page.\n\
             [web-arm] #####################################################\n"
        );
    }
    assert!(
        !capped.is_empty() || !replayed.is_empty(),
        "the corpus loaded but produced neither a replay nor a cap — the loader returned nothing"
    );
    println!(
        "[web-arm] replayed {} of {} cases",
        replayed.len(),
        cases.len()
    );
    Ok(())
}
