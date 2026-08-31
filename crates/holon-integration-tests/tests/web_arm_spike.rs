//! Web-arm increment 1 spike: proves `WebUserDriver`'s browser transport by
//! driving the served dioxus-web app with real clicks and keystrokes, and
//! measures the per-op latency the design's risk section keys off.
//!
//! Settles on the relay oracle like every other rung. It ran on the DOM settle
//! window alone until 2026-09-01 and reddened about one run in four with
//! `enter did not split the block`: the worker advances on a 16ms tick pump, so
//! the DOM sits unchanged mid-flight and a quiet window is not evidence the
//! gesture landed (WebArmPBT.md §4a).
//!
//! Not a keystone replay: a `hand-authored-regressions/keystone.jsonl` case is
//! a `Fixture<E2ETransition>` over the keystone's reference state and wiring
//! manifest, which needs the SUT construction increment 2 brings. This is the
//! hand-rolled substitute the spike brief allows — every gesture asserts its
//! effect.
//!
//! Run against a live server:
//!   node frontends/dioxus-web/serve.mjs   # PORT=8791
//!   cargo test -p holon-integration-tests --features web-arm \
//!     --test web_arm_spike -- --ignored --nocapture
#![cfg(feature = "web-arm")]

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::KeyChord;
use holon_frontend::user_driver::UserDriver;
use holon_integration_tests::web_relay_oracle::WebRelayOracle;
use holon_integration_tests::web_relay_oracle::hub_url;
use holon_integration_tests::web_user_driver::RenderedNode;
use holon_integration_tests::web_user_driver::WebUserDriver;

/// Token typed into case 1 and required absent from case 2. Must not occur in
/// any seeded content, and must be typeable as bare US-keyboard keystrokes.
const SENTINEL: &str = "ZQXJV7WKF4";

fn app_url() -> String {
    std::env::var("HOLON_WEB_URL").unwrap_or_else(|_| "http://127.0.0.1:8791/".to_string())
}

fn headless() -> bool {
    std::env::var("HOLON_WEB_HEADED").is_err()
}

/// Records one op's wall time so the run can report p50/p95.
struct Latencies {
    /// (label, wall time incl. the settle wait, gesture→last DOM change)
    samples: Vec<(String, Duration, Duration)>,
}

impl Latencies {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    async fn time<F, T>(&mut self, driver: &WebUserDriver, label: &str, f: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let start = Instant::now();
        let out = f.await;
        self.samples
            .push((label.to_string(), start.elapsed(), driver.last_effect()));
        out
    }

    fn report(&self) -> String {
        let quantiles = |mut v: Vec<u128>| {
            v.sort_unstable();
            let pick = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
            format!(
                "p50={}ms p95={}ms min={}ms max={}ms",
                pick(0.5),
                pick(0.95),
                v[0],
                v[v.len() - 1]
            )
        };
        let mut out = format!(
            "ops={}\n  wall (incl. the settle wait): {}\n  effect (gesture → last DOM \
             change):  {}\n",
            self.samples.len(),
            quantiles(self.samples.iter().map(|s| s.1.as_millis()).collect()),
            quantiles(self.samples.iter().map(|s| s.2.as_millis()).collect()),
        );
        for (label, wall, effect) in &self.samples {
            out.push_str(&format!(
                "  {label}: wall {}ms, effect {}ms\n",
                wall.as_millis(),
                effect.as_millis()
            ));
        }
        out
    }
}

/// Sidebar rows render their bullet glyph inside the same node as the label
/// ("·notebook\nWelcome"), so labels are matched by containment.
fn find_by_text<'a>(nodes: &'a [RenderedNode], role: &str, text: &str) -> Option<&'a RenderedNode> {
    nodes
        .iter()
        .find(|n| n.role == role && n.text.contains(text))
}

