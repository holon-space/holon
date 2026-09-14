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
use holon_gpui::RebindHandle;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::ComposedSut;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::resize_window;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The modal has no command and no keybinding — the toolbar gear is its one
/// door.
const SETTINGS_GEAR: &str = "settings-gear";

/// The `table` renderer's contract ids (`render/builders/table.rs`).
const HEADER_PREFIX: &str = "table-header-col-";
const CELL_PREFIX: &str = "table-cell-col-";

/// Every viewport width the table must survive, from the app's own floor to a
/// comfortable desktop.
///
/// 300 is `MIN_WIDTH` in `window_state.rs` — the width the app itself will open
/// at. At that width the five columns kept their flex shares of a ~220px
/// content box and every one of them became narrower than its own words: the
/// header read `Int / eg / rat / ion`, `jsonplaceholder` read
/// `jso / npl / ace / hol / der`, and one row filled the viewport
/// (`docs/Testing/bugfunnel/entries/
/// 2026-09-12-the-settings-integrations-table-collapses-at-the-minimum-window-width.md`).
///
/// This rung had asserted exactly the right property since it was written, and
/// only ever at 1512 — where the columns are wide enough that they cannot fail
/// it. A SWEEP is what makes the property about the table rather than about one
/// window: the fix changes the table's shape below a threshold, and a threshold
/// tested at one point on either side of it is a constant, not a rule.
const WINDOW_HEIGHT: f32 = 900.0;
const WIDTHS: &[f32] = &[300.0, 360.0, 480.0, 640.0, 900.0, 1200.0, 1512.0];

/// The modal panel's own geometry (`lib.rs` `modal_overlay`): an overlay inset
/// by 16px holding a panel that is `w_full` capped at 640px, centred, with 24px
/// of padding. The panel is a plain gpui div and registers no bounds of its
/// own, so its edges are reconstructed here — and assertion (0) below checks
/// the reconstruction against the painted header row rather than trusting it.
const MODAL_MAX_W: f32 = 640.0;
const MODAL_PAD: f32 = 24.0;
const OVERLAY_INSET: f32 = 16.0;

/// The panel's left and right edges in a window `window_w` wide.
fn panel_edges(window_w: f32) -> (f32, f32) {
    let panel_w = MODAL_MAX_W.min(window_w - 2.0 * OVERLAY_INSET);
    let left = (window_w - panel_w) / 2.0;
    (left, left + panel_w)
}

/// Sub-pixel layout rounding. Far below the ~80px column pitch this judges.
const EPS: f32 = 1.0;

/// `text(...)` cells paint at `line_height(26px)` (`render/builders/text.rs`).
/// One line plus rounding passes; two lines (52px) is the wrap.
const MAX_SINGLE_LINE_H: f32 = 34.0;

