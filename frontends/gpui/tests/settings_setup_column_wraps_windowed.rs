//! The Settings → Integrations **Setup** column WRAPS: when its operation
//! buttons want more room than the column has, they continue on a second line
//! inside the cell instead of running out of it.
//!
//! Martin's ruling D120.a(2), 2026-09-12: the Setup column fits its buttons
//! today only because a weight budget was tuned to the three operations an
//! integration currently advertises. Any pressure that costs one button's worth
//! of width — a fourth operation, or the same three operations in a smaller
//! window — puts the last button outside the column again. A budget cannot
//! absorb that; a wrapping container can.
//!
//! The pressure this rung applies is the SMALL WINDOW, not a fabricated fourth
//! operation. Both squeeze the same axis, and only one of them is reachable
//! with the operations production actually registers: an integration row
//! advertises exactly three (`set_field`, `begin_oauth`, `open_default_view`,
//! `holon-app/src/integrations_operations.rs`), so a four-button fixture would
//! have to invent an operation and would then be judging a test double. The
//! modal is `w_full` capped at 640px, so a narrow window shrinks it, the
//! table's flex weights shrink with it, and the three REAL buttons no longer
//! fit on one line. `settings_integrations_table_fits_windowed` runs the same
//! table at 1512px, where they do — which is why it has never seen this.
//!
//! Only the FIRST provider row stays above the fold at this width, and the
//! bundled provider that sorts first authenticates with a static token, so it
//! advertises two operations rather than three. The rung therefore gives that
//! row a consent flow through the mirror — the same one-column write
//! `settings_introduced_row_fits_windowed` uses, and the exact shape `gcal`
//! carries — so the row on screen is the three-operation one. Written into the
//! mirror rather than by installing a sidecar because the mirror IS what the
//! Settings list reads.
//!
//! Three claims, all read off `BoundsRegistry`:
//!   1. every Setup operation button lies inside the cell that holds it, and
//!      inside the modal panel;
//!   2. no two buttons in one cell overlap — wrapping must move a button to a
//!      new line, not paint it on top of its neighbour;
//!   3. the wrap ENGAGED: the buttons of the pressured cell occupy at least two
//!      vertical bands.
//!
//! Non-vacuity is measured IN THIS RUN, never assumed: the rung first asserts
//! that the cell's buttons cannot all fit on one line (their own painted widths
//! plus the gaps exceed the cell's width). Without that, a window that happened
//! to be roomy would pass claims 1 and 2 while judging nothing.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_setup_column_wraps_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind harness
//! @pbt covers settings-setup-column-wraps — a Setup cell short of room
//! continues its operation buttons on a second line
//! @pbt slips-if-removed the last operation button of every integration row is
//! painted outside the Setup column on any window narrower than the desktop
//! default, and the only rung that judges this table runs at 1512px

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

/// The `op_button` builder's contract id. The registry also carries an
/// anonymous `op_button#NN` entry at the SAME rect for each button, so the
/// overlap claim must count each button once — this prefix is what identifies
/// the semantic one.
const OP_BUTTON_PREFIX: &str = "op-button-";

/// The column this rung judges, by its header text.
const SETUP_HEADER: &str = "Setup";

/// Narrow enough that the modal's Setup column no longer holds three operation
/// buttons on one line, and tall enough that the table's first rows stay above
/// the fold. The modal is `w_full` capped at 640px inside 16px of overlay
/// padding, so the panel — and every flex weight in the table — tracks this
/// width directly.
const WINDOW: &str = "560x900";
const WINDOW_W: f32 = 560.0;
const WINDOW_H: f32 = 900.0;

/// The Integrations table is the LAST section of the Settings modal, so its
/// rows sit against the panel's bottom edge and a wrapped second line lands
/// outside the clip. The rung scrolls the modal the way a user does until the
/// subject row is painted whole; the cap turns "the modal does not scroll" into
/// a failure instead of a hang.
const WHEEL_DY: f32 = 60.0;
const MAX_NOTCHES: usize = 20;

