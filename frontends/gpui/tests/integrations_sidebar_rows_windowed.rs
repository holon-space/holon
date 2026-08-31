//! The left sidebar's Integrations rows, in a REAL window over a REAL booted
//! engine: each enabled integration paints its status as a SYMBOL, the symbols
//! line up down a column, and the row is clickable.
//!
//! The section is a `live_query` whose item template only the production shell
//! interprets against delivered rows — a ViewModel snapshot expands nothing —
//! so this is the tier that can answer "what does the user actually see in that
//! list, and where".
//!
//! Five claims, each with its own failure mode:
//!   1. every enabled row paints the glyph for the status the mirror holds (the
//!      status word is gone from the row, so a wrong glyph is now the only way
//!      the state can be misread);
//!   2. the glyphs share one x across rows whose NAMES differ in length — the
//!      "invisible tabular" claim, which content-sized layout gets wrong for
//!      free;
//!   3. the row carries a click affordance at all. `selectable` registers no
//!      element when its action wiring is empty, so the painted
//!      `selectable-integration:<p>` IS the proof the action survived from the
//!      seeded template into the window;
//!   4. the row wears the integration's icon and the name a PERSON reads, and
//!      the technical name appears nowhere in it;
//!   5. clicking the row opens that integration's default view in the main
//!      panel — the whole point of making the row clickable, and the only claim
//!      here that exercises the op end to end.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! integrations_sidebar_rows_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind harness
//! @pbt covers integrations-sidebar-row-symbol-and-alignment
//! @pbt slips-if-removed the discovery list silently reverts to a ragged
//! status column, or to an inert row, while every headless rung stays green
//! because the template string still says otherwise

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

/// One integration as the mirror holds it, which is the oracle for what its row
/// must paint.
struct MirrorRow {
    /// The technical name — the sidecar's file name, the row's id, and the
    /// string the row must NEVER show.
    provider: String,
    /// The name a person reads.
    name: String,
    status: String,
}

/// The provider whose sidecar carries the full presentation set — a display
/// name unlike its technical name, an icon, and a `default_view`. R1 and R3 are
/// about THIS row; the claims above sweep every row.
const CLAUDE_HISTORY: &str = "claude-history";
const CLAUDE_HISTORY_DISPLAY: &str = "Claude History";
/// The page `claude-history.yaml` names as its `default_view`, scheme-prefixed
/// as `navigation.focus` stores it.
const CLAUDE_HISTORY_VIEW: &str = "block:claude-history-view";

/// Sub-pixel layout rounding is allowed; anything a reader could see as a
/// stagger is not.
const MAX_COLUMN_STAGGER_PX: f32 = 0.5;

/// The status words `IntegrationStatus::label()` writes. Spread across the rows
/// so each row's glyph has to come from ITS row.
const STATUS_WORDS: [&str; 4] = ["Connected", "Pending", "Needs auth", "Unavailable"];

/// The tracked element a row's status symbol registers under — `text` binds its
/// el_id to the row and the column it reads.
fn symbol_id(provider: &str) -> String {
    format!("text-integration:{provider}-status")
}

/// The row's name text, under the same scheme.
fn name_id(provider: &str) -> String {
    format!("text-integration:{provider}-display_name")
}

/// Every painted element inside `root_id`'s subtree, found by walking the
/// tracked parent chain. The row's `text`/`icon` children carry no row binding
/// of their own — only the `selectable` wrapper does — so containment is what
/// joins them to their row.
fn descendants_of(bounds: &BoundsRegistry, root_id: &str) -> Vec<(String, ElementInfo)> {
    let all = bounds.all_elements();
    let parent: std::collections::HashMap<String, String> = all
        .iter()
        .filter_map(|(id, info)| Some((id.clone(), info.parent_id.as_ref()?.to_string())))
        .collect();
    all.into_iter()
        .filter(|(id, _)| {
            let mut cursor = id.as_str();
            // The chain is a few levels deep (selectable > row > text); the
            // bound only stops a cycle from hanging the rung.
            for _ in 0..32 {
                match parent.get(cursor) {
                    Some(p) if p == root_id => return true,
                    Some(p) => cursor = p.as_str(),
                    None => return false,
                }
            }
            false
        })
        .collect()
}