/// The columns whose cells wrap their own content — `list(#{…, wrap: "wrap"})`
/// in `integrations_section.rs`. The table gives these no floor, so they are
/// the ones allowed to shrink below their authored share, and the budget claim
/// names them rather than inferring them: the wrapping row paints as a plain
/// `row`, so nothing in the painted tree says which column it came from.
const WRAPPING_HEADERS: &[&str] = &["Setup"];

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
            "col {k} header {:?} x={:.1}..{:.1} (w={:.1} h={:.1})\n",
            h.displayed_text.as_deref().unwrap_or("?"),
            h.x,
            right(h),
            h.width,
            h.height
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
    let mut sweep = Sweep::open();
    let mut violations: Vec<String> = Vec::new();
    let seen: Vec<Seen> = WIDTHS
        .iter()
        .map(|w| {
            let (seen, found) = sweep.measure(*w, WINDOW_HEIGHT);
            violations.extend(found);
            seen
        })
        .collect();
    let painted_rows: Vec<usize> = seen.iter().map(|s| s.rows).collect();

    // Teardown BEFORE the first assertion so a red does not also trip the gpui
    // leak detector, which would bury the real failure. One boot means one
    // teardown at the end of the sweep, so the per-width judgements are
    // collected rather than asserted where they are made.
    let Sweep {
        mut app,
        rebind,
        bundle,
        ..
    } = sweep;
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    assert!(
        violations.is_empty(),
        "the integrations table does not fit at every swept width: {} violation(s) over the \
         {} widths {:?}.\n\n{}",
        violations.len(),
        WIDTHS.len(),
        WIDTHS,
        violations.join("\n\n")
    );

    // ── The sweep really swept ─────────────────────────────────────────────
    // One window measured at seven widths, not seven windows: the narrow end
    // breaks the header row across lines and the wide end keeps it on one. If
    // the resize never reached the platform, every width would report the same
    // shape and the fit claims above would be one layout judged seven times.
    let narrow = seen.first().expect("WIDTHS is not empty");
    let wide = seen.last().expect("WIDTHS is not empty");
    assert!(
        narrow.stacked,
        "at {}px the header row still painted on one line, so no width in the sweep made the \
         table change shape and the sweep measured one layout seven times. Headers stacked per \
         width: {:?}",
        WIDTHS[0],
        WIDTHS
            .iter()
            .zip(seen.iter().map(|s| s.stacked))
            .collect::<Vec<_>>()
    );
    assert!(
        !wide.stacked,
        "at {}px the header row painted on more than one line, so the widest window in the sweep \
         is still below the table's declared budget and there is no width where the columns sit \
         side by side. Headers stacked per width: {:?}",
        WIDTHS[WIDTHS.len() - 1],
        WIDTHS
            .iter()
            .zip(seen.iter().map(|s| s.stacked))
            .collect::<Vec<_>>()
    );

    assert!(
        seen.iter().any(|s| s.cols >= 2),
        "no width in the sweep painted two columns on one line, so `fits its column` judged \
         nothing anywhere. Columns per width: {:?}",
        WIDTHS
            .iter()
            .zip(seen.iter().map(|s| s.cols))
            .collect::<Vec<_>>()
    );

    // ── A column never shrinks below the budget it was authored for ────────
    // The rule the fix installs, stated without a model of the text system: the
    // weights are a budget for a container of a DECLARED size, so a narrower
    // container breaks the row and every column keeps its width, instead of
    // every column keeping its share and losing its words. It is also what
    // stops an introduced connection's `from` and `calls` values eliding to
    // nothing at 300, which is the same collapse seen from the other end
    // (`docs/Testing/bugfunnel/entries/
    // 2026-09-12-the-hosts-disclosure-line-elides-a-host-that-fits-its-column.md`).
    //
    // The reference is the WIDEST WINDOW's run rather than a constant: there
    // the whole table fits on one line, so each column is exactly its share of
    // the declared budget — which is the floor every narrower window must also
    // clear. Narrower windows often come out WIDER than the floor, because a
    // wrapped line shares its width between fewer columns; that is the reflow
    // working, not a violation.
    let budget = seen.last().expect("WIDTHS is not empty").headers.clone();
    assert!(
        budget.len() >= 2,
        "the widest run painted {} column(s), so there is no budget to hold the narrow runs to",
        budget.len()
    );
    // A column whose cells WRAP is exempt, and deliberately so: it is the one
    // shape the table gives no floor, because giving one both over-provisions it
    // in a narrow container and makes its own wrapping unreachable. It is not
    // unjudged — its buttons must still stay inside its cell, and its header
    // must still hold one line, both asserted per width above.
    let wrapping = seen.last().expect("WIDTHS is not empty").wrapping.clone();
    assert!(
        wrapping.len() < budget.len(),
        "every column wraps, so the budget claim below exempts all of them and judges nothing"
    );
    for (k, floor) in budget.iter().enumerate() {
        if wrapping.contains(&k) {
            continue;
        }
        for (w, s) in WIDTHS.iter().zip(&seen) {
            let Some(got) = s.headers.get(k).copied() else {
                continue;
            };
            assert!(
                got >= floor - EPS,
                "at {w}px wide, column {k} is {got:.1}px against its {floor:.1}px share of the \
                 budget. A narrow window must break the row, not shave every column — shaving is \
                 what left each cell too narrow for its own word. Column {k} per width: {:?}",
                WIDTHS
                    .iter()
                    .zip(seen.iter().map(|s| s.headers.get(k).copied()))
                    .collect::<Vec<_>>()
            );
        }
    }

    // The row count is judged over the SWEEP rather than inside each width. A
    // narrow viewport legitimately shows fewer rows — the panel is 720px tall
    // and the list scrolls — so demanding two rows at every width would make
    // the narrow cases fail as broken preconditions instead of on the property.
    // Demanding them SOMEWHERE keeps the cell assertions from being vacuous
    // everywhere at once.
    assert!(
        painted_rows.iter().any(|n| *n >= 2),
        "no width in the sweep painted two visible data rows, so the per-cell assertions judged \
         nothing anywhere. Rows per width: {:?}",
        WIDTHS.iter().zip(&painted_rows).collect::<Vec<_>>()
    );
}