/// The renderer emits the scheme-qualified id, so this parses it as-is.
fn uri(node: &RenderedNode) -> Result<EntityUri> {
    EntityUri::parse(&node.id)
        .with_context(|| format!("rendered node id {:?} is not an entity uri", node.id))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a served dioxus-web dist and a local Chrome"]
async fn web_arm_spike_drives_the_browser() -> Result<()> {
    let url = app_url();
    // ONE oracle for the whole test: the hub holds a single `role=browser`
    // socket, and the two contexts below are strictly sequential.
    let oracle = Arc::new(WebRelayOracle::start(&hub_url()));

    // ── Case reset 1: fresh browser context (OPFS empty) ────────────────────
    let reset_start = Instant::now();
    let driver = WebUserDriver::launch_with_oracle(&url, headless(), Arc::clone(&oracle)).await?;
    let boot = reset_start.elapsed();
    let seeded = driver.snapshot();
    println!("[web-arm] case reset (launch + fresh context + boot): {boot:?}");
    println!("[web-arm] seeded rendered set: {} nodes", seeded.len());
    for n in &seeded {
        println!("  [{}] {} = {:?}", n.role, n.id, n.text);
    }
    if seeded.is_empty() {
        bail!("app rendered no entity nodes — the wasm worker probably never booted");
    }

    let mut lat = Latencies::new();

    // ── Step 1: click a sidebar row (real click → navigation) ───────────────
    let journals = find_by_text(&seeded, "selectable", "Journals")
        .cloned()
        .context("no sidebar selectable labelled 'Journals' in the rendered set")?;
    let journals_uri = uri(&journals)?;
    lat.time(&driver, "click_entity(sidebar Journals)", async {
        driver.click_entity(&journals_uri, "sidebar").await
    })
    .await?;
    let after_nav = driver.body_text().await?;
    if !after_nav.contains("Journal") {
        bail!("clicking the Journals row did not bring journals content into view: {after_nav:?}");
    }
    println!("[web-arm] step 1 OK — sidebar click navigated");

    // ── Step 2: click back to Welcome, then focus a block's text ────────────
    let welcome = find_by_text(&driver.snapshot(), "selectable", "Welcome")
        .cloned()
        .context("no sidebar selectable labelled 'Welcome'")?;
    let welcome_uri = uri(&welcome)?;
    lat.time(&driver, "click_entity(sidebar Welcome)", async {
        driver.click_entity(&welcome_uri, "sidebar").await
    })
    .await?;

    let target = driver
        .snapshot()
        .into_iter()
        .find(|n| n.role == "rendered-text" && n.text.contains("Holon is now running"))
        .context("the seeded Welcome body block is not rendered")?;
    let target_uri = uri(&target)?;
    let before_text = driver
        .displayed_text(&target_uri)
        .context("target block has no displayed text")?;

    lat.time(&driver, "click_entity(block text → focus editor)", async {
        driver.click_entity(&target_uri, "main").await
    })
    .await?;
    println!("[web-arm] step 2 OK — focused {target_uri}");

    // ── Step 3: type real keystrokes, assert the text changed ───────────────
    lat.time(&driver, "send_raw_keystroke(end)", async {
        driver.send_raw_keystroke("end", &[]).await
    })
    .await?;
    for ch in ["X", "Y", "Z"] {
        lat.time(&driver, &format!("send_raw_keystroke({ch})"), async {
            driver.send_raw_keystroke(ch, &[]).await
        })
        .await?;
    }
    let after_typing = driver
        .displayed_text(&target_uri)
        .context("target block vanished after typing")?;
    if after_typing == before_text {
        bail!(
            "typing changed nothing — before {before_text:?}, after {after_typing:?}. The \
             keystrokes never reached the editor."
        );
    }
    println!("[web-arm] step 3 OK — {before_text:?} → {after_typing:?}");

    // ── Step 4: key chord (enter splits the block) ──────────────────────────
    // Count block bodies by entity, not by role: the split's new block mounts
    // as an editor-cell (it takes focus) while its sibling stays rendered-text.
    let body_ids = |nodes: &[RenderedNode]| {
        let mut ids: Vec<String> = nodes
            .iter()
            .filter(|n| n.role == "rendered-text" || n.role == "editor-cell")
            .map(|n| n.id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let blocks_before = body_ids(&driver.snapshot()).len();
    let chord = KeyChord::new(&[holon_api::Key::Enter]);
    let matched = lat
        .time(&driver, "send_key_chord(enter)", async {
            driver
                .send_key_chord(
                    &target_uri,
                    &Default::default(),
                    &target_uri,
                    &chord,
                    Default::default(),
                )
                .await
        })
        .await?;
    let blocks_after = body_ids(&driver.snapshot()).len();
    println!(
        "[web-arm] step 4 — chord matched={matched}, block bodies {blocks_before} → \
         {blocks_after}"
    );
    if blocks_after <= blocks_before {
        bail!("enter did not split the block: {blocks_before} → {blocks_after}");
    }

    // ── Step 4b: write a unique sentinel that must NOT survive the reset ────
    //
    // The reset check needs a token that (a) cannot occur in seeded content and
    // (b) is still intact in case 1's FINAL dom. Asserting on the post-typing
    // block text does not qualify: the split above cuts that text at the caret,
    // so the string is already gone from case 1 and its absence in case 2 would
    // prove nothing about OPFS.
    for ch in SENTINEL.chars() {
        lat.time(&driver, "send_raw_keystroke(sentinel)", async {
            driver.send_raw_keystroke(&ch.to_string(), &[]).await
        })
        .await?;
    }

    // ── Step 5: pad the sample to N≥30 with real ops ────────────────────────
    while lat.samples.len() < 30 {
        let n = lat.samples.len();
        lat.time(&driver, "send_raw_keystroke(a)", async {
            driver.send_raw_keystroke("a", &[]).await
        })
        .await?;
        if n % 5 == 0 {
            lat.time(&driver, "click_entity(block text)", async {
                driver.click_entity(&target_uri, "main").await
            })
            .await?;
        }
    }
    println!("[web-arm] latency\n{}", lat.report());

    // POSITIVE CONTROL: the sentinel must be in case 1's final dom, or the
    // absence check below is vacuous and would "pass" against a leaking reset.
    let case1_dom = driver.body_text().await?;
    if !case1_dom.contains(SENTINEL) {
        bail!(
            "sentinel {SENTINEL:?} is not in case 1's final dom — the reset check that follows \
             would be vacuous. Case 1 body text: {case1_dom:?}"
        );
    }
    println!("[web-arm] positive control OK — sentinel present in case 1's final dom");
    driver.close().await?;

    // ── Case reset 2: a second fresh context must not see case 1's edits ────
    let reset2 = Instant::now();
    let driver2 = WebUserDriver::launch_with_oracle(&url, headless(), Arc::clone(&oracle)).await?;
    let reset2_cost = reset2.elapsed();
    let fresh = driver2.snapshot();
    let case2_dom = driver2.body_text().await?;
    if case2_dom.contains(SENTINEL) || fresh.iter().any(|n| n.text.contains(SENTINEL)) {
        bail!(
            "OPFS was NOT clean on the second context — case 1's sentinel {SENTINEL:?} survived. \
             Per-case reset by browser context does not hold. Case 2 body text: {case2_dom:?}"
        );
    }
    let sidebar_of = |nodes: &[RenderedNode]| {
        let mut ids: Vec<String> = nodes
            .iter()
            .filter(|n| n.role == "selectable")
            .map(|n| n.id.clone())
            .collect();
        ids.sort();
        ids
    };
    if sidebar_of(&fresh) != sidebar_of(&seeded) {
        bail!(
            "second fresh context seeded a different sidebar ({:?} vs {:?}) — case reset is not \
             deterministic",
            sidebar_of(&fresh),
            sidebar_of(&seeded)
        );
    }
    println!("[web-arm] case reset 2 (fresh context, OPFS-clean asserted): {reset2_cost:?}");
    driver2.close().await?;
    Ok(())
}
