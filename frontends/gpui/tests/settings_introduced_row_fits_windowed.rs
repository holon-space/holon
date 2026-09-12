//! An INTRODUCED connection's disclosure stays inside the Integration column.
//!
//! The `user-connections` dogfood RE-RUN of 2026-09-12 found the `from <path>`
//! line running unbroken out of the ~108px Integration cell and straight across
//! Config, Status and Enabled, with all four texts superimposed and none of
//! them legible
//! (`docs/Testing/bugfunnel/entries/
//! 2026-09-12-an-introduced-connections-origin-line-overprints-three-table-columns.md`).
//!
//! `settings_integrations_table_fits_windowed` asserts exactly this property
//! and passed throughout, because its fixture holds only BUNDLED rows, whose
//! `origin` and `hosts` are the empty string — so no row in it has ever had a
//! second line in the Integration cell, and the second line is the whole shape
//! the feature added.
//!
//! It gets its own file rather than another row in that one: an introduced row
//! is three lines tall and fills the Settings list's viewport on its own,
//! pushing every bundled row below the fold, so the two fixtures cannot judge
//! the same window. This rung's non-vacuity bar is therefore the introduced row
//! itself — the assertions mean nothing unless its disclosure is on screen.
//!
//! A path is ONE word with no break opportunity, so wrapping cannot save it:
//! either the cell shortens what it holds or it overprints its neighbours. That
//! is why the property is geometric and lives in a window.
//!
//! What this rung cannot see: WHICH END the shortening takes. `BoundsRegistry`
//! records a text element's CONTENT, not the glyphs gpui painted after eliding,
//! so a path cut at the head and one cut at the tail are the same string here.
//! That choice — the file name is the half a reader identifies a connection by,
//! and the directory is shared by every installed file — is pinned in
//! `holon-app`'s `integrations_section` template instead.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_introduced_row_fits_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
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

/// The modal has no command and no keybinding — the toolbar gear is its one
/// door.
const SETTINGS_GEAR: &str = "settings-gear";

/// The `table` renderer's contract ids (`render/builders/table.rs`).
const HEADER_PREFIX: &str = "table-header-col-";
const CELL_PREFIX: &str = "table-cell-col-";

/// The desktop window the dogfood pass ran at.
const WINDOW: &str = "1512x900";
const WINDOW_W: f32 = 1512.0;

/// The modal panel's own geometry (`lib.rs` `modal_overlay`): `w_full` capped
/// at 640px, centred, with 24px of padding.
const MODAL_MAX_W: f32 = 640.0;

/// Sub-pixel layout rounding. Far below the ~80px column pitch this judges.
const EPS: f32 = 1.0;

/// The connection given a file origin. Any bundled provider serves — the mirror
/// is what says a row was introduced — but it must be one the modal paints:
/// this rung judges only cells with a real rect, and `claude-history` sorts
/// first.
const SUBJECT: &str = "claude-history";

/// A realistic absolute path: long, and a single unbreakable word.
const ORIGIN: &str =
    "/Users/someone/Library/Application Support/holon/integrations/fixturebox.yaml";

/// The hosts the connection's manual calls — the other disclosure line.
const HOSTS: &str = "api.fixturebox.example, cdn.fixturebox.example";

/// The contract id the Integration cell's origin line registers under
/// (`text-{row_id}-{field}`, `render/builders/text.rs`).
const ORIGIN_ELEMENT: &str = "text-integration:claude-history-origin";

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