/// What one viewport's run measured, for the cross-width claims the caller
/// makes: how many visible data rows and columns it judged (so the sweep can
/// prove it was not vacuous end to end) and how wide each column came out.
struct Seen {
    rows: usize,
    cols: usize,
    headers: Vec<f32>,
    /// Whether this viewport broke the header row across lines. The sweep's
    /// non-vacuity: a resize that never happened paints one shape at every
    /// width, and only a real sweep has a narrow end that wraps and a wide end
    /// that does not.
    stacked: bool,
    /// Columns whose cells hold a wrapping collection. They are the ones the
    /// table gives no floor, so they are the ones allowed to shrink.
    wrapping: BTreeSet<usize>,
}

/// One viewport's worth of the property.
/// The booted window every width of the sweep is measured through.
///
/// ONE boot, RESIZED between widths. The table's shape is a function of the
/// container width, so re-booting per width measures one rule seven times at
/// seven boot prices; the window is the same window at seven widths. That the
/// resize took effect is asserted per width (see `viewport`), so a platform
/// where `Window::resize` is inert fails loudly instead of reporting the
/// geometry of one window seven times and calling it a sweep.
struct Sweep {
    app: HeadlessAppContext,
    bounds: BoundsRegistry,
    runtime: Arc<tokio::runtime::Runtime>,
    rebind: RebindHandle,
    bundle: ComposedSut,
    /// The empty HOME the SUT boots against; held so it outlives the session.
    _home: tempfile::TempDir,
}

impl Sweep {
    /// Boot at the narrowest swept width and open Settings on it.
    fn open() -> Self {
        let window_spec = format!("{}x{}", WIDTHS[0] as i32, WINDOW_HEIGHT as i32);
        // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
        // app boots and before any thread reads the environment.
        unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", &window_spec) };

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

        Self {
            app,
            bounds,
            runtime,
            rebind,
            bundle,
            _home: home,
        }
    }

    /// Resize to `window_w`x`window_h`, settle, and JUDGE that width.
    ///
    /// Returns what the caller's cross-width claims need, plus every violation
    /// found — judged, not asserted, because the sweep's teardown has to happen
    /// before the first panic.
    fn measure(&mut self, window_w: f32, window_h: f32) -> (Seen, Vec<String>) {
        let app = &mut self.app;
        let bounds = &self.bounds;
        let runtime = &self.runtime;
        let rebind = &self.rebind;
        let mut violations: Vec<String> = Vec::new();
        let window_spec = format!("{}x{}", window_w as i32, window_h as i32);
        let window = rebind.window();
        let viewport = resize_window(app, window, window_w, window_h);

        settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(30));

