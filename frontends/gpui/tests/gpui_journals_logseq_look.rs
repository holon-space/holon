//! LogSeq-look journals feed — the batch windowed PBT for the three-part
//! restyle: (REQ1) the date renders as a HEADING, (REQ2) each day-page section
//! has a MIN-HEIGHT floor, (REQ3) each day shows an empty BULLET to write into,
//! including a truly EMPTY day.
//!
//! Windowed, not headless: the feed's per-day content is a `live_query`
//! deferred to the platform layer that never materialises in the headless
//! keystone snapshot, and the heading/geometry are paint properties the
//! headless snapshot drops. This harness renders the real feed in a
//! `HeadlessAppContext` window and reads the PAINTED tree (`rendered_elements`)
//! — the streaming creation slot is absent from `widget_tree_snapshot` (a sync
//! re-snapshot), so the bullet must be read from geometry.
//!
//! RED-BY-INVERSION (holon-feature red-first, proven by reverting each change):
//!  - REQ1: revert `text(col("content"), #{style:"h2"})` in the
//!    `embedded_page_expanded` header → day headers paint at body height → the
//!    `>= H2_MIN_HEIGHT` assertion fails.
//!  - REQ2: revert `column(#{min_height:220}, expand_toggle(...))` in the same
//!    variant → day sections collapse to ~105px → the `>= FLOOR_MIN` assertion
//!    fails.
//!  - REQ3: revert `creation_slot:true, virtual_parent:true` on the day content
//!    tree → NO `:__virtual:` slot paints → the bullet assertions fail. The
//!    EMPTY-day bullet additionally needs the `resolve_creation_parent`
//!    explicit-empty affordance: without it, only non-empty days get a slot and
//!    the empty-day assertion fails.
//!
//! DETERMINISM: the feed default-expands the days it draws, but which days the
//! window materialises varies (viewport-lazy / expand-state timing). The
//! assertions are therefore COUNT-scoped to the days that ARE materialised (at
//! least 2), plus the empty-day case checked explicitly — never "every day",
//! which would chase nondeterminism. See the vacuity guards.
//!
//! ⚠ Run with `--test-threads=1`: the gpui test platform holds thread-local
//! state and two windowed tests in one process abort.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::PlatformTextSystem;
use holon_api::EntityUri;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::row_origin::RowOrigin;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::builders::window_wide;
use holon_integration_tests::pbt::window_slice::seed::JOURNALS_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_empty_journal_day;
use holon_integration_tests::pbt::window_slice::seed::graft_journal_days;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutLayout;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

/// A day header wider than this is an h2 HEADING, not body text. Height cannot
/// discriminate — text elements carry a `min_h` floor, so both `style:"h2"`
/// (22px font) and body (14px) paint ~26px tall. WIDTH does: the same 10-char
/// date renders ~129px at h2 and ~80px at body. The threshold sits between so a
/// reverted style goes red. Read on the EMPTY day, whose header is unclipped
/// (a non-empty day's header can be width-clipped by its content column, which
/// hides the h2 advance even though the style is applied — same variant, same
/// `style:"h2"`, per the `journal_feed` matview's blanket `expand_default=1`).
const H2_MIN_WIDTH: f32 = 100.0;

/// The authored `min_height` floor, minus a px of slack for sub-pixel layout.
/// A floored day-section column paints >= this; an unfloored one is ~105px.
const FLOOR_MIN: f32 = 218.0;

/// The `min_height` value authored in `block_profile.yaml`.
const AUTHORED_FLOOR: f32 = 220.0;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

