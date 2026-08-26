//! FIXTURE 3 for "an opened nested page paints zero children": a REAL booted
//! engine under a REAL gpui window.
//!
//! The two earlier fixtures could not decide the question. Fixture 1
//! (`nested_page_chevron_gate.rs`) hand-writes its `expand_toggle` DSL, so its
//! content leaf is a `text(...)` that trivially paints. Fixture 2
//! (`real_profile_embedded_page_probe`) loads the SHIPPED `embedded_page`
//! variant but resolves `render_entity()` through `StubBuilderServices`, which
//! hands back a canned `table_expr()` for every entity — a leaf that renders
//! nothing whether or not production is broken.
//!
//! This fixture removes the stub entirely: it boots the same
//! `FrontendSession` + `ReactiveEngine` the composed windowed harness boots
//! (`compose_sut_windowed_base_seeded`, the donor wiring in
//! `pbt_harness/windowed_wide.rs`), seeds a vault whose Host Page contains a
//! nested page with two children, attaches a real window over it, navigates
//! main focus to the Host Page, and clicks the nested page's trailing chevron
//! at its measured center. Every leaf therefore resolves through real block
//! data and real profile resolution.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test nested_page_real_engine
//! -- --test-threads=1 --nocapture`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon_api::EntityUri;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::SutFocusWrite;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

const GLYPH_COLLAPSED: &str = "\u{25B6}";
const GLYPH_EXPANDED: &str = "\u{25BC}";

const HOST_PAGE: &str = "real-host-page";
const NESTED_PAGE: &str = "real-nested-page";
const CHILD_A: &str = "buy milk";
const CHILD_B: &str = "see Journals now";

/// The live reproduction's shape: a Host Page with (a) one ordinary child and
/// (b) a nested page carrying TWO children of its own.
const HOST_ORG: &str = concat!(
    "#+ID: real-host-page\n",
    "* An ordinary host child\n",
    ":PROPERTIES:\n",
    ":ID: real-host-child\n",
    ":END:\n",
    "* A Nested Page :Page:\n",
    ":PROPERTIES:\n",
    ":ID: real-nested-page\n",
    ":END:\n",
    "** buy milk\n",
    ":PROPERTIES:\n",
    ":ID: real-nested-kid-a\n",
    ":END:\n",
    "** see Journals now\n",
    ":PROPERTIES:\n",
    ":ID: real-nested-kid-b\n",
    ":END:\n",
);

/// A second page so the sidebar has more than the host — mirrors the live
/// vault (the host is never the only page) without touching the assertion.
const OTHER_ORG: &str = "#+ID: structural-page\n* an unrelated note\n";

fn dump_painted(label: &str, bounds: &BoundsRegistry) {
    let mut els = bounds.all_elements();
    els.sort_by(|(_, a), (_, b)| {
        (a.y, a.x)
            .partial_cmp(&(b.y, b.x))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    eprintln!("── PAINTED SET {label} ({} elements) ──", els.len());
    for (id, info) in &els {
        eprintln!(
            "  {id} | {} | {:?} | x={:.1} y={:.1} w={:.1} h={:.1}",
            info.widget_type,
            info.displayed_text.as_deref().unwrap_or(""),
            info.x,
            info.y,
            info.width,
            info.height,
        );
    }
}

fn painted_texts(bounds: &BoundsRegistry) -> Vec<String> {
    bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, i)| i.displayed_text.map(|t| t.to_string()))
        .collect()
}

/// Count the rows `from descendants` yields for `nested` under a given query
/// context, so the report can say whether the content is lost BEFORE the
/// renderer (query returns nothing) or AFTER it (rows exist, leaf paints
/// nothing). `label` names which context shape is being probed.
fn probe_descendants(
    label: &str,
    qe: &Arc<dyn holon_api::QueryEngine>,
    runtime: &tokio::runtime::Runtime,
    ctx: holon_api::QueryContext,
) {
    use tokio_stream::StreamExt;

    let rows = runtime.block_on(async {
        let mut stream = match qe
            .watch_query(
                "from descendants",
                holon_api::QueryLanguage::HolonPrql,
                std::collections::HashMap::new(),
                Some(ctx),
                holon_api::render_requirements::RenderRequirements::none(),
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return Err(format!("{e:#}")),
        };
        let mut count = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let next = tokio::time::timeout_at(deadline, stream.next()).await;
            match next {
                Err(_) => break,
                Ok(None) => break,
                Ok(Some(batch)) => count += batch.inner.items.len(),
            }
        }
        Ok(count)
    });
    match rows {
        Ok(n) => eprintln!("PROBE [{label}]: 'from descendants' delivered {n} change(s)"),
        Err(e) => eprintln!("PROBE [{label}]: 'from descendants' FAILED: {e}"),
    }
}

fn chevron_glyph(bounds: &BoundsRegistry) -> Option<String> {
    bounds
        .element_info(&expand_toggle_id_for(NESTED_PAGE))
        .and_then(|i| i.displayed_text.map(|t| t.to_string()))
}

