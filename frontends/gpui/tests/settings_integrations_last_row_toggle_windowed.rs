//! The LAST row of the Settings → Integrations table is REACHABLE: a real
//! click on its `Enabled` switch flips that provider's `enabled` in the
//! `integration_state` mirror — judged in a REAL window over a REAL booted
//! engine.
//!
//! A verifier pass of 2026-09-12 reported that only the FIRST row of this
//! table can be clicked and that the modal does not scroll — which, if true,
//! would mean "switch it off in Settings" is not a remedy any connection but
//! the first one has. It is not true, and
//! `docs/Testing/bugfunnel/entries/
//! 2026-09-12-only-the-first-settings-integrations-row-can-be-clicked.md`
//! records the measurement that refutes it. This rung is what stands in that
//! report's place.
//!
//! Two facts it pins, neither of which any neighbouring rung asserts —
//! `settings_integrations_table_fits_windowed.rs` judges the table's
//! HORIZONTAL fit, and `settings_integrations_ops_windowed.rs` clicks one op
//! button on the `gcal` row, which is above the fold:
//!
//!   1. The table is taller than the modal at this window size, so the bottom
//!      rows START clipped — and the one gesture a user has for that, the wheel
//!      over the panel, reaches them within a bounded number of notches.
//!   2. A real click on the LAST row's switch then flips THAT provider's
//!      `enabled`, and only that one. Judged on state, not on geometry: a row
//!      can be positioned correctly and still not receive the click.
//!
//! The pointer move before the wheel is load-bearing, not ceremony. gpui gates
//! a div's wheel handling on the hitbox being hovered
//! (`Interactivity::paint` → `Hitbox::should_handle_scroll`), so a bare
//! `ScrollWheelEvent` is dropped in silence — which is exactly how the report
//! above concluded that a working modal could not scroll.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integrations_last_row_toggle_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::collections::HashMap;
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
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The modal has no command and no keybinding — the toolbar gear is its one
/// door.
const SETTINGS_GEAR: &str = "settings-gear";

/// The `table` renderer's contract ids (`render/builders/table.rs`).
const HEADER_PREFIX: &str = "table-header-col-";
const CELL_PREFIX: &str = "table-cell-col-";

/// The column that holds the switch (`integrations_section.rs`
/// `SETTINGS_ITEM_TEMPLATE`). Found by its header text rather than by index so
/// a column reorder retargets the test instead of silently judging the wrong
/// cell.
const ENABLED_HEADER: &str = "Enabled";

/// The widget tags the switch paints under its cell.
const TOGGLE_TAGS: &[&str] = &["state_toggle", "switch_track"];

/// The default desktop window the verifier pass ran at. Set before the window
/// opens; `launch_holon_window_impl` reads it.
const WINDOW: &str = "1512x900";
const WINDOW_W: f32 = 1512.0;
const WINDOW_H: f32 = 900.0;

/// Sub-pixel layout rounding.
const EPS: f32 = 1.0;

/// Where the wheel gesture is aimed: the middle of the window, which is inside
/// the centred modal panel and outside every other scrollable surface.
fn modal_center() -> Point<Pixels> {
    Point {
        x: Pixels::from(WINDOW_W / 2.0),
        y: Pixels::from(WINDOW_H / 2.0),
    }
}

/// One wheel notch, and the budget. The modal's content overhangs its 720px
/// panel by ~306px, so a handful of notches suffices; the cap is what turns
/// "the modal does not scroll" into a failure instead of a hang.
const WHEEL_DY: f32 = 60.0;
const MAX_NOTCHES: usize = 20;

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