/// `modal_overlay` (`lib.rs`): 16px overlay padding around a `w_full` panel
/// capped at 640px, with 24px of its own padding.
const MODAL_OVERLAY_PAD: f32 = 16.0;
const MODAL_MAX_W: f32 = 640.0;

/// Sub-pixel layout rounding.
const EPS: f32 = 1.0;

/// The row the window keeps above the fold: the bundled provider that sorts
/// first. `SETTINGS_SQL` orders by `provider_name` ASC.
const SUBJECT: &str = "claude-history";

/// The gap between Setup buttons, from the section template's
/// `list(#{… gap: 8})`. Used only to state the non-vacuity bar; the widths
/// themselves are measured.
const OP_GAP: f32 = 8.0;

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

/// Where the wheel gesture is aimed: the middle of the window, which is inside
/// the centred modal panel and outside every other scrollable surface.
fn modal_center() -> Point<Pixels> {
    Point {
        x: Pixels::from(WINDOW_W / 2.0),
        y: Pixels::from(WINDOW_H / 2.0),
    }
}

/// One wheel notch at `at`, preceded by the pointer move that makes the hitbox
/// under it the one gpui offers the scroll to. gpui gates a div's wheel
/// handling on the hitbox being HOVERED (`Interactivity::paint` →
/// `should_handle_scroll`), so a bare `ScrollWheelEvent` is dropped in silence.
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

fn right(info: &ElementInfo) -> f32 {
    info.x + info.width
}

fn bottom(info: &ElementInfo) -> f32 {
    info.y + info.height
}

/// Do two painted rects share any area? Sub-pixel touching is not an overlap.
fn overlaps(a: &ElementInfo, b: &ElementInfo) -> bool {
    a.x < right(b) - EPS && b.x < right(a) - EPS && a.y < bottom(b) - EPS && b.y < bottom(a) - EPS
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
        self.all
            .iter()
            .filter_map(|(id, info)| {
                let k = id.strip_prefix(HEADER_PREFIX)?.parse::<usize>().ok()?;
                info.has_visible_area().then(|| (k, info.clone()))
            })
            .collect()
    }

    /// Every VISIBLE `table-cell-col-{k}-{row}` of column `k`. The Settings
    /// list overdraws rows just outside its viewport; those register a
    /// degenerate rect and carry no judgeable geometry.
    fn cells_of_column(&self, k: usize) -> BTreeMap<String, (String, ElementInfo)> {
        let prefix = format!("{CELL_PREFIX}{k}-");
        self.all
            .iter()
            .filter_map(|(id, info)| {
                let row = id.strip_prefix(&prefix)?;
                info.has_visible_area()
                    .then(|| (row.to_string(), (id.clone(), info.clone())))
            })
            .collect()
    }

    /// The `op-button-*` elements whose tracked-parent chain reaches
    /// `cell_id` — one entry per button, in paint order.
    fn buttons_under(&self, cell_id: &str) -> Vec<(String, ElementInfo)> {
        self.all
            .iter()
            .filter(|(id, info)| {
                id.starts_with(OP_BUTTON_PREFIX)
                    && info.has_visible_area()
                    && self.is_under(id, cell_id)
            })
            .cloned()
            .collect()
    }

    fn is_under(&self, id: &str, ancestor: &str) -> bool {
        let mut cur = id.to_string();
        for _ in 0..64 {
            let Some(info) = self.by_id.get(&cur) else {
                return false;
            };
            let Some(parent) = info.parent_id.as_deref() else {
                return false;
            };
            if parent == ancestor {
                return true;
            }
            cur = parent.to_string();
        }
        false
    }
}

