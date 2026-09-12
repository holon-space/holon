//! The Settings → Integrations table FITS: every cell's painted content lies
//! inside the cell that holds it, and nothing the table paints reaches past the
//! modal panel — judged in a REAL window over a REAL booted engine.
//!
//! The dogfood pass of 2026-09-12
//! (`docs/Testing/bugfunnel/entries/
//! 2026-09-12-the-settings-integrations-table-wraps-mid-word-and-overflows-its-modal.
//! md`) saw two things at the default window size: the Config column broke the
//! single word "unconfigured" across two lines, and the Setup column's content
//! ran past the modal's right border. `settings_integrations_table_windowed.rs`
//! already asserts the table's columns are ALIGNED, and
//! `settings_integrations_row_op_alignment_windowed.rs` that its buttons sit on
//! their row's baseline — neither asks whether what a column holds FITS in it.
//! That is the missing oracle, and it is geometry, not text presence.
//!
//! Three properties, all read off `BoundsRegistry`:
//!   1. every painted descendant of a `table-cell-col-{k}-{row}` lies inside
//!      that cell's horizontal extent — no column overflows;
//!   2. nothing the table paints crosses the modal panel's right edge;
//!   3. a column that holds only text paints one line per cell — a cell two
//!      lines tall is a word wrapped, which for "unconfigured" (one word, no
//!      break opportunity) means broken mid-word.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integrations_table_fits_windowed -- --test-threads=1`
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

/// The default desktop window the dogfood pass ran at. Set before the window
/// opens; `launch_holon_window_impl` reads it.
const WINDOW: &str = "1512x900";
const WINDOW_W: f32 = 1512.0;

/// The modal panel's own geometry (`lib.rs` `modal_overlay`): `w_full` capped
/// at 640px, centred, with 24px of padding. The panel is a plain gpui div and
/// registers no bounds of its own, so its edges are reconstructed here — and
/// assertion (0) below checks the reconstruction against the painted header row
/// rather than trusting it.
const MODAL_MAX_W: f32 = 640.0;
const MODAL_PAD: f32 = 24.0;

/// Sub-pixel layout rounding. Far below the ~80px column pitch this judges.
const EPS: f32 = 1.0;

/// `text(...)` cells paint at `line_height(26px)` (`render/builders/text.rs`).
/// One line plus rounding passes; two lines (52px) is the wrap.
const MAX_SINGLE_LINE_H: f32 = 34.0;

/// Widget tags that make a column interactive rather than a plain text column.
/// A plain column is the one whose cells must be one line tall.
const INTERACTIVE_TAGS: &[&str] = &["op_button", "state_toggle", "switch_track", "list"];

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

    /// The painted elements whose tracked-parent chain reaches `cell_id`. This
    /// is the cell's content — what must fit inside it.
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

/// Numbers, not booleans: the full table geometry so a red log shows which
/// column overflows and by how much.
fn table_report(
    painted: &Painted,
    headers: &BTreeMap<usize, ElementInfo>,
    cells: &BTreeMap<(usize, String), (String, ElementInfo)>,
) -> String {
    let mut s = String::from("\n=== settings integrations table geometry ===\n");
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
fn every_integrations_cell_fits_its_column_and_the_modal() {
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
                "Holon-SettingsIntegrationsTableFits-Windowed",
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

    // Numbers on every run, not only on a red: the margins a column has left
    // are what a reader needs to judge whether a passing table is comfortable
    // or one glyph from overflowing again.
    eprintln!("{report}");

    // ── Non-vacuity ────────────────────────────────────────────────────────
    assert!(
        !headers.is_empty(),
        "the open Settings modal must paint the integrations table's header row, else every \
         assertion below judges nothing.{report}"
    );
    let rows: BTreeSet<&String> = cells.keys().map(|(_, row)| row).collect();
    assert!(
        rows.len() >= 2,
        "the Settings modal bundles more than one provider, so at least two visible data rows must \
         be painted; saw {rows:?}.{report}"
    );
    let columns: BTreeSet<usize> = cells.keys().map(|(k, _)| *k).collect();
    assert!(
        columns.len() >= 2,
        "a table needs at least two columns for `fits its column` to mean anything; saw \
         {columns:?}.{report}"
    );
    assert!(
        contents.values().any(|d| !d.is_empty()),
        "every visible cell painted an empty content subtree — the fit assertions would be \
         vacuous.{report}"
    );

    // ── (0) The reconstructed modal panel matches what was painted ─────────
    let panel_left = (WINDOW_W - MODAL_MAX_W) / 2.0;
    let panel_right = panel_left + MODAL_MAX_W;
    let content_left = panel_left + MODAL_PAD;
    let content_right = panel_right - MODAL_PAD;
    for (k, h) in &headers {
        assert!(
            h.x >= content_left - EPS && right(h) <= content_right + EPS,
            "header cell {k} sits at x={:.1}..{:.1}, outside the modal panel's content box \
             {content_left:.1}..{content_right:.1} reconstructed from `modal_overlay`. Either the \
             panel geometry changed or the header row itself overflows; fix this test's \
             reconstruction before reading the assertions below.{report}",
            h.x,
            right(h)
        );
    }

    // ── (1) Every cell's content fits the cell that holds it ───────────────
    for ((k, row), (_, cell)) in &cells {
        let header_label = headers
            .get(k)
            .and_then(|h| h.displayed_text.as_deref())
            .unwrap_or("?")
            .to_string();
        for (id, info) in &contents[&(*k, row.clone())] {
            assert!(
                info.x >= cell.x - EPS && right(info) <= right(cell) + EPS,
                "column {k} ({header_label:?}) row {row:?} paints {id} at \
                 x={:.1}..{:.1}, outside its cell's {:.1}..{:.1} — the column is too narrow for \
                 what it holds, so the content spills over the next column.{report}",
                info.x,
                right(info),
                cell.x,
                right(cell)
            );
        }
    }

    // ── (2) Nothing the table paints crosses the modal panel ───────────────
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

    // ── (3) A text-only column paints one line per cell ────────────────────
    // "unconfigured" is a single word with no break opportunity, so a two-line
    // Config cell is that word broken mid-word — the defect the dogfood pass
    // saw. gpui wraps INSIDE one text element rather than emitting one element
    // per line, so the split shows up as the cell's height, not as a second
    // painted element; the assertion is therefore on the line count.
    for ((k, row), (_, cell)) in &cells {
        let content = &contents[&(*k, row.clone())];
        let interactive = content.iter().any(|(_, i)| {
            INTERACTIVE_TAGS.contains(&i.widget_type.as_ref())
                || i.vm_node
                    .as_ref()
                    .is_some_and(|n| INTERACTIVE_TAGS.contains(&n.tag.as_ref()))
        });
        if interactive {
            continue;
        }
        let header_label = headers
            .get(k)
            .and_then(|h| h.displayed_text.as_deref())
            .unwrap_or("?")
            .to_string();
        assert!(
            cell.height <= MAX_SINGLE_LINE_H,
            "column {k} ({header_label:?}) row {row:?} is {:.1}px tall — more than the \
             {MAX_SINGLE_LINE_H:.1}px a single line takes, so its label wrapped. A text column \
             holding one word (`unconfigured`) has no break opportunity, so the wrap breaks the \
             word mid-word.{report}",
            cell.height
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
