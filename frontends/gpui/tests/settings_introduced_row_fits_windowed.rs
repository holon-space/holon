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
use pbt_harness::windowed_wide::resize_window;
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
const WINDOW_H: f32 = 900.0;

/// Every width the disclosure lines must survive, from the app's own floor
/// (`MIN_WIDTH` in `window_state.rs`) up. The re-check read BOTH disclosure
/// values eliding to nothing at 300, so the claims this rung makes at the
/// desktop width are re-judged at every width the app can open at — a cell
/// budget is exactly the shape that is right at one width and wrong at the
/// next.
const WIDTHS: &[f32] = &[300.0, 360.0, 480.0, 640.0, 900.0, 1200.0];

/// The modal panel's own geometry (`lib.rs` `modal_overlay`): an overlay inset
/// by 16px holding a panel that is `w_full` capped at 640px and centred. The
/// panel registers no bounds of its own, so its edges are reconstructed here.
const MODAL_MAX_W: f32 = 640.0;
const OVERLAY_INSET: f32 = 16.0;

/// The panel's left and right edges in a window `window_w` wide.
fn panel_edges(window_w: f32) -> (f32, f32) {
    let panel_w = MODAL_MAX_W.min(window_w - 2.0 * OVERLAY_INSET);
    let left = (window_w - panel_w) / 2.0;
    (left, left + panel_w)
}

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
const HOSTS_ELEMENT: &str = "text-integration:claude-history-hosts";

/// A host short enough that the Integration cell has room for it several times
/// over. The 2026-09-12 re-check saw `127.0.0.1` — nine characters — painted as
/// `127.0.` with a third of the column still empty, while the `from …` line one
/// line ABOVE it, in the same cell, ran about 60% further right
/// (`docs/Testing/bugfunnel/entries/
/// 2026-09-12-the-hosts-disclosure-line-elides-a-host-that-fits-its-column.
/// md`). A host cut after `127.0.` names nothing, and naming the host is the
/// line's only job.
const SHORT_VALUE: &str = "127.0.0.1";

/// How much of the cell must be left over before "it fits" is a claim rather
/// than a coincidence. A whole `127.0.0.1` costs well under this.
const SLACK_PX: f32 = 24.0;