/// Numbers, not booleans: the whole Setup column, so a red log shows which
/// button leaves which cell and by how much.
fn setup_report(
    notches: usize,
    header: &ElementInfo,
    cells: &BTreeMap<String, (String, ElementInfo)>,
    buttons: &BTreeMap<String, Vec<(String, ElementInfo)>>,
) -> String {
    let mut s =
        format!("\n=== settings Setup column geometry (after {notches} wheel notches) ===\n");
    s.push_str(&format!(
        "header {:?} x={:.1}..{:.1} (w={:.1})\n",
        header.displayed_text.as_deref().unwrap_or("?"),
        header.x,
        right(header),
        header.width
    ));
    for (row, (_, cell)) in cells {
        let bs = &buttons[row];
        let needed: f32 = bs.iter().map(|(_, i)| i.width).sum::<f32>()
            + OP_GAP * (bs.len().saturating_sub(1) as f32);
        s.push_str(&format!(
            "  cell row {row:?}: x={:.1}..{:.1} y={:.1}..{:.1} (w={:.1}) — {} buttons needing \
             {needed:.1}px on one line\n",
            cell.x,
            right(cell),
            cell.y,
            bottom(cell),
            cell.width,
            bs.len()
        ));
        for (id, info) in bs {
            s.push_str(&format!(
                "      x={:7.1}..{:7.1} y={:7.1}..{:7.1} text={:?} id={id}\n",
                info.x,
                right(info),
                info.y,
                bottom(info),
                info.displayed_text.as_deref().unwrap_or("")
            ));
        }
    }
    s
}

