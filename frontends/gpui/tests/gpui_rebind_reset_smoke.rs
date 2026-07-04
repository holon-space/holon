//! Step 0 (Phase 1 Option A): desktop double-rebind smoke.
//!
//! Proves `RebindHandle::rebind` — which has NO in-tree caller today — actually
//! re-points a live GPUI window at a freshly-built engine. Boots SUT#1 via
//! `holon_gpui::reset::build_fresh_sut`, opens a rebindable window over it,
//! settles, then builds SUT#2 (a DISTINCT seed) and rebinds the SAME window onto
//! it, asserting the rendered tree switches from SUT#1's content to SUT#2's.
//!
//! ⚠ `--test-threads=1` (gpui is not parallel-safe).

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AssetSource, PlatformTextSystem, TestApp};
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::reset::{build_fresh_sut, FreshSut};

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Pump until the element count is stable and no `"loading"` placeholders remain.
fn settle_to_fixed_point(
    app: &mut TestApp,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let elements = bounds.all_elements();
        let count = elements.len();
        let still_loading = elements
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                return;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    panic!(
        "window never reached a fixed point within {timeout:?}: {} elements",
        bounds.all_elements().len()
    );
}

/// Materialize seed .org files into a fresh temp org root + return fresh db/config paths.
fn materialize_seed(
    files: &[(&str, &str)],
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let org_root = std::fs::canonicalize(temp.path()).expect("canonicalize temp");
    for (name, content) in files {
        std::fs::write(org_root.join(name), content).expect("write seed file");
    }
    let db_path = org_root.join("holon.db");
    let config_dir = org_root.clone();
    (temp, db_path, org_root, config_dir)
}

/// The set of on-screen text strings across every tracked widget (the left
/// sidebar renders each Page's `content`, which for a page equals its source
/// filename stem — so distinct seed filenames give distinct, deterministic
/// sidebar entries).
fn displayed_texts(bounds: &BoundsRegistry) -> std::collections::BTreeSet<String> {
    bounds
        .all_elements()
        .iter()
        .filter_map(|(_, i)| i.displayed_text.as_deref().map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect()
}

const INDEX_ORG: &str = include_str!("../../../assets/default/index.org");
const JOURNALS_ORG: &str = include_str!("../../../assets/default/Journals.org");

#[test]
fn rebind_reset_smoke() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));

    // SUT#1: default shell + a page whose filename stem ("alphapage") is a
    // unique sidebar sentinel (the left sidebar renders a Page's content, which
    // for a page equals its source filename stem).
    let page_body = "* heading\n";
    let (_t1, db1, root1, cfg1) = materialize_seed(&[
        ("index.org", INDEX_ORG),
        ("Journals.org", JOURNALS_ORG),
        ("alphapage.org", page_body),
    ]);
    let sut1: FreshSut = runtime
        .block_on(build_fresh_sut(
            db1,
            root1,
            cfg1,
            Duration::from_millis(500),
        ))
        .expect("build SUT#1");

    let debug = Arc::new(holon_mcp::server::DebugServices::default());
    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                sut1.session.clone(),
                sut1.engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug.clone()),
                "Holon-Rebind-Smoke",
                cx,
            )
        })
        .expect("window opened over SUT#1");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
    let texts1 = displayed_texts(&bounds);
    eprintln!("SUT#1 sidebar texts: {texts1:?}");
    assert!(
        texts1.contains("alphapage"),
        "SUT#1 window should render its 'alphapage' Page in the sidebar; got {texts1:?}"
    );
    assert!(
        !texts1.contains("bravopage"),
        "SUT#1 must NOT show SUT#2's page before rebind; got {texts1:?}"
    );

    // SUT#2: default shell + a DIFFERENT page filename ("bravopage").
    let (_t2, db2, root2, cfg2) = materialize_seed(&[
        ("index.org", INDEX_ORG),
        ("Journals.org", JOURNALS_ORG),
        ("bravopage.org", page_body),
    ]);
    let sut2: FreshSut = runtime
        .block_on(build_fresh_sut(
            db2,
            root2,
            cfg2,
            Duration::from_millis(500),
        ))
        .expect("build SUT#2");

    // Rebind the SAME window onto SUT#2 — the crux: `rebind` has no in-tree
    // caller, so this is the first proof it re-points a live window's render.
    app.update(|cx| rebind.rebind(sut2.session.clone(), sut2.engine.clone(), cx));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
    let texts2 = displayed_texts(&bounds);
    eprintln!("SUT#2 sidebar texts: {texts2:?}");
    assert!(
        texts2.contains("bravopage"),
        "after rebind the window MUST render SUT#2's 'bravopage' Page; got {texts2:?}"
    );
    assert!(
        !texts2.contains("alphapage"),
        "after rebind SUT#1's 'alphapage' must be GONE (stale engine); got {texts2:?}"
    );

    // Teardown: leak like the windowed harness to avoid Drop hazards.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(sut1);
    std::mem::forget(sut2);
}