        let painted = Painted::snapshot(bounds);
        let headers = painted.headers();
        let cells = painted.cells();
        let report = table_report(&painted, &headers, &cells);
        let contents: BTreeMap<(usize, String), Vec<(String, ElementInfo)>> = cells
            .iter()
            .map(|(key, (cell_id, _))| (key.clone(), painted.descendants_of(cell_id)))
            .collect();

        // Numbers on every run, not only on a red: the margins a column has left
        // are what a reader needs to judge whether a passing table is comfortable
        // or one glyph from overflowing again.
        eprintln!("--- window {window_spec} ---{report}");

        // ── The resize reached the window ──────────────────────────────────────
        // Every judgement below is about a window `window_w` wide. If the platform
        // ignored the resize, all seven widths would report one window's geometry
        // and the sweep would pass by measuring nothing.
        if (viewport - window_w).abs() > 1.0 {
            violations.push(format!(
                "the window is {viewport:.1}px wide after being resized to {window_w}px, so this \
             width's judgements are about a different window than the one they name.{report}"
            ));
        }

        // ── Non-vacuity ────────────────────────────────────────────────────────
        if headers.is_empty() {
            violations.push(format!(
            "the open Settings modal must paint the integrations table's header row, else every \
             assertion below judges nothing.{report}"
        ));
        }
        let rows: BTreeSet<&String> = cells.keys().map(|(_, row)| row).collect();
        let columns: BTreeSet<usize> = cells.keys().map(|(k, _)| *k).collect();
        // How many columns and rows are VISIBLE is a property of this viewport, not
        // of the table: below the wrap threshold a row occupies several lines and
        // the panel scrolls, so a narrow run legitimately judges one column of one
        // row. The sweep as a whole is what must be non-vacuous, and the caller
        // asserts that over every width at once.
        if !(rows.is_empty() || contents.values().any(|d| !d.is_empty())) {
            violations.push(format!(
                "every visible cell painted an empty content subtree — the fit assertions would be \
             vacuous.{report}"
            ));
        }

        // ── (4) The HEADER row survives this width ─────────────────────────────
        // Judged FIRST, and independently of the data rows, because the header is
        // painted whatever the list scrolls to. Every header is one word, so the
        // rule has nothing to argue about: `Integration` painted as
        // `Int / eg / rat / ion` is the clearest single statement of the collapse,
        // and it is the row a reader uses to tell which column is which.
        for (k, h) in &headers {
            if h.height > MAX_SINGLE_LINE_H {
                violations.push(format!(
                    "in the {window_spec} window, the header {:?} (column {k}) is {:.1}px tall and \
                 {:.1}px wide — more than the {MAX_SINGLE_LINE_H:.1}px one line takes, so the \
                 column is narrower than its own one-word name and the name is broken across \
                 lines. Below the width where the columns stop fitting side by side, the table \
                 has to change shape rather than keep shrinking them.{report}",
                    h.displayed_text.as_deref().unwrap_or("?"),
                    h.height,
                    h.width
                ));
            }
        }

        // ── (0) The reconstructed modal panel matches what was painted ─────────
        let (panel_left, panel_right) = panel_edges(window_w);
        let content_left = panel_left + MODAL_PAD;
        let content_right = panel_right - MODAL_PAD;
        for (k, h) in &headers {
            if !(h.x >= content_left - EPS && right(h) <= content_right + EPS) {
                violations.push(format!(
                    "header cell {k} sits at x={:.1}..{:.1}, outside the modal panel's content box \
                 {content_left:.1}..{content_right:.1} reconstructed from `modal_overlay`. Either \
                 the panel geometry changed or the header row itself overflows; fix this test's \
                 reconstruction before reading the assertions below.{report}",
                    h.x,
                    right(h)
                ));
            }
        }