#[test]
fn the_setup_columns_operation_buttons_wrap_instead_of_leaving_the_column() {
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
                "Holon-SetupColumnWraps-Windowed",
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

    // Give the first row a consent flow, after the modal is already open, so the
    // update reaches the screen through the section's own `live_query` — the
    // path a real projection uses. `begin_oauth` declares a relation guard over
    // exactly these three columns, so this is what makes the row offer its
    // third operation.
    runtime.block_on(async {
        db.execute_values(
            "UPDATE integration_state SET configurable = 1, config_status = 'unconfigured', \
             configure_progress = '' WHERE provider_name = ?",
            vec![holon_api::Value::String(SUBJECT.to_string())],
        )
        .await
        .expect("the mirror must carry the consent-flow columns");
    });
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Bring the subject row onto the screen. The Integrations table is the last
    // section of the modal, so its first row sits against the panel's bottom
    // edge — where a cell that grew a second line is clipped and the button on
    // that line registers no rect at all. Scrolling is what makes the wrapped
    // and the overflowing state comparable.
    let setup_column_of = |p: &Painted| -> Option<usize> {
        p.headers()
            .iter()
            .find_map(|(k, h)| (h.displayed_text.as_deref() == Some(SETUP_HEADER)).then_some(*k))
    };
    let subject_row = format!("integration:{SUBJECT}");
    let subject_whole = |p: &Painted| -> bool {
        setup_column_of(p).is_some_and(|k| {
            p.cells_of_column(k)
                .get(&subject_row)
                .is_some_and(|(cell_id, _)| p.buttons_under(cell_id).len() >= 3)
        })
    };
    let mut painted = Painted::snapshot(&bounds);
    let mut notches = 0usize;
    while !subject_whole(&painted) && notches < MAX_NOTCHES {
        wheel(&mut app, window, modal_center(), -WHEEL_DY);
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(5));
        notches += 1;
        painted = Painted::snapshot(&bounds);
    }

    let headers = painted.headers();
    let setup_col = headers.iter().find_map(|(k, h)| {
        (h.displayed_text.as_deref() == Some(SETUP_HEADER)).then(|| (*k, h.clone()))
    });
    let cells = setup_col
        .as_ref()
        .map(|(k, _)| painted.cells_of_column(*k))
        .unwrap_or_default();
    let buttons: BTreeMap<String, Vec<(String, ElementInfo)>> = cells
        .iter()
        .map(|(row, (cell_id, _))| (row.clone(), painted.buttons_under(cell_id)))
        .collect();
    let report = setup_col
        .as_ref()
        .map(|(_, h)| setup_report(notches, h, &cells, &buttons))
        .unwrap_or_else(|| {
            format!(
                "\n=== settings Setup column geometry ===\nno {SETUP_HEADER:?} header painted; \
                 headers were {:?}\n",
                headers
                    .iter()
                    .map(|(k, h)| (*k, h.displayed_text.as_deref().unwrap_or("?").to_string()))
                    .collect::<Vec<_>>()
            )
        });

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    // Numbers on every run, not only on a red: how much room a passing Setup
    // cell has left is what says whether the next operation still fits.
    eprintln!("{report}");

    // ── Non-vacuity ────────────────────────────────────────────────────────
    let (_, header) = setup_col.unwrap_or_else(|| {
        panic!(
            "the open Settings modal must paint a {SETUP_HEADER:?} header, else every assertion \
             below judges nothing.{report}"
        )
    });
    let rows: BTreeSet<&String> = cells.keys().collect();
    assert!(
        !rows.is_empty(),
        "no visible Setup cell was painted, so this rung judges nothing.{report}"
    );

    // The pressured cells: the ones whose own buttons cannot share one line.
    // Measured from what was painted, never from a px-per-glyph model.
    let pressured: Vec<&String> = cells
        .iter()
        .filter(|(row, (_, cell))| {
            let bs = &buttons[*row];
            bs.len() >= 2
                && bs.iter().map(|(_, i)| i.width).sum::<f32>() + OP_GAP * (bs.len() - 1) as f32
                    > cell.width + EPS
        })
        .map(|(row, _)| row)
        .collect();
    assert!(
        !pressured.is_empty(),
        "no Setup cell in this window is short of room — every row's buttons fit on one line, so \
         claims 1-3 would pass without wrapping anything. Narrow {WINDOW:?} further, or the \
         column's flex weight has grown.{report}"
    );

    // ── (1) Every Setup button stays inside its cell and inside the panel ──
    let panel_w = (WINDOW_W - 2.0 * MODAL_OVERLAY_PAD).min(MODAL_MAX_W);
    let panel_left = (WINDOW_W - panel_w) / 2.0;
    let panel_right = panel_left + panel_w;
    assert!(
        header.x >= panel_left - EPS && right(&header) <= panel_right + EPS,
        "the {SETUP_HEADER:?} header sits at x={:.1}..{:.1}, outside the modal panel's \
         {panel_left:.1}..{panel_right:.1} reconstructed from `modal_overlay`. Fix this rung's \
         reconstruction before reading the claims below.{report}",
        header.x,
        right(&header)
    );
    for (row, (_, cell)) in &cells {
        for (id, info) in &buttons[row] {
            assert!(
                info.x >= cell.x - EPS && right(info) <= right(cell) + EPS,
                "Setup row {row:?} paints {id} at x={:.1}..{:.1}, outside its cell's \
                 {:.1}..{:.1}. The buttons are laid on one line whatever the column's width, so \
                 the last one is painted over the modal's border and cannot be clicked.{report}",
                info.x,
                right(info),
                cell.x,
                right(cell)
            );
            assert!(
                info.x >= panel_left - EPS && right(info) <= panel_right + EPS,
                "Setup row {row:?} paints {id} at x={:.1}..{:.1}, past the modal panel's \
                 {panel_left:.1}..{panel_right:.1}.{report}",
                info.x,
                right(info)
            );
        }
    }

    // ── (2) No two buttons of a cell overlap ───────────────────────────────
    // A container that wraps must MOVE a button to a new line. One that clamps
    // its children instead would satisfy claim 1 by stacking them on the same
    // spot, which reads as a single button.
    for (row, (_, _)) in &cells {
        let bs = &buttons[row];
        for i in 0..bs.len() {
            for j in (i + 1)..bs.len() {
                assert!(
                    !overlaps(&bs[i].1, &bs[j].1),
                    "Setup row {row:?} paints {} and {} on top of each other — one is hidden \
                     behind the other and only one of the two operations can be clicked.{report}",
                    bs[i].0,
                    bs[j].0
                );
            }
        }
    }

    // ── (3) The wrap engaged ───────────────────────────────────────────────
    // Claims 1 and 2 also hold for a cell that simply dropped a button. This is
    // what says the missing width was answered with a second line.
    for row in pressured {
        let bands: BTreeSet<i32> = buttons[row]
            .iter()
            .map(|(_, i)| (i.y / 4.0).round() as i32)
            .collect();
        assert!(
            bands.len() >= 2,
            "Setup row {row:?} cannot fit its buttons on one line, yet they all share one — the \
             cell is not wrapping, it is overflowing.{report}"
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