fn subtree_inventory(bounds: &BoundsRegistry, root_id: &str) -> String {
    let mut out = format!("\n=== painted inside {root_id} ===\n");
    let mut els = descendants_of(bounds, root_id);
    els.sort_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap());
    for (id, info) in els {
        let tag = info.vm_node.as_ref().map(|n| n.tag.as_ref()).unwrap_or("?");
        out.push_str(&format!(
            "  tag={tag:<10} x={:7.1} w={:6.1} text={:?} id={id}\n",
            info.x, info.width, info.displayed_text,
        ));
    }
    out
}

/// The click affordance `selectable` registers — present ONLY when the row
/// carries a non-empty action wiring.
fn selectable_id(provider: &str) -> String {
    format!("selectable-integration:{provider}")
}

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

fn visible(bounds: &BoundsRegistry, id: &str) -> Option<ElementInfo> {
    bounds
        .element_info(id)
        .filter(ElementInfo::has_visible_area)
}

fn main_test(bounds: &BoundsRegistry, mirror: &[MirrorRow]) {
    let mut inventory = String::from("\n=== sidebar integration rows ===\n");
    for row in mirror {
        let sym = bounds.element_info(&symbol_id(&row.provider));
        let name = bounds.element_info(&name_id(&row.provider));
        inventory.push_str(&format!(
            "  {:<16} status={:<12} name={:?} symbol={:?}\n",
            row.provider,
            row.status,
            name.as_ref()
                .map(|i| (i.x, i.width, i.displayed_text.clone())),
            sym.as_ref()
                .map(|i| (i.x, i.width, i.displayed_text.clone())),
        ));
    }
    eprintln!("{inventory}");

    // 1. The glyph matches the status the mirror holds, for every row.
    let mut painted: Vec<(&MirrorRow, ElementInfo)> = Vec::new();
    for row in mirror {
        let Some(info) = visible(bounds, &symbol_id(&row.provider)) else {
            continue;
        };
        let (want_glyph, _) = holon_frontend::shadow_builders::status_symbol_and_color(&row.status)
            .unwrap_or_else(|| panic!("no glyph for status {:?}{inventory}", row.status));
        let got = info.displayed_text.as_deref().unwrap_or_else(|| {
            panic!(
                "{}'s status symbol painted no text{inventory}",
                row.provider
            )
        });
        assert_eq!(
            got, want_glyph,
            "{} is {:?} in the mirror, so its row must paint {want_glyph:?} — it painted \
             {got:?}. The status WORD no longer appears in the row, so a wrong glyph is the \
             whole of what the user reads.{inventory}",
            row.provider, row.status,
        );
        painted.push((row, info));
    }

    // Teeth for claim 1: with one glyph on screen, a row painting a constant
    // would satisfy every comparison above.
    let mut glyphs: Vec<&str> = painted
        .iter()
        .filter_map(|(_, i)| i.displayed_text.as_deref())
        .collect();
    glyphs.sort_unstable();
    glyphs.dedup();
    assert!(
        glyphs.len() >= 2,
        "the rows painted one distinct glyph ({glyphs:?}); the check above cannot tell a row \
         reading its own status from one painting a constant{inventory}"
    );

    // Teeth: without two rows whose names differ in length, a ragged column
    // would pass the alignment check below by coincidence.
    let shortest = painted.iter().map(|(r, _)| r.name.len()).min().unwrap_or(0);
    let longest = painted.iter().map(|(r, _)| r.name.len()).max().unwrap_or(0);
    assert!(
        painted.len() >= 2 && longest > shortest,
        "this rung needs at least two painted rows with DIFFERENT name lengths to have teeth; \
         got {} row(s), name lengths {shortest}..{longest}{inventory}",
        painted.len(),
    );

    // 2. One x for the whole column.
    let (first_row, first) = &painted[0];
    for (row, info) in &painted {
        let dx = (info.x - first.x).abs();
        assert!(
            dx <= MAX_COLUMN_STAGGER_PX,
            "{}'s status symbol sits {dx:.2}px off {}'s (x {:.2} vs {:.2}) — the column is \
             ragged, so the list reads as a pile of lines instead of a table. Name lengths \
             differ ({} vs {}), which is exactly what content-sized layout staggers.{inventory}",
            row.provider,
            first_row.provider,
            info.x,
            first.x,
            row.name.len(),
            first_row.name.len(),
        );
    }

    // The symbols must be to the RIGHT of their own row's name — otherwise the
    // check above could be passing over an overlapped, degenerate layout.
    for (row, info) in &painted {
        let name = visible(bounds, &name_id(&row.provider)).unwrap_or_else(|| {
            panic!(
                "{}'s row paints a status symbol but no name{inventory}",
                row.provider
            )
        });
        assert!(
            info.x >= name.x + name.width,
            "{}'s status symbol (x {:.1}) must sit right of its name (x {:.1} w {:.1}){inventory}",
            row.provider,
            info.x,
            name.x,
            name.width,
        );
    }

    // 3. The row is clickable.
    for (row, _) in &painted {
        assert!(
            visible(bounds, &selectable_id(&row.provider)).is_some(),
            "{}'s row painted no `selectable` element. `selectable` registers one only when its \
             action wiring is non-empty, so this says the row's click action did not survive \
             from the seeded template into the window.{inventory}",
            row.provider,
        );
    }
}