fn right(info: &ElementInfo) -> f32 {
    info.x + info.width
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

    /// Every VISIBLE `table-cell-col-{k}-{row}`. The Settings list overdraws
    /// rows just outside its viewport; those register a degenerate rect and
    /// carry no judgeable geometry.
    fn cells(&self) -> BTreeMap<(usize, String), (String, ElementInfo)> {
        let mut out = BTreeMap::new();
        for (id, info) in &self.all {
            if !info.has_visible_area() {
                continue;
            }
            let Some(rest) = id.strip_prefix(CELL_PREFIX) else {
                continue;
            };
            let Some((k_str, row)) = rest.split_once('-') else {
                continue;
            };
            if let Ok(k) = k_str.parse::<usize>() {
                out.insert((k, row.to_string()), (id.clone(), info.clone()));
            }
        }
        out
    }

    /// The painted elements whose tracked-parent chain reaches `cell_id`.
    fn descendants_of(&self, cell_id: &str) -> Vec<(String, ElementInfo)> {
        self.all
            .iter()
            .filter(|(id, info)| {
                id.as_str() != cell_id && info.has_visible_area() && self.is_under(id, cell_id)
            })
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

/// Numbers, not booleans: the whole row's geometry, so a red log shows which
/// text crosses which column boundary and by how much.
fn table_report(
    painted: &Painted,
    headers: &BTreeMap<usize, ElementInfo>,
    cells: &BTreeMap<(usize, String), (String, ElementInfo)>,
) -> String {
    let mut s = String::from("\n=== introduced-row table geometry ===\n");
    for (k, h) in headers {
        s.push_str(&format!(
            "col {k} header {:?} x={:.1}..{:.1} (w={:.1})\n",
            h.displayed_text.as_deref().unwrap_or("?"),
            h.x,
            right(h),
            h.width
        ));
    }
    for ((k, row), (cell_id, cell)) in cells {
        s.push_str(&format!(
            "  cell col {k} row {row:?}: x={:.1}..{:.1} h={:.1}\n",
            cell.x,
            right(cell),
            cell.height
        ));
        for (id, info) in painted.descendants_of(cell_id) {
            s.push_str(&format!(
                "      {:<14} x={:7.1}..{:7.1} h={:5.1} text={:?} id={id}\n",
                info.widget_type.as_ref(),
                info.x,
                right(&info),
                info.height,
                info.displayed_text.as_deref().unwrap_or("")
            ));
        }
    }
    s
}

#[test]
fn an_introduced_rows_origin_line_stays_inside_its_column() {
    // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
    // app boots and before any thread reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", WINDOW) };

    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    // The bundled gcal sidecar names `~/.config/holon/gcal-client-*`; point HOME
    // at an empty dir so a consent flow cannot reach a real browser.
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
                "Holon-IntroducedRowFits-Windowed",
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

    // Make one row an INTRODUCED one, after the modal is already open, so the
    // update reaches the screen through the section's own `live_query` — the
    // path a real scan uses. Written into the mirror rather than by installing
    // a connection file, for the reason
    // `settings_introduced_connection_disclosure_windowed` gives: the mirror IS
    // what the Settings list reads, and the projector's half is pinned
    // headlessly.
    runtime.block_on(async {
        db.execute_values(
            "UPDATE integration_state SET origin = ?, hosts = ? WHERE provider_name = ?",
            vec![
                holon_api::Value::String(ORIGIN.to_string()),
                holon_api::Value::String(HOSTS.to_string()),
                holon_api::Value::String(SUBJECT.to_string()),
            ],
        )
        .await
        .expect("the mirror must carry the disclosure columns");
    });
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let painted = Painted::snapshot(&bounds);
    let headers = painted.headers();
    let cells = painted.cells();
    let report = table_report(&painted, &headers, &cells);
    let contents: BTreeMap<(usize, String), Vec<(String, ElementInfo)>> = cells
        .iter()
        .map(|(key, (cell_id, _))| (key.clone(), painted.descendants_of(cell_id)))
        .collect();

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    eprintln!("{report}");

    // ── Non-vacuity ────────────────────────────────────────────────────────
    assert!(
        !headers.is_empty(),
        "the open Settings modal must paint the integrations table's header row, else every \
         assertion below judges nothing.{report}"
    );
    let columns: BTreeSet<usize> = cells.keys().map(|(k, _)| *k).collect();
    assert!(
        columns.len() >= 2,
        "a table needs at least two columns for `stays inside its column` to mean anything; saw \
         {columns:?}.{report}"
    );
    let origin_painted = contents
        .values()
        .flatten()
        .any(|(id, i)| id == ORIGIN_ELEMENT && i.displayed_text.as_deref() == Some(ORIGIN));
    assert!(
        origin_painted,
        "no visible cell holds {ORIGIN_ELEMENT:?} carrying the origin, so this rung is judging \
         bundled rows whose Integration cell has never had a second line — which is exactly how \
         the overprint shipped.{report}"
    );

    // ── (1) The disclosure fits the cell that holds it ─────────────────────
    // The overprint, stated as geometry: a text wider than its cell is painted
    // over whatever the next columns hold, and both become unreadable.
    for ((k, row), (_, cell)) in &cells {
        let header_label = headers
            .get(k)
            .and_then(|h| h.displayed_text.as_deref())
            .unwrap_or("?")
            .to_string();
        for (id, info) in &contents[&(*k, row.clone())] {
            assert!(
                info.x >= cell.x - EPS && right(info) <= right(cell) + EPS,
                "column {k} ({header_label:?}) row {row:?} paints {id} at x={:.1}..{:.1}, outside \
                 its cell's {:.1}..{:.1}. An introduced connection's origin and hosts are the only \
                 content this column ever holds that is wider than it, and a path is one word with \
                 no break opportunity — so what spills is painted straight over the Config, Status \
                 and Enabled texts and neither can be read.{report}",
                info.x,
                right(info),
                cell.x,
                right(cell)
            );
        }
    }

    // ── (2) Nothing the row paints crosses the modal panel ─────────────────
    let panel_left = (WINDOW_W - MODAL_MAX_W) / 2.0;
    let panel_right = panel_left + MODAL_MAX_W;
    for ((k, row), (_, cell)) in &cells {
        for (id, info) in contents[&(*k, row.clone())].iter().chain(std::iter::once(&(
            format!("{CELL_PREFIX}{k}-{row}"),
            cell.clone(),
        ))) {
            assert!(
                info.x >= panel_left - EPS && right(info) <= panel_right + EPS,
                "the table paints {id} at x={:.1}..{:.1}, past the modal panel's \
                 {panel_left:.1}..{panel_right:.1} — it is cut off at the modal's border and \
                 unreadable.{report}",
                info.x,
                right(info)
            );
        }
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