/// One wheel notch at `at`, preceded by the pointer move that makes the hitbox
/// under it the one gpui offers the scroll to.
fn wheel(app: &mut HeadlessAppContext, window: gpui::AnyWindowHandle, at: Point<Pixels>, dy: f32) {
    app.update(|cx| {
        window
            .update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::ScrollWheelEvent {
                        position: at,
                        delta: gpui::ScrollDelta::Pixels(gpui::point(
                            Pixels::from(0.0),
                            Pixels::from(dy),
                        )),
                        modifiers: Default::default(),
                        touch_phase: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap_or_else(|e| panic!("window alive for the wheel gesture: {e}"));
    });
}

fn center_of(info: &ElementInfo) -> Point<Pixels> {
    let (x, y) = info.center();
    Point {
        x: Pixels::from(x),
        y: Pixels::from(y),
    }
}

fn bottom(info: &ElementInfo) -> f32 {
    info.y + info.height
}

/// Everything the window painted, keyed by contract id.
struct Painted {
    all: Vec<(String, ElementInfo)>,
    by_id: HashMap<String, ElementInfo>,
}

impl Painted {
    fn snapshot(bounds: &BoundsRegistry) -> Self {
        let all = bounds.all_elements();
        let by_id = all.iter().cloned().collect();
        Self { all, by_id }
    }

    /// `table-header-col-{k}` → column index.
    fn headers(&self) -> BTreeMap<usize, ElementInfo> {
        let mut out = BTreeMap::new();
        for (id, info) in &self.all {
            if let Some(rest) = id.strip_prefix(HEADER_PREFIX) {
                if let Ok(k) = rest.parse::<usize>() {
                    out.insert(k, info.clone());
                }
            }
        }
        out
    }

    /// Every `table-cell-col-{k}-{row}` of column `k`, keyed by row id.
    ///
    /// Deliberately UNFILTERED by visible area: a row collapsed to zero height
    /// is exactly the defect this rung judges, and dropping it would make the
    /// last painted row the last VISIBLE one — the test would then click a row
    /// that works and pass.
    fn cells_of_column(&self, k: usize) -> BTreeMap<String, (String, ElementInfo)> {
        let prefix = format!("{CELL_PREFIX}{k}-");
        let mut out = BTreeMap::new();
        for (id, info) in &self.all {
            if let Some(row) = id.strip_prefix(prefix.as_str()) {
                out.insert(row.to_string(), (id.clone(), info.clone()));
            }
        }
        out
    }

    /// The painted elements whose tracked-parent chain reaches `cell_id`.
    fn descendants_of(&self, cell_id: &str) -> Vec<(String, ElementInfo)> {
        self.all
            .iter()
            .filter(|(id, _)| id.as_str() != cell_id && self.is_under(id, cell_id))
            .cloned()
            .collect()
    }

    fn is_under(&self, id: &str, ancestor: &str) -> bool {
        let mut cursor = self.by_id.get(id).and_then(|i| i.parent_id.clone());
        // The tracked tree under one table cell is a handful of levels deep;
        // the bound is a cycle guard, not a real limit.
        for _ in 0..64 {
            let Some(current) = cursor else { return false };
            if current.as_ref() == ancestor {
                return true;
            }
            cursor = self
                .by_id
                .get(current.as_ref())
                .and_then(|i| i.parent_id.clone());
        }
        panic!("tracked-parent chain of {id:?} exceeded 64 levels — the registry has a cycle");
    }
}

/// The LAST row of column `k`: its row id, its cell, and the switch painted
/// under it (`None` when the cell painted no control of its own).
type LastSwitch = (String, ElementInfo, Option<(String, ElementInfo)>);

fn last_switch(painted: &Painted, k: usize) -> Option<LastSwitch> {
    let cells = painted.cells_of_column(k);
    let (row, (cell_id, cell)) = cells.iter().next_back()?;
    let switch = painted.descendants_of(cell_id).into_iter().find(|(_, i)| {
        TOGGLE_TAGS.contains(&i.widget_type.as_ref())
            || i.vm_node
                .as_ref()
                .is_some_and(|n| TOGGLE_TAGS.contains(&n.tag.as_ref()))
    });
    Some((row.clone(), cell.clone(), switch))
}

/// Whether a click could land on the last row's switch: the tracker records
/// bounds already intersected with the content mask, and gpui hit-tests the
/// same intersection — so a switch clipped away has no area and a switch below
/// the window cannot be pointed at.
fn last_switch_is_reachable(painted: &Painted, k: usize) -> bool {
    let Some((_, _, Some((_, info)))) = last_switch(painted, k) else {
        return false;
    };
    info.has_visible_area()
        && info.y >= -EPS
        && bottom(&info) <= WINDOW_H + EPS
        && info.x >= -EPS
        && info.x + info.width <= WINDOW_W + EPS
}

/// Numbers on every run: where each row's switch actually sits against the
/// window, so a red says WHICH rows fell off the bottom and by how much.
fn rows_report(
    painted: &Painted,
    cells: &BTreeMap<String, (String, ElementInfo)>,
    enabled: &BTreeMap<String, i64>,
) -> String {
    let mut s = format!(
        "\n=== settings integrations `Enabled` column in a {WINDOW} window (window bottom \
         y={WINDOW_H:.1}) ===\n"
    );
    for (row, (cell_id, cell)) in cells {
        s.push_str(&format!(
            "  row {row:?}: cell y={:.1}..{:.1} h={:.1} x={:.1} w={:.1}{}\n",
            cell.y,
            bottom(cell),
            cell.height,
            cell.x,
            cell.width,
            if bottom(cell) > WINDOW_H {
                "  <-- BELOW THE WINDOW"
            } else {
                ""
            }
        ));
        for (id, info) in painted.descendants_of(cell_id) {
            s.push_str(&format!(
                "      {:<14} y={:7.1}..{:7.1} h={:5.1} w={:5.1} id={id}\n",
                info.widget_type.as_ref(),
                info.y,
                bottom(&info),
                info.height,
                info.width,
            ));
        }
    }
    s.push_str(&format!("  mirror `enabled`: {enabled:?}\n"));
    s
}

#[test]
fn the_last_integrations_row_switch_is_clickable() {
    // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
    // app boots and before any thread reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", WINDOW) };

    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    // The bundled gcal sidecar names `~/.config/holon/gcal-client-*`; point HOME
    // at an empty dir so nothing this test clicks can reach a real browser.
    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: as above.
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

    // `provider_name` -> `enabled` from the mirror the switch writes. A
    // closure rather than a function: `holon-gpui` does not depend on
    // `holon-turso`, so the handle type cannot be named here.
    let mirror_enabled = {
        let runtime = runtime.clone();
        move || -> BTreeMap<String, i64> {
            runtime.block_on(async {
                db.query(
                    "SELECT provider_name, enabled FROM integration_state ORDER BY provider_name \
                     ASC",
                    Default::default(),
                )
                .await
                .expect("read the integration_state mirror")
                .iter()
                .map(|r| {
                    let name = r
                        .get("provider_name")
                        .and_then(|v| v.as_string())
                        .expect("every mirror row carries provider_name")
                        .to_string();
                    let enabled = r
                        .get("enabled")
                        .and_then(|v| v.as_i64())
                        .expect("every mirror row carries enabled");
                    (name, enabled)
                })
                .collect()
            })
        }
    };

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
                "Holon-SettingsIntegrationsLastRowToggle-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let gear = bounds.element_info(SETTINGS_GEAR).unwrap_or_else(|| {
        panic!(
            "the toolbar gear is not registered as {SETTINGS_GEAR:?}, so no window test can open \
             Settings — the modal has no command and no keybinding either"
        )
    });
    let window = rebind.window();
    click_at(&mut app, window, center_of(&gear), "gear");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let unscrolled = Painted::snapshot(&bounds);
    let headers = unscrolled.headers();
    let enabled_col = headers
        .iter()
        .find(|(_, h)| h.displayed_text.as_deref() == Some(ENABLED_HEADER))
        .map(|(k, _)| *k);
    let header_texts: Vec<String> = headers
        .values()
        .map(|h| h.displayed_text.as_deref().unwrap_or("?").to_string())
        .collect();

    let before = mirror_enabled();

    // The fold, as the modal first shows it. Kept in the log because it is what
    // the 2026-09-12 report measured, and a reader comparing the two blocks can
    // see that the rows below it are clipped rather than missing.
    let unscrolled_report = enabled_col
        .map(|k| {
            format!(
                "\n--- before scrolling ---{}",
                rows_report(&unscrolled, &unscrolled.cells_of_column(k), &before)
            )
        })
        .unwrap_or_default();

    // Scroll the way a user does. gpui gates a div's wheel handling on the
    // hitbox being HOVERED (`Interactivity::paint` → `should_handle_scroll`),
    // so the pointer move is part of the gesture — a bare `ScrollWheelEvent`
    // is dropped and would make this rung declare a working modal broken.
    let mut notches = 0usize;
    let mut painted = unscrolled;
    while enabled_col.is_some_and(|k| !last_switch_is_reachable(&painted, k))
        && notches < MAX_NOTCHES
    {
        wheel(&mut app, window, modal_center(), -WHEEL_DY);
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(5));
        notches += 1;
        painted = Painted::snapshot(&bounds);
    }

    // Everything the assertions need, gathered before teardown. Teardown has to
    // happen before the first `assert!`, or a red also trips the gpui leak
    // detector and buries the real failure.
    let outcome = enabled_col.map(|k| {
        let cells = painted.cells_of_column(k);
        let report = format!(
            "{unscrolled_report}\n--- after {notches} wheel notch(es) of {WHEEL_DY:.0}px over the \
             modal ---{}",
            rows_report(&painted, &cells, &before)
        );
        (cells, report, last_switch(&painted, k))
    });

    // The click, at the centre of the last row's switch — or, if the switch
    // painted nothing of its own, at the centre of its cell, which is where a
    // user would aim.
    let clicked_at = outcome.as_ref().and_then(|(_, _, target)| {
        target.as_ref().map(|(row, cell, switch)| {
            let (what, info) = match switch {
                Some((id, info)) => (id.clone(), info.clone()),
                None => (format!("{CELL_PREFIX}?-{row}"), cell.clone()),
            };
            let at = center_of(&info);
            click_at(&mut app, window, at, &what);
            (what, info, at)
        })
    });

    // The click's effect, read where the switch writes it. Polled, because the
    // op travels dispatcher → provider → projector before the mirror moves.
    let target_provider = outcome
        .as_ref()
        .and_then(|(_, _, t)| t.as_ref())
        .map(|(row, _, _)| row.rsplit(':').next().unwrap_or(row).to_string());
    let mut after = before.clone();
    if let Some(provider) = &target_provider {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(2));
            after = mirror_enabled();
            if after.get(provider) != before.get(provider) {
                break;
            }
        }
    }

    // Teardown BEFORE the assertions (see above).
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    let Some((cells, report, target)) = outcome else {
        panic!(
            "the open Settings modal paints no {ENABLED_HEADER:?} column — headers seen: \
             {header_texts:?}. Without it there is no switch to click and the assertions below \
             would judge nothing."
        )
    };
    eprintln!("{report}");

    // ── Non-vacuity ────────────────────────────────────────────────────────
    assert!(
        cells.len() >= 2,
        "the Settings modal bundles more than one provider, so the {ENABLED_HEADER:?} column must \
         paint at least two cells — with one row, `the LAST row is clickable` says nothing about \
         rows below the first.{report}"
    );
    assert_eq!(
        before.len(),
        cells.len(),
        "the mirror holds {} providers but the table painted {} rows — the test would then be \
         judging a table that is not showing what the mirror has.{report}",
        before.len(),
        cells.len()
    );
    let (row, cell, switch) = target.expect("cells is non-empty, so it has a last entry");
    let last_key = cells
        .keys()
        .next_back()
        .expect("cells is non-empty")
        .clone();
    assert_eq!(
        row, last_key,
        "the click must target the LAST row of the column, not any other.{report}"
    );
    let provider = target_provider.expect("a target row implies a provider");
    let last_provider = before
        .keys()
        .next_back()
        .expect("the mirror is non-empty")
        .clone();
    assert_eq!(
        provider, last_provider,
        "the last painted row must be the last provider in the mirror's `provider_name ASC` \
         order — otherwise the row the test clicks is not the bottom one a user sees.{report}"
    );
    let (switch_id, switch_info) = switch.unwrap_or_else(|| {
        panic!(
            "the last row {row:?} paints no switch under its {ENABLED_HEADER:?} cell — a row \
             whose only control is missing cannot be switched off at all.{report}"
        )
    });

    // ── The scroll gesture brought the row onto the screen ─────────────────
    // The table is taller than the modal at this window size, so the bottom
    // rows start clipped; the property is that the ONE gesture a user has for
    // that — the wheel over the panel — reaches them, within a bounded number
    // of notches.
    assert!(
        notches < MAX_NOTCHES,
        "after {notches} wheel notch(es) of {WHEEL_DY:.0}px over the modal at ({:.0}, {:.0}), the \
         last row's switch is still not clickable. The modal clips its content at its panel edge, \
         so if the wheel does not move it, the rows below the fold cannot be reached at \
         all.{report}",
        f32::from(modal_center().x),
        f32::from(modal_center().y)
    );
    assert!(
        switch_info.has_visible_area(),
        "the last row's switch {switch_id} paints {:.1}x{:.1} px after scrolling — it is clipped \
         to nothing, so no click can land on it.{report}",
        switch_info.width,
        switch_info.height
    );
    assert!(
        bottom(&switch_info) <= WINDOW_H + EPS && switch_info.y >= -EPS,
        "the last row's switch {switch_id} sits at y={:.1}..{:.1} after scrolling, outside the \
         {WINDOW_H:.1}px window.{report}",
        switch_info.y,
        bottom(&switch_info)
    );
    assert!(
        switch_info.x >= -EPS && switch_info.x + switch_info.width <= WINDOW_W + EPS,
        "the last row's switch {switch_id} sits at x={:.1}..{:.1}, outside the {WINDOW_W:.1}px \
         window.{report}",
        switch_info.x,
        switch_info.x + switch_info.width
    );

    // ── The click reached THAT row ─────────────────────────────────────────
    let (clicked_what, _, at) = clicked_at.expect("a target row implies a click");
    let before_v = before[&provider];
    let after_v = after[&provider];
    assert_ne!(
        after_v,
        before_v,
        "clicking {clicked_what} at ({:.1}, {:.1}) — the centre of the LAST integrations row's \
         switch, cell y={:.1}..{:.1} — must flip `enabled` for {provider:?} in the \
         `integration_state` mirror; it is still {before_v}. Switching a bad connection off in \
         Settings is the only remedy this surface offers, and it does not exist for any row but \
         the first.{report}\nmirror after the click: {after:?}",
        f32::from(at.x),
        f32::from(at.y),
        cell.y,
        bottom(&cell),
    );

    // ── ...and only that row ───────────────────────────────────────────────
    for (name, value) in &before {
        if name == &provider {
            continue;
        }
        assert_eq!(
            after.get(name),
            Some(value),
            "the click on {provider:?}'s switch also moved {name:?} — one click must decide one \
             integration.{report}\nmirror after the click: {after:?}"
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