/// R1: the row wears the integration's ICON and the name a person reads — and
/// never the technical one.
///
/// `claude-history` is the row that can tell the two names apart: its sidecar
/// sets `display_name: "Claude History"`, so a row still bound to
/// `provider_name` paints a string that differs in every character.
fn the_row_shows_the_human_name_and_an_icon(bounds: &BoundsRegistry) {
    let root = selectable_id(CLAUDE_HISTORY);
    let inventory = subtree_inventory(bounds, &root);
    let subtree = descendants_of(bounds, &root);
    assert!(
        !subtree.is_empty(),
        "nothing painted inside {root} — the row did not render, so every claim below would be \
         vacuous{inventory}"
    );

    // The icon. It carries no row binding of its own (a props-only widget with
    // no data row), so containment in the row's subtree is what makes it THIS
    // row's icon.
    assert!(
        subtree.iter().any(|(_, info)| {
            info.vm_node
                .as_ref()
                .is_some_and(|n| n.tag.as_ref() == "icon")
                && info.has_visible_area()
        }),
        "the Claude History row painted no icon. The sidecar sets `icon: robot`, and the row \
         template reads `icon(col(\"icon\"))`{inventory}"
    );

    // The human name.
    let name = visible(bounds, &name_id(CLAUDE_HISTORY)).unwrap_or_else(|| {
        panic!("the Claude History row painted no display-name text{inventory}")
    });
    assert_eq!(
        name.displayed_text.as_deref(),
        Some(CLAUDE_HISTORY_DISPLAY),
        "the row must show the sidecar's display_name{inventory}"
    );

    // And nowhere the technical one. Asserted over the WHOLE row subtree rather
    // than the one text element: the point is that the string a person should
    // never see is absent from the row, not merely absent from one slot.
    for (id, info) in &subtree {
        let Some(text) = info.displayed_text.as_deref() else {
            continue;
        };
        assert!(
            !text.contains(CLAUDE_HISTORY),
            "the row paints the TECHNICAL name {CLAUDE_HISTORY:?} in {id} ({text:?}). That \
             string is the sidecar's file name and the row's id; the sidebar shows \
             {CLAUDE_HISTORY_DISPLAY:?}{inventory}"
        );
    }
}