        // ── (1) Every cell's content fits the cell that holds it ───────────────
        for ((k, row), (_, cell)) in &cells {
            let header_label = headers
                .get(k)
                .and_then(|h| h.displayed_text.as_deref())
                .unwrap_or("?")
                .to_string();
            for (id, info) in &contents[&(*k, row.clone())] {
                if !(info.x >= cell.x - EPS && right(info) <= right(cell) + EPS) {
                    violations.push(format!(
                        "column {k} ({header_label:?}) row {row:?} paints {id} at \
                     x={:.1}..{:.1}, outside its cell's {:.1}..{:.1} — the column is too narrow \
                     for what it holds, so the content spills over the next column.{report}",
                        info.x,
                        right(info),
                        cell.x,
                        right(cell)
                    ));
                }
            }
        }

        // ── (2) Nothing the table paints crosses the modal panel ───────────────
        for ((k, row), (_, cell)) in &cells {
            for (id, info) in contents[&(*k, row.clone())].iter().chain(std::iter::once(&(
                format!("{CELL_PREFIX}{k}-{row}"),
                cell.clone(),
            ))) {
                if !(info.x >= panel_left - EPS && right(info) <= panel_right + EPS) {
                    violations.push(format!(
                        "the table paints {id} at x={:.1}..{:.1}, past the modal panel's \
                     {panel_left:.1}..{panel_right:.1} — it is cut off at the modal's border and \
                     unreadable.{report}",
                        info.x,
                        right(info)
                    ));
                }
            }
        }

        // ── (3) No painted text is broken across lines ─────────────────────────
        // "unconfigured" and "jsonplaceholder" are single words with no break
        // opportunity, so a two-line cell is that word broken mid-word — the defect
        // the dogfood pass saw, and at 300 every cell was a vertical stack of
        // syllables. gpui wraps INSIDE one text element rather than emitting one
        // element per line, so the split shows up as that element's HEIGHT.
        //
        // Judged per TEXT, not per cell: an introduced connection's Integration
        // cell is three lines by design — name, `from <file>`, `calls <host>` — and
        // a cell-level rule would read that as a wrap. Each of those three texts is
        // still one line, which is the property.
        for (k, row) in cells.keys() {
            let header_label = headers
                .get(k)
                .and_then(|h| h.displayed_text.as_deref())
                .unwrap_or("?")
                .to_string();
            for (id, info) in &contents[&(*k, row.clone())] {
                let interactive = INTERACTIVE_TAGS.contains(&info.widget_type.as_ref())
                    || info
                        .vm_node
                        .as_ref()
                        .is_some_and(|n| INTERACTIVE_TAGS.contains(&n.tag.as_ref()));
                if interactive || info.widget_type.as_ref() != "text" {
                    continue;
                }
                if info.height > MAX_SINGLE_LINE_H {
                    violations.push(format!(
                    "in the {window_spec} window, column {k} ({header_label:?}) row {row:?} paints \
                     {id} {:.1}px tall in {:.1}px of width — more than the \
                     {MAX_SINGLE_LINE_H:.1}px one line takes, so the text wrapped. Its value is \
                     one word with no break opportunity, so the wrap breaks the word mid-word and \
                     the cell reads as a vertical stack of syllables.{report}",
                    info.height,
                    info.width
                ));
                }
            }
        }

        let wrapping: BTreeSet<usize> = headers
            .iter()
            .filter(|(_, h)| {
                h.displayed_text
                    .as_deref()
                    .is_some_and(|t| WRAPPING_HEADERS.contains(&t))
            })
            .map(|(k, _)| *k)
            .collect();

        let header_ys: Vec<f32> = headers.values().map(|h| h.y).collect();
        let stacked = header_ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(*y), hi.max(*y)));
        (
            Seen {
                rows: rows.len(),
                cols: columns.len(),
                headers: headers.values().map(|h| h.width).collect(),
                stacked: stacked.1 - stacked.0 > EPS,
                wrapping,
            },
            violations,
        )
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
