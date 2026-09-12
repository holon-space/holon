//! Switching an integration on is a MEASURED interaction.
//!
//! The `user-connections` dogfood pass reported that twenty integration
//! `set_field` operations produced zero `holon_latency` events, and concluded
//! the surface is uninstrumented. Those twenty were driven through the MCP
//! server, which reaches `BackendEngine::execute_operation` directly — below
//! every frontend dispatch seam, so nothing opened an interaction clock. That
//! is a gap in what a programmatic driver can measure, not in the product.
//!
//! This rung settles the question the other way round, on the surface the SLO
//! is actually about: a real mouse click on the Settings switch. What it pins
//! is that the interaction OPENS a clock and that the clock CLOSES when the
//! mirror lands. A sample left open expires as `e2e_expired` — WARN spam and an
//! interaction the SLO cannot see, which is exactly the state the entry
//! described.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integration_toggle_latency_windowed -- --test-threads=1`
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
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::composed::slo_probe::SloProbe;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

const SETTINGS_GEAR: &str = "settings-gear";

/// The row whose switch is clicked, and the latency target its interaction
/// opens under — the `integration:` entity scheme the mirror keys on.
///
/// The FIRST row by the table's `provider_name ASC` order, because this rung
/// performs no pointer move and no scroll, so it can only reach what the
/// window mask shows at rest. A lower row's tracked box comes back 80x0 (the
/// panel's `max_h(720px)` inside a 900 px window), which is CLIPPING and not a
/// collapsed layout: `settings_integrations_last_row_toggle_windowed.rs`
/// scrolls — a `MouseMoveEvent` first, since gpui drops a wheel event without
/// one — and clicks the LAST row successfully. Nothing here is a product
/// limit; this rung is about latency and stays on the row it can reach without
/// a scroll.
const SUBJECT: &str = "claude-history";
const LATENCY_TARGET: &str = "integration:claude-history";

/// The "Enabled" column of `SETTINGS_ITEM_TEMPLATE`, 0-based. Held against the
/// header text below, so a reordered template fails here rather than clicking
/// the wrong cell.
const ENABLED_COLUMN: &str = "table-cell-col-3";
const ENABLED_HEADER: &str = "table-header-col-3";

/// The Settings table's "Enabled" cell for a row — the box the switch fills.
///
/// Addressed by the table's own cell id rather than by a switch id: the gpui
/// `state_toggle` builder registers its bounds under the generic node id
/// (`state_toggle#51`), which carries no row, so it cannot be aimed at.
fn enabled_cell_id(provider: &str) -> String {
    format!("{ENABLED_COLUMN}-integration:{provider}")
}

fn click_at(
    app: &mut HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    center: Point<Pixels>,
    what: &str,
) {
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
            .unwrap_or_else(|e| panic!("window alive for the {what} click: {e}"));
    });
}

fn center_of(info: &ElementInfo) -> Point<Pixels> {
    let (x, y) = info.center();
    Point {
        x: Pixels::from(x),
        y: Pixels::from(y),
    }
}

/// The `state_toggle` element whose box sits inside `cell`.
fn switch_inside(bounds: &BoundsRegistry, cell: &ElementInfo) -> Option<ElementInfo> {
    bounds
        .all_elements()
        .into_iter()
        .find(|(id, info)| {
            id.starts_with("state_toggle")
                && info.x >= cell.x - 1.0
                && info.x + info.width <= cell.x + cell.width + 1.0
                && info.y >= cell.y - 1.0
                && info.y + info.height <= cell.y + cell.height + 1.0
        })
        .map(|(_, info)| info)
}

fn registered_ids(bounds: &BoundsRegistry) -> Vec<String> {
    let mut ids: Vec<String> = bounds
        .all_elements()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids
}

fn mirror_enabled(
    runtime: &tokio::runtime::Runtime,
    db: &holon::storage::DbHandle,
    provider: &str,
) -> Option<i64> {
    runtime.block_on(async {
        db.query(
            "SELECT provider_name, enabled FROM integration_state",
            Default::default(),
        )
        .await
        .expect("read the mirror")
        .iter()
        .find(|r| r.get("provider_name").and_then(|v| v.as_string()) == Some(provider))
        .and_then(|r| r.get("enabled"))
        .and_then(|v| v.as_i64())
    })
}