#[test]
fn integration_rows_show_name_icon_aligned_status_and_open_their_view() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary, set before the app boots.
    unsafe { std::env::set_var("HOME", home.path()) };

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle.session.clone().expect("full_headless -> session");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> reactive engine");
    let backend = bundle
        .engine
        .clone()
        .expect("full_headless -> backend engine");

    // The discovery list shows the integrations that are switched ON, so the
    // rung switches them all on and judges the list it gets. Reading the mirror
    // back (rather than hardcoding the bundle) keeps the oracle true when the
    // bundled providers change.
    let mirror: Vec<MirrorRow> = runtime.block_on(async {
        let db = backend.db_handle();
        db.execute_values("UPDATE integration_state SET enabled = 1", vec![])
            .await
            .expect("switch every mirrored integration on");
        // Every provider boots `Pending`, and a rung that saw one status word
        // could not tell a row reading its OWN status from one painting a
        // constant. Spread the four words across the rows first.
        for (i, status) in STATUS_WORDS.iter().enumerate() {
            db.execute_values(
                &format!(
                    "UPDATE integration_state SET status = '{status}' WHERE provider_name IN \
                     (SELECT provider_name FROM integration_state ORDER BY provider_name ASC \
                      LIMIT 1 OFFSET {i})"
                ),
                vec![],
            )
            .await
            .expect("spread the status words across the rows");
        }
        db.query(
            "SELECT provider_name, display_name, status FROM integration_state WHERE enabled = 1 \
             ORDER BY display_name ASC",
            Default::default(),
        )
        .await
        .expect("read the rows the sidebar section queries")
        .iter()
        .map(|r| {
            let field = |name: &str| {
                r.get(name)
                    .and_then(|v| v.as_string())
                    .unwrap_or_else(|| panic!("integration_state.{name}"))
                    .to_string()
            };
            MirrorRow {
                provider: field("provider_name"),
                name: field("display_name"),
                status: field("status"),
            }
        })
        .collect()
    });
    assert!(
        !mirror.is_empty(),
        "no enabled integrations in the mirror — the sidebar section would be empty and every \
         assertion below vacuous"
    );

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
                "Holon-IntegrationsSidebarRows-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");
    let window = rebind.window();

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // What ONE row contains first, then the cross-row geometry: a row
        // missing its icon or its name should say so in those words, rather
        // than surfacing as a column check that cannot find something to
        // measure.
        the_row_shows_the_human_name_and_an_icon(&bounds);
        main_test(&bounds, &mirror);

        // R3: the click opens the integration's default view in the main panel.
        //
        // Read through the production join — the main panel's own source — so
        // the oracle is the row the panel renders from, not a private notion of
        // focus.
        let focus_of_main = || -> String {
            runtime.block_on(async {
                backend
                    .db_handle()
                    .query(
                        "SELECT fr.root_id FROM focus_roots fr JOIN navigation_cursor nc ON \
                         nc.region = fr.region AND nc.history_id = fr.history_id WHERE fr.region \
                         = 'main'",
                        Default::default(),
                    )
                    .await
                    .expect("read the main region's focus root")
                    .first()
                    .and_then(|r| r.get("root_id"))
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            })
        };

        let before = focus_of_main();
        assert_ne!(
            before, CLAUDE_HISTORY_VIEW,
            "main already shows the Claude History view BEFORE the click — the assertion after \
             it would pass without the click doing anything"
        );

        let row = visible(&bounds, &selectable_id(CLAUDE_HISTORY))
            .expect("the Claude History row must be painted to be clicked");
        click_at(&mut app, window, center_of(&row), "the Claude History row");
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

        let after = focus_of_main();
        assert_eq!(
            after, CLAUDE_HISTORY_VIEW,
            "clicking the Claude History row must open its default view in main. Main was \
             {before:?} before the click and is {after:?} now. The row dispatches \
             `integration.open_default_view(id)`, which resolves \
             `integration_state.default_view` ({CLAUDE_HISTORY_VIEW}) and focuses it."
        );
    }));

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(rebind);
        app.update(|cx| cx.shutdown());
        app.run_until_parked();
    }));
    std::mem::forget(app);
    std::mem::forget(bundle);

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