fn settle(
    app: &mut HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable = 0u32;
    while start.elapsed() < timeout {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let count = bounds.all_elements().len();
        let loading = bounds
            .all_elements()
            .iter()
            .any(|(_, i)| i.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !loading {
            stable += 1;
            if stable >= 5 {
                return;
            }
        } else {
            stable = 0;
        }
        last_count = count;
    }
}

/// The tallest painted header text for `day` (the main-panel h2 header, not the
/// body-height sidebar row of the same page). `None` if the day drew no header.
fn header_height(elements: &[RenderedElement], day: &EntityUri, content: &str) -> Option<f32> {
    elements
        .iter()
        .filter(|e| {
            e.entity_id.as_ref() == Some(day)
                && e.widget_type == "text"
                && e.displayed_text.as_deref() == Some(content)
        })
        .map(|e| e.height)
        .reduce(f32::max)
}

/// The widest painted header text for `day` — the h2 advance when unclipped.
fn header_width(elements: &[RenderedElement], day: &EntityUri, content: &str) -> Option<f32> {
    elements
        .iter()
        .filter(|e| {
            e.entity_id.as_ref() == Some(day)
                && e.widget_type == "text"
                && e.displayed_text.as_deref() == Some(content)
        })
        .map(|e| e.width)
        .reduce(f32::max)
}

/// Is `day`'s OWN section column floored to `>= FLOOR_MIN`? Tied precisely by
/// walking the paint tree UP from the day's header to its first enclosing
/// column — the `min_height` wrapper. A geometric "contains y" test would be
/// fooled by the tall main-panel column that encloses EVERY day's header, so it
/// would pass even with no floor; the parent chain pins the day's own section.
fn day_section_floored(elements: &[RenderedElement], day: &EntityUri, content: &str) -> bool {
    let by_id: std::collections::HashMap<&str, &RenderedElement> =
        elements.iter().map(|e| (e.el_id.as_str(), e)).collect();
    let header = elements.iter().find(|e| {
        e.entity_id.as_ref() == Some(day)
            && e.widget_type == "text"
            && e.displayed_text.as_deref() == Some(content)
    });
    let Some(header) = header else { return false };
    let mut cur = header;
    for _ in 0..16 {
        let Some(pid) = cur.parent_id.as_ref() else {
            return false;
        };
        let Some(parent) = by_id.get(pid.as_str()).copied() else {
            return false;
        };
        if parent.widget_type == "column" || parent.el_id.starts_with("column") {
            // The FIRST enclosing column is the day's section (the `min_height`
            // wrapper when present); its height is the floor verdict.
            return parent.height >= FLOOR_MIN;
        }
        cur = parent;
    }
    false
}

/// Is `day`'s empty-bullet creation slot (`block:__virtual:<day>`) painted?
fn slot_painted(elements: &[RenderedElement], day: &EntityUri) -> bool {
    let slot = RowOrigin::creation_placeholder_id(day);
    elements
        .iter()
        .any(|e| e.entity_id.as_ref().map(EntityUri::as_str) == Some(slot.as_str()))
}

#[test]
fn journals_logseq_look_heading_floor_and_empty_bullet() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env.reactive_engine.get().cloned().expect("reactive engine");
    let debug_services = env.debug_services().cloned().expect("debug services");
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
                Some(debug_services.clone()),
                None,
                "Holon-Journals-LogSeq",
                cx,
            )
        })
        .expect("window opened");
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Three days with a block each, plus one truly EMPTY day (no blocks) — the
    // fresh-journal case whose bullet needs the explicit-empty affordance.
    let mut feed: Vec<(String, String)> = runtime
        .block_on(graft_journal_days(&env, 3))
        .expect("graft days");
    let empty_day = runtime
        .block_on(graft_empty_journal_day(&env, "jday-empty", "2026-02-02"))
        .expect("graft empty day");
    feed.push(empty_day.clone());
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(120)));
    settle(&mut app, &bounds, &runtime, Duration::from_secs(120));

    // Click the journals page in the sidebar to focus the Main panel on the
    // feed (the click's bound `navigation.focus` is what writes `focus_roots`).
    let journals = EntityUri::block(JOURNALS_ID);
    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx")
        .clone();
    let app_ptr: *const HeadlessAppContext = &app;
    let driver = SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );
    runtime
        .block_on(async { driver.click_entity(&journals, "left_sidebar").await })
        .expect("click journals");
    settle(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let sut = window_wide(Box::new(bounds.clone()), engine.clone());
    let elements = runtime.block_on(async { sut.rendered_elements().await });

    // ── Classify the feed days by what the frame actually painted ────────────
    let materialized: Vec<&(String, String)> = feed
        .iter()
        .filter(|(id, content)| header_height(&elements, &EntityUri::block(id), content).is_some())
        .collect();
    let floored_days: Vec<&(String, String)> = materialized
        .iter()
        .copied()
        .filter(|(id, content)| day_section_floored(&elements, &EntityUri::block(id), content))
        .collect();
    let slotted_nonempty: Vec<&(String, String)> = feed[..3]
        .iter()
        .filter(|(id, _)| slot_painted(&elements, &EntityUri::block(id)))
        .collect();

    // The widest header any feed day painted. Only an UNCLIPPED h2 header
    // reaches h2 width; an empty day (no content column to clip it) reliably
    // provides one, but the assertion takes the max across days so it does not
    // pin to which day the frame left unclipped.
    let max_hdr_w = feed
        .iter()
        .filter_map(|(id, content)| header_width(&elements, &EntityUri::block(id), content))
        .fold(0.0f32, f32::max);
    let empty = EntityUri::block(&empty_day.0);

    eprintln!(
        "[journals-logseq] materialized={} floored={} slotted_nonempty={} max_hdr_w={max_hdr_w:.0} \
         empty_slot={} tallest_floor_col={:.0}",
        materialized.len(),
        floored_days.len(),
        slotted_nonempty.len(),
        slot_painted(&elements, &empty),
        elements
            .iter()
            .filter(|e| e.widget_type == "column" || e.el_id.starts_with("column"))
            .map(|e| e.height)
            .fold(0.0f32, f32::max),
    );

    // ── Vacuity guard ────────────────────────────────────────────────────────
    assert!(
        materialized.len() >= 2,
        "the feed must materialise at least 2 day pages for these assertions to mean anything — \
         only {} painted a header",
        materialized.len(),
    );

    // ── REQ1: the date renders as a HEADING ──────────────────────────────────
    assert!(
        max_hdr_w >= H2_MIN_WIDTH,
        "REQ1 heading: at least one feed day's date must paint at h2 width (>= {H2_MIN_WIDTH}px, \
         `style:\"h2\"`); the widest was {max_hdr_w:.0}px — a reverted style paints every date at \
         body width (~80px for a 10-char date)",
    );

    // ── REQ2: each day-page section has a MIN-HEIGHT floor ───────────────────
    // Which specific days floor varies with viewport-lazy materialisation, so
    // this counts floored sections rather than pinning to one day.
    assert!(
        floored_days.len() >= 2,
        "REQ2 min-height: at least 2 materialised day sections must paint a column floored to \
         >= {FLOOR_MIN}px ({AUTHORED_FLOOR}px authored); only {} of {} were — without the \
         `min_height` column a short day collapses to ~105px",
        floored_days.len(),
        materialized.len(),
    );

    // ── REQ3: LogSeq-faithful empty bullet — EMPTY day only ──────────────────
    // Martin's JRN-2 ruling: the trailing `block:__virtual:<day>` "type here"
    // bullet affords ONLY on an EMPTY day (a fresh journal, nothing written yet);
    // a NON-empty day shows no redundant trailing bullet (you add via its rows).
    // Two hard assertions pin this from both sides, and together they are
    // deterministic despite viewport-lazy materialisation:
    //   (a) the EMPTY day paints its bullet — the streaming provider must
    //       converge it onto the settled frame (its zero-row inner stream still
    //       drives the append; the atomic recompose in `AppendedRowsProvider`
    //       guarantees it). This is also the red-first for the resolve fix.
    //   (b) NO materialised NON-empty day paints a bullet — pins empty-only AND
    //       catches the dogfood-#3 last-item paint issue backwards (a non-empty
    //       day must not emit a trailing slot into the virtualized list at all).
    assert!(
        slot_painted(&elements, &empty),
        "REQ3 (a) empty-day bullet: the EMPTY day {} must paint its `block:__virtual:<day>` \
         bullet so a fresh journal can be written into — it did not. Needs the explicit-\
         `virtual_parent` empty affordance in `resolve_creation_parent` AND the atomic streaming \
         recompose that converges it onto the settled frame",
        empty_day.0,
    );
    let stray: Vec<&str> = feed[..3]
        .iter()
        .filter(|(id, _)| slot_painted(&elements, &EntityUri::block(id)))
        .map(|(id, _)| id.as_str())
        .collect();
    assert!(
        stray.is_empty(),
        "REQ3 (b) empty-only: NON-empty days must paint NO trailing bullet (LogSeq-faithful), but \
         {stray:?} did. `resolve_creation_parent` must resolve an explicit container to `None` \
         when the rowset is non-empty",
    );

    // Clean shutdown, then leak the app/env: dropping the gpui HeadlessApp's
    // entity map on the test thread panics in gpui internals (entity_map.rs) —
    // the shared teardown every windowed rung here uses.
    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}
