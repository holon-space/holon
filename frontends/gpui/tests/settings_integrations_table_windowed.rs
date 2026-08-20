//! The Settings modal renders its integrations as a COLUMNAR TABLE — a header
//! row plus one row per provider, with every column left-aligned across all
//! rows and the header — in a REAL window over a REAL booted engine.
//!
//! Today the section renders `list(#{item_template: render_entity()})`, i.e. a
//! flat `row(...)` per provider with no header and no shared column geometry
//! (`crates/holon-app/src/integrations_section.rs`,
//! `assets/default/types/integration_profile.yaml`). So this test is RED for
//! the right reason: it asks for `table-header-col-*` / `table-cell-col-*`
//! elements that the flat template never paints. The columnar `table` widget is
//! the missing feature.
//!
//! The id scheme is the contract between this test and the future `table`
//! builder: for a table of C columns over R provider rows, the builder
//! registers `table-header-col-{k}` for each header cell and
//! `table-cell-col-{k}-{row}` for each data cell (`k` 0-based, `{row}` the
//! row's entity key). This test discovers C and R from those ids rather than
//! hard-coding the provider set, so it does not drift when the bundled
//! providers change.
//!
//! The modal's one door is the toolbar gear, so the rung clicks it (same path
//! as `settings_integrations_ops_windowed.rs`).
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integrations_table_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

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

/// The toolbar affordance that opens Settings. The modal has no command and no
/// keybinding — this is the only door.
const SETTINGS_GEAR: &str = "settings-gear";

/// The id scheme the `table` builder must register (see module docs).
const HEADER_PREFIX: &str = "table-header-col-";
const CELL_PREFIX: &str = "table-cell-col-";

/// Column-left agreement tolerance, logical px.
const X_EPS: f32 = 1.0;

/// Dispatch a real left click at `center`.
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

/// Every widget tag the window painted, with counts — the evidence a reader
/// needs to tell "the modal never opened" from "it opened and the table is
/// missing".
fn painted_widget_census(bounds: &BoundsRegistry) -> String {
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for (_, info) in bounds.all_elements() {
        if let Some(node) = &info.vm_node {
            *tags.entry(node.tag.to_string()).or_default() += 1;
        }
    }
    format!("{tags:?}")
}

/// `table-header-col-{k}` → `(k, ElementInfo)`.
fn header_cells(bounds: &BoundsRegistry) -> BTreeMap<usize, ElementInfo> {
    let mut out = BTreeMap::new();
    for (id, info) in bounds.all_elements() {
        if let Some(rest) = id.strip_prefix(HEADER_PREFIX) {
            if let Ok(k) = rest.parse::<usize>() {
                out.insert(k, info);
            }
        }
    }
    out
}

/// `table-cell-col-{k}-{row}` → column `k` → list of `(row, x-left)`.
fn data_cells(bounds: &BoundsRegistry) -> BTreeMap<usize, Vec<(String, f32)>> {
    let mut out: BTreeMap<usize, Vec<(String, f32)>> = BTreeMap::new();
    for (id, info) in bounds.all_elements() {
        if let Some(rest) = id.strip_prefix(CELL_PREFIX) {
            if let Some((k_str, row)) = rest.split_once('-') {
                if let Ok(k) = k_str.parse::<usize>() {
                    out.entry(k).or_default().push((row.to_string(), info.x));
                }
            }
        }
    }
    for cells in out.values_mut() {
        cells.sort_by(|a, b| a.0.cmp(&b.0));
    }
    out
}

/// A readable dump of every column's per-row left-x — numbers, not booleans, so
/// a red log shows exactly which column is ragged and by how much.
fn column_geometry_report(
    headers: &BTreeMap<usize, ElementInfo>,
    cells: &BTreeMap<usize, Vec<(String, f32)>>,
) -> String {
    let mut s = String::new();
    let cols: std::collections::BTreeSet<usize> =
        headers.keys().chain(cells.keys()).copied().collect();
    for k in cols {
        let header_x = headers.get(&k).map(|h| h.x);
        let rows = cells.get(&k).cloned().unwrap_or_default();
        s.push_str(&format!("  col {k}: header_x={header_x:?} rows={rows:?}\n"));
    }
    s
}

#[test]
fn the_settings_modal_renders_integrations_as_an_aligned_table() {
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
                "Holon-SettingsIntegrationsTable-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    assert!(
        header_cells(&bounds).is_empty(),
        "precondition: the Settings modal is closed, so no table header is painted"
    );

    let gear = bounds.element_info(SETTINGS_GEAR).unwrap_or_else(|| {
        panic!(
            "the toolbar gear is not registered as {SETTINGS_GEAR:?}, so no window test can open \
             Settings — the modal has no command and no keybinding either"
        )
    });

    let window = rebind.window();
    click_at(&mut app, window, center_of(&gear), "gear");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let headers = header_cells(&bounds);
    let cells = data_cells(&bounds);
    let census = painted_widget_census(&bounds);
    let geometry = column_geometry_report(&headers, &cells);

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    // (1) The table exists at all: an open Settings modal must paint a header
    // row. This is the assertion that goes RED today — the flat
    // `render_entity()` template paints no `table-header-col-*` element.
    assert!(
        !headers.is_empty(),
        "the open Settings modal must render the integrations as a TABLE with a header row, but no \
         `{HEADER_PREFIX}*` element was painted.\ncensus: {census}\ngeometry:\n{geometry}"
    );

    // (2) At least two columns and (3) at least two provider rows, or "aligned
    // across all rows" is vacuous.
    let column_ids: std::collections::BTreeSet<usize> =
        headers.keys().chain(cells.keys()).copied().collect();
    assert!(
        column_ids.len() >= 2,
        "a table needs at least two columns to have column structure worth aligning; saw \
         {}.\ngeometry:\n{geometry}",
        column_ids.len()
    );
    let row_keys: std::collections::BTreeSet<String> = cells
        .values()
        .flat_map(|v| v.iter().map(|(r, _)| r.clone()))
        .collect();
    assert!(
        row_keys.len() >= 2,
        "the Settings modal bundles more than one provider, so the table must paint at least two \
         data rows; saw rows {row_keys:?}.\ngeometry:\n{geometry}"
    );

    // (4) Every column is left-aligned across ALL rows AND its header. This is
    // the property a flat `row` per provider lacks.
    for k in &column_ids {
        let header = headers.get(k).unwrap_or_else(|| {
            panic!(
                "column {k} has data cells but no `{HEADER_PREFIX}{k}` header cell — the header \
                 row and the body disagree on the column set.\ngeometry:\n{geometry}"
            )
        });
        assert!(
            header
                .displayed_text
                .as_deref()
                .is_some_and(|t| !t.is_empty()),
            "header cell `{HEADER_PREFIX}{k}` must carry the column's label text, but painted \
             none.\ngeometry:\n{geometry}"
        );

        let col_cells = cells.get(k).cloned().unwrap_or_default();
        assert_eq!(
            col_cells.len(),
            row_keys.len(),
            "column {k} must have exactly one cell per provider row ({} rows), saw {}.\n\
             geometry:\n{geometry}",
            row_keys.len(),
            col_cells.len()
        );

        let ref_x = header.x;
        for (row, x) in &col_cells {
            assert!(
                (x - ref_x).abs() <= X_EPS,
                "column {k} is not left-aligned: header starts at x={ref_x:.1} but row {row:?} \
                 cell starts at x={x:.1} (>{X_EPS} px apart).\ngeometry:\n{geometry}"
            );
        }
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