#[test]
fn a_real_engine_nested_page_paints_its_children_when_opened() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

    // The REAL booted stack: same builder the composed windowed harness uses,
    // seeded with OUR vault instead of the wide tree.
    let set = ComponentSet::full_headless();
    let seed_files = [
        ("real-host-page.org", HOST_ORG),
        ("structural-page.org", OTHER_ORG),
    ];
    let bundle = runtime.block_on(async {
        compose_sut_windowed_base_seeded(&set, &resolver, &seed_files, &[]).await
    });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let comp = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted HeadlessFrontendComponent");

    let host = EntityUri::block(HOST_PAGE);

    // Focus main on the Host Page BEFORE the window attaches (the donor's
    // ordering: window bring-up does not reset engine focus, so the first
    // rendered frame already paints the focused page). Production-shaped —
    // `apply_navigate_focus` CLICKS the sidebar entry through the headless
    // driver, which dispatches the entry's bound `navigation.focus`.
    runtime.block_on(async {
        comp.apply_navigate_focus(CapRegion::Main, &host).await;
    });

    let debug = Arc::new(holon_mcp::server::DebugServices::default());
    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug.clone()),
                None,
                "Holon-NestedPage-RealEngine",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    eprintln!(
        "engine focus after boot: {:?}",
        engine.focused_block().map(|b| b.to_string())
    );
    dump_painted("BEFORE CLICK", &bounds);
    eprintln!(
        "gate BEFORE: block_expanded_view({NESTED_PAGE}) = {:?}, chevron glyph = {:?}",
        engine.block_expanded_view(NESTED_PAGE),
        chevron_glyph(&bounds)
    );

    let toggle_id = expand_toggle_id_for(NESTED_PAGE);
    let info = bounds.element_info(&toggle_id).unwrap_or_else(|| {
        dump_painted("NO CHEVRON", &bounds);
        panic!("no expand toggle registered under {toggle_id}")
    });
    let (cx_f, cy_f) = info.center();
    let center = Point {
        x: Pixels::from(cx_f),
        y: Pixels::from(cy_f),
    };
    eprintln!(
        "chevron {toggle_id}: x={:.1} y={:.1} w={:.1} h={:.1} -> click ({cx_f:.1}, {cy_f:.1})",
        info.x, info.y, info.width, info.height
    );

    let window = rebind.window();
    app.update(|cx| {
        window
            .update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::MouseMoveEvent {
                        position: center,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseDownEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseUpEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .expect("window alive for the chevron click");
    });

    // Bounded settle: a real engine has to run the nested page's query and
    // project its rows. Break early once both children are on screen.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.run_until_parked();
        app.advance_clock(Duration::from_millis(200));
        app.run_until_parked();
        bounds.flush();
        let texts = painted_texts(&bounds);
        if [CHILD_A, CHILD_B]
            .iter()
            .all(|c| texts.iter().any(|t| t.contains(c)))
        {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    dump_painted("AFTER CLICK", &bounds);

    // Where is the content lost? Probe the SAME query the shipped
    // `embedded_page` content node runs, first with the context shape the gpui
    // `live_query` builder constructs, then with a path-resolved context.
    let nested = EntityUri::block(NESTED_PAGE);
    if let Some(qe) = engine.query_engine() {
        probe_descendants(
            "builder-shaped ctx (unfiltered path)",
            &qe,
            &runtime,
            holon_api::QueryContext::for_block(&nested, Some(host.clone())),
        );
        match runtime.block_on(async { qe.lookup_block_path(&nested).await }) {
            Ok(path) => {
                eprintln!("PROBE: lookup_block_path({nested}) = {path:?}");
                probe_descendants(
                    "path-resolved ctx",
                    &qe,
                    &runtime,
                    holon_api::QueryContext::for_block_with_path(&nested, Some(host.clone()), path),
                );
            }
            Err(e) => eprintln!("PROBE: lookup_block_path({nested}) FAILED: {e:#}"),
        }
    } else {
        eprintln!("PROBE: engine exposes no QueryEngine");
    }

    let glyph = chevron_glyph(&bounds);
    eprintln!(
        "gate AFTER: block_expanded_view({NESTED_PAGE}) = {:?}, chevron glyph = {:?} (collapsed \
         {GLYPH_COLLAPSED:?} / expanded {GLYPH_EXPANDED:?})",
        engine.block_expanded_view(NESTED_PAGE),
        glyph
    );

    let painted = painted_texts(&bounds);
    let missing: Vec<&str> = [CHILD_A, CHILD_B]
        .into_iter()
        .filter(|c| !painted.iter().any(|t| t.contains(c)))
        .collect();

    // Teardown BEFORE the assertion so a red does not also trip the gpui leak
    // detector / drop a runtime in an async context.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    assert!(
        missing.is_empty(),
        "an opened nested page must PAINT its children through a REAL engine: {missing:?} is not \
         among the painted text.\ngate = {:?}, chevron glyph = {glyph:?}\npainted = {painted:#?}",
        engine.block_expanded_view(NESTED_PAGE),
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