/// What one disclosure line costs the cell in height. A line at `size: 11`
/// paints ~19px; the bar is set below that so layout rounding cannot decide the
/// verdict, and far above zero so a line that is still there cannot pass.
const LINE_COST_PX: f32 = 10.0;

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

    // ── Phases 2a / 2b / 3 ─────────────────────────────────────────────────
    // Three more configurations of the SAME row, so the disclosure lines are
    // measured against each other rather than against a px-per-character model
    // of the text system — a constant like that would be a second, silently
    // rotting copy of how gpui lays text out.
    let mut phase = |width: f32, origin: &str, hosts: &str| -> Phase {
        let viewport = resize_window(&mut app, window, width, WINDOW_H);
        runtime.block_on(async {
            db.execute_values(
                "UPDATE integration_state SET origin = ?, hosts = ? WHERE provider_name = ?",
                vec![
                    holon_api::Value::String(origin.to_string()),
                    holon_api::Value::String(hosts.to_string()),
                    holon_api::Value::String(SUBJECT.to_string()),
                ],
            )
            .await
            .expect("the mirror must take the disclosure columns");
        });
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
        let snapshot = Painted::snapshot(&bounds);
        let headers = snapshot.headers();
        let cells = snapshot.cells();
        let report = table_report(&snapshot, &headers, &cells);
        // Numbers on every run. The disclosure lines are judged against each
        // other, so a reader needs all three configurations side by side to see
        // which one moved.
        eprintln!("--- phase {width}px origin={origin:?} hosts={hosts:?} ---{report}");

        // Everything this configuration paints outside the box that should hold
        // it, collected rather than asserted: the sweep's teardown has to happen
        // before the first panic, and a width that is judged by `continue` would
        // otherwise pass by measuring nothing.
        let (panel_left, panel_right) = panel_edges(width);
        let mut overflow: Vec<String> = Vec::new();
        if (viewport - width).abs() > EPS {
            overflow.push(format!(
                "the window is {viewport:.1}px wide after being resized to {width}px, so this \
                 width's judgements are about a different window than the one they name."
            ));
        }
        for ((k, row), (cell_id, cell)) in &cells {
            let header_label = headers
                .get(k)
                .and_then(|h| h.displayed_text.as_deref())
                .unwrap_or("?")
                .to_string();
            for (id, info) in snapshot.descendants_of(cell_id) {
                if info.x < cell.x - EPS || right(&info) > right(cell) + EPS {
                    overflow.push(format!(
                        "at {width}px, column {k} ({header_label:?}) row {row:?} paints {id} at \
                         x={:.1}..{:.1}, outside its cell's {:.1}..{:.1} — what spills is painted \
                         over the next columns and neither can be read.",
                        info.x,
                        right(&info),
                        cell.x,
                        right(cell)
                    ));
                }
                if info.x < panel_left - EPS || right(&info) > panel_right + EPS {
                    overflow.push(format!(
                        "at {width}px, {id} paints at x={:.1}..{:.1}, past the modal panel's \
                         {panel_left:.1}..{panel_right:.1} — it is cut off at the modal's border.",
                        info.x,
                        right(&info)
                    ));
                }
            }
        }

        Phase {
            width,
            overflow,
            report,
            origin: snapshot
                .by_id
                .get(ORIGIN_ELEMENT)
                .cloned()
                .filter(ElementInfo::has_visible_area),
            hosts: snapshot
                .by_id
                .get(HOSTS_ELEMENT)
                .cloned()
                .filter(ElementInfo::has_visible_area),
            cell: cells
                .into_iter()
                .find(|((k, row), _)| *k == 0 && row.contains(SUBJECT))
                .map(|(_, (_, cell))| cell),
        }
    };

    // 2a: what the dogfood pass actually saw — a long path above a short host.
    //     Run for its geometry in the log and for the overflow it judges, which
    //     the phase collects itself; nothing below compares against it.
    phase(WINDOW_W, ORIGIN, SHORT_VALUE);
    // 2b: the same host with nothing long above it. Whatever width it paints
    //     here is the width that host NEEDS, measured by the same text system
    //     in the same cell in the same run.
    let short_over_short = phase(WINDOW_W, SHORT_VALUE, SHORT_VALUE);
    // 3: a refused connection stores no hosts at all, and the cell still
    //    painted the bare label `calls` with empty space after it. The label is
    //    a literal in the template and registers no element of its own, so what
    //    a window can read is the cell's HEIGHT: a line carrying no value must
    //    cost no line.
    let no_hosts = phase(WINDOW_W, SHORT_VALUE, "");

    // ── The width sweep ────────────────────────────────────────────────────
    // The desktop width above is where the dogfood pass ran; it is not where a
    // cell budget breaks. Both disclosure values were read eliding to nothing at
    // the app's own floor, so the same row is re-measured at every width the app
    // can open at, in the two configurations the claims below are about.
    let swept: Vec<(Phase, Phase)> = WIDTHS
        .iter()
        .map(|w| {
            let with_hosts = phase(*w, SHORT_VALUE, SHORT_VALUE);
            let without_hosts = phase(*w, SHORT_VALUE, "");
            (with_hosts, without_hosts)
        })
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
    let (panel_left, panel_right) = panel_edges(WINDOW_W);
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

    // ── (3) A host the column has room for is painted WHOLE ────────────────
    // MEASURED, and the reported defect does not reproduce at this width: with
    // `127.0.0.1` in it the hosts line paints 45.0px inside a 108px cell, whole
    // and with ~35px to spare, whether or not a long path sits on the line
    // above it. The 2026-09-12 re-check read `calls 127.0.` off a screenshot at
    // the same logical width; nothing in this harness reproduces that cut. What
    // DOES reproduce is the same values eliding to nothing at 300, and that is
    // the column collapse, judged across the width sweep by
    // `settings_integrations_table_fits_windowed`.
    //
    // So the claim kept here is the standing one: the cell has room to spare
    // for a host this short. It is what would go red if the column's budget
    // were ever spent elsewhere.
    let need = short_over_short.hosts_or_panic("the reference phase");
    let cell = short_over_short.cell_or_panic();
    assert!(
        right(&need) <= right(&cell) - SLACK_PX,
        "with {SHORT_VALUE:?} on both lines the Integration cell must have at least \
         {SLACK_PX:.0}px to spare — the hosts line ends at {:.1} and the cell at {:.1}. A cell \
         this tight is one glyph from cutting a host, and a host cut short names nothing.{}",
        right(&need),
        right(&cell),
        short_over_short.report
    );

    // ── (4) No value, no line ──────────────────────────────────────────────
    assert!(
        no_hosts.origin.is_some(),
        "precondition: the no-hosts phase must still paint the origin line, else the row is not \
         an introduced one at all and the claim below is free.{}",
        no_hosts.report
    );
    let with_hosts_h = cell.height;
    let without_hosts_h = no_hosts.cell_or_panic().height;
    assert!(
        without_hosts_h <= with_hosts_h - LINE_COST_PX,
        "the connection stores no hosts and its Integration cell is still {without_hosts_h:.1}px \
         tall against {with_hosts_h:.1}px for the same row WITH a host — so the `calls` line is \
         still there, holding a label and nothing else. What the user reads is the bare word \
         `calls` followed by empty space: a label promising a value that never comes. `if_col` \
         already drops the whole disclosure block when the origin is empty; the hosts line owes \
         the same.{}",
        no_hosts.report
    );

    // ── (5) Every width, not just the desktop one ──────────────────────────
    let spills: Vec<&String> = swept
        .iter()
        .flat_map(|(a, b)| a.overflow.iter().chain(b.overflow.iter()))
        .collect();
    assert!(
        spills.is_empty(),
        "the introduced row does not stay inside its column at every swept width: {} \
         violation(s) over the {} widths {:?}.\n\n{}",
        spills.len(),
        WIDTHS.len(),
        WIDTHS,
        spills
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );

    // The sweep really swept: the Integration column's budget must actually
    // change across the widths. If every width reported the same cell, (5)
    // judged one layout six times.
    //
    // Below the table's wrap threshold the header row breaks and the subject
    // row scrolls out of the panel, so a narrow width legitimately paints no
    // Integration cell at all. That is why "it swept" is a claim about the
    // sweep as a whole rather than about its narrow end.
    let cell_widths: Vec<(f32, f32)> = swept
        .iter()
        .filter_map(|(with_hosts, _)| {
            with_hosts
                .cell
                .as_ref()
                .map(|c| (with_hosts.width, c.width))
        })
        .collect();
    assert!(
        cell_widths.len() >= 2,
        "only {} of the {} swept widths painted the subject row's Integration cell, so (5) and \
         (6) judged at most one layout. Cells per width: {:?}",
        cell_widths.len(),
        WIDTHS.len(),
        cell_widths
    );
    let narrowest = cell_widths
        .iter()
        .map(|(_, w)| *w)
        .fold(f32::INFINITY, f32::min);
    let widest = cell_widths
        .iter()
        .map(|(_, w)| *w)
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        widest > narrowest + EPS,
        "the Integration cell is {narrowest:.1}px wide at every width that painted it, so no \
         width in the sweep changed the column's budget and (5) judged one layout \
         {} times. Cells per width: {cell_widths:?}",
        cell_widths.len()
    );

    // ── (6) `no value, no line` holds at every width that paints the line ──
    // Below the wrap threshold the Integration cell paints the provider name
    // alone — measured here, the disclosure block is absent at 300, 360, 480
    // and 640, and the cell is 15px tall with a host and 15px tall without one.
    // "A line carrying no value must cost no line" has nothing to say where
    // there is no line either way, so the claim is judged where the WITH-host
    // configuration actually paints one.
    let mut judged = 0usize;
    for (with_hosts, without_hosts) in &swept {
        let (Some(tall), Some(short)) = (
            with_hosts.cell.as_ref().map(|c| c.height),
            without_hosts.cell.as_ref().map(|c| c.height),
        ) else {
            continue;
        };
        if with_hosts.hosts.is_none() {
            continue;
        }
        judged += 1;
        let w = with_hosts.width;
        assert!(
            short <= tall - LINE_COST_PX,
            "at {w}px the connection stores no hosts and its Integration cell is still \
             {short:.1}px tall against {tall:.1}px for the same row WITH a host, so the bare \
             `calls` label is still painted with nothing after it.{}",
            without_hosts.report
        );
    }
    assert!(
        judged >= 2,
        "only {judged} of the {} swept widths painted a hosts line at all, so `no value, no \
         line` was judged at almost no width.",
        WIDTHS.len()
    );
}

/// One configuration of the subject row, as painted.
struct Phase {
    /// The viewport this configuration was measured at, so a red names the
    /// window it is about.
    width: f32,
    /// Everything painted outside its cell or outside the modal panel at this
    /// width.
    overflow: Vec<String>,
    report: String,
    origin: Option<ElementInfo>,
    hosts: Option<ElementInfo>,
    cell: Option<ElementInfo>,
}

impl Phase {
    fn hosts_or_panic(&self, which: &str) -> ElementInfo {
        self.hosts.clone().unwrap_or_else(|| {
            panic!(
                "{which} painted no visible {HOSTS_ELEMENT}, so the comparison judges nothing.{}",
                self.report
            )
        })
    }

    fn cell_or_panic(&self) -> ElementInfo {
        self.cell.clone().unwrap_or_else(|| {
            panic!(
                "the subject row painted no visible Integration cell.{}",
                self.report
            )
        })
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