#[test]
fn clicking_the_settings_switch_opens_and_closes_a_latency_interaction() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    // Point HOME at an empty dir before anything reads it: the bundled gcal
    // sidecar names `~/.config/holon/gcal-client-*`, and on a machine that has
    // those files a consent flow could reach a real browser.
    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
    // app boots and before any thread reads the environment.
    unsafe { std::env::set_var("HOME", home.path()) };

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let db = bundle
        .engine
        .as_ref()
        .expect("full_headless -> a Turso engine")
        .db_handle()
        .clone();

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
                None,
                None,
                "Holon-IntegrationToggleLatency-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let gear = bounds
        .element_info(SETTINGS_GEAR)
        .expect("the toolbar gear is the modal's only door");
    let window = rebind.window();
    click_at(&mut app, window, center_of(&gear), "gear");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let before = mirror_enabled(&runtime, &db, SUBJECT)
        .expect("the projector writes a row for every bundled provider");

    let header = bounds
        .element_info(ENABLED_HEADER)
        .expect("the settings table paints a header row");
    assert_eq!(
        header.displayed_text.as_deref(),
        Some("Enabled"),
        "column 3 must still be the switch column, or this rung clicks the wrong cell"
    );
    let cell = bounds
        .element_info(&enabled_cell_id(SUBJECT))
        .unwrap_or_else(|| {
            panic!(
                "the open Settings modal must paint an Enabled cell for '{SUBJECT}' as {:?}; \
                 registered ids: {:?}",
                enabled_cell_id(SUBJECT),
                registered_ids(&bounds)
            )
        });
    // The switch INSIDE that cell. The gpui `state_toggle` builder registers
    // its bounds under a row-less node id, so the row comes from the cell and
    // the clickable box from containment — the cell is wider than the switch
    // and its centre misses.
    let switch = switch_inside(&bounds, &cell).unwrap_or_else(|| {
        panic!(
            "the Enabled cell for '{SUBJECT}' must contain a `state_toggle` — the cell is at \
             ({}, {}) {}x{} and no state_toggle element sits inside it",
            cell.x, cell.y, cell.width, cell.height
        )
    });
    // Armed around the click alone: boot emits stages of its own, and a window
    // that included them would pass whether or not the click emitted anything.
    let probe = SloProbe::arm();

    // A few px INTO the box rather than at `center()`: a row at the mask edge
    // can record a height of 0, and its "centre" is then the row's top edge —
    // a click there lands between rows.
    click_at(
        &mut app,
        window,
        Point {
            x: Pixels::from(switch.x + switch.width / 2.0),
            y: Pixels::from(switch.y + 8.0),
        },
        "integration switch",
    );

    // Give the dispatch -> store write -> projector mirror chain time to land.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after = before;
    while Instant::now() < deadline {
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(2));
        after = mirror_enabled(&runtime, &db, SUBJECT).unwrap_or(before);
        if after != before {
            break;
        }
    }

    // Read the correlator BEFORE teardown: a dropped window cannot close a
    // sample, so reading after would measure the teardown.
    let still_pending = holon_api::latency_e2e::pending_targets()
        .iter()
        .any(|t| t == LATENCY_TARGET);
    let saw_dispatch = probe.saw_stage("dispatch", "set_field");
    let stages = probe.stage_samples();
    drop(probe);

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    assert_ne!(
        after, before,
        "clicking the switch must flip the mirror — without that the latency assertion below \
         would pass vacuously, on an interaction that never happened"
    );
    assert!(
        saw_dispatch,
        "the click must emit a `stage=\"dispatch\"` sample for `set_field` — without it the \
         interaction reports its total and nothing about where the time went, and \
         `scripts/measure_latency.py` shows this surface as uninstrumented. Stages seen while \
         armed: {stages:?}"
    );
    assert!(
        !still_pending,
        "the switch click's end-to-end latency sample for {LATENCY_TARGET:?} must CLOSE once the \
         projection lands (the mirror already flipped). Left open it expires as `e2e_expired`, \
         and the interaction is invisible to the SLO — which is the state the bugfunnel entry \
         `2026-09-12-switching-an-integration-emits-no-latency-stage…` describes"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
