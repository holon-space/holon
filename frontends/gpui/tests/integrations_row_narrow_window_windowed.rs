//! A long integration name must NOT push the status glyph out of its row.
//!
//! The sidebar is a fixed-width column (260px) whatever the window does, so a
//! long `display_name` over-constrains the row: icon + name + elastic spacer +
//! an 18px glyph box, in less width than the name alone wants. A flex item's
//! automatic minimum is its MIN-CONTENT width, so a label that refuses to
//! shrink takes the space it wants and the glyph — `flex_shrink_0`, because a
//! squashed glyph box would break the alignment column — is pushed off the
//! row's right edge. On screen the status simply vanishes for exactly the rows
//! whose names are longest.
//!
//! The fix is one branch in the gpui `text` builder (`min_w(0)` + `truncate()`
//! for column-bound labels). This rung is what makes that branch undeletable:
//! the verifier removed it and every other gate stayed green.
//!
//! TWO claims:
//!   1. containment — the glyph's box stays inside the row's box;
//!   2. the name YIELDED, and visibly: it paints narrower than the same string
//!      needs unclipped. The `…` itself is a paint attribute (`TextOverflow`),
//!      invisible to `BoundsRegistry`, so the rung pins the CLIP and the
//!      ellipsis rides on the same `truncate()` call that produces it.
//!
//! The unclipped width is measured IN THIS RUN, from a short-named row in the
//! same window and font, rather than hardcoded — a px-per-char constant would
//! be a second, silently rotting model of the text system.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! integrations_row_narrow_window_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind harness
//! @pbt covers integrations-row-long-name-keeps-status-in-row
//! @pbt slips-if-removed the status glyph silently leaves the row for the
//! longest-named integrations, and every other rung stays green because they
//! all use names that happen to fit

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
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

/// Narrow, but still wide enough that the left sidebar stays DOCKED — the
/// hazard is the sidebar's own fixed width against a long name, not the
/// window's, and an overlay drawer would just add a click to open.
const NARROW_WINDOW: &str = "800x900";

/// The row under test and the row that supplies the font scale.
const LONG_PROVIDER: &str = "claude-history";
const SHORT_PROVIDER: &str = "gcal";

/// Long enough to over-constrain a 260px sidebar row several times over. The
/// rung asserts that it DOES (no vacuous pass) before judging anything.
const LONG_NAME: &str = "Claude History Of Every Session Ever Recorded On This Machine";

/// Layout rounding, not a stagger.
const EDGE_TOLERANCE_PX: f32 = 0.5;

fn symbol_id(provider: &str) -> String {
    format!("text-integration:{provider}-status")
}

fn name_id(provider: &str) -> String {
    format!("text-integration:{provider}-display_name")
}

fn row_id(provider: &str) -> String {
    format!("selectable-integration:{provider}")
}

fn visible(bounds: &BoundsRegistry, id: &str) -> ElementInfo {
    bounds
        .element_info(id)
        .filter(ElementInfo::has_visible_area)
        .unwrap_or_else(|| {
            panic!(
                "{id} is not painted. This rung needs the Integrations rows on screen; if the \
                 sidebar became an overlay drawer at {NARROW_WINDOW}, the rung must open it (or \
                 widen the window) rather than skip."
            )
        })
}

fn main_test(bounds: &BoundsRegistry) {
    let row = visible(bounds, &row_id(LONG_PROVIDER));
    let name = visible(bounds, &name_id(LONG_PROVIDER));
    let short_name = visible(bounds, &name_id(SHORT_PROVIDER));

    // The glyph is looked up separately from `visible`: squeezed out of its
    // row it still REGISTERS, with a rect of no area, and "vanished" is the
    // defect itself rather than a broken precondition — so it gets its own
    // words instead of the missing-element message.
    let symbol = bounds
        .element_info(&symbol_id(LONG_PROVIDER))
        .unwrap_or_else(|| panic!("{} registered no element at all", symbol_id(LONG_PROVIDER)));
    assert!(
        symbol.has_visible_area(),
        "the status glyph paints NOTHING ({:.1}x{:.1} at x={:.1}) while its row is {:.1}px wide \
         and the name is {:.1}px: the label refused to shrink and squeezed the glyph out of the \
         row. For a user the status of this integration is simply gone. A column-bound label must \
         yield (min_w(0) + truncate() in the gpui `text` builder).",
        symbol.width,
        symbol.height,
        symbol.x,
        row.width,
        name.width,
    );

    let short_text = short_name
        .displayed_text
        .as_deref()
        .expect("the short row must paint its name");
    let px_per_char = short_name.width / short_text.chars().count() as f32;
    let unclipped = px_per_char * LONG_NAME.chars().count() as f32;

    let report = format!(
        "\nrow    x={:.1} w={:.1} (right {:.1})\nname   x={:.1} w={:.1} (right {:.1})\nsymbol \
         x={:.1} w={:.1} (right {:.1})\nscale  {px_per_char:.2}px/char from {short_text:?} \
         ({:.1}px) → {LONG_NAME:?} needs ~{unclipped:.0}px unclipped\n",
        row.x,
        row.width,
        row.x + row.width,
        name.x,
        name.width,
        name.x + name.width,
        symbol.x,
        symbol.width,
        symbol.x + symbol.width,
        short_name.width,
    );
    eprintln!("{report}");

    // Non-vacuity: the name must actually WANT more room than the row has,
    // otherwise nothing is being constrained and both claims below are free.
    assert!(
        unclipped > row.width,
        "this rung needs a name that over-constrains its row: {LONG_NAME:?} wants ~{unclipped:.0}px \
         and the row is {:.0}px wide. Lengthen LONG_NAME.{report}",
        row.width,
    );

    // 1. The glyph stays in the row.
    assert!(
        symbol.x + symbol.width <= row.x + row.width + EDGE_TOLERANCE_PX,
        "the status glyph runs {:.1}px past the row's right edge — for a user, the status of the \
         longest-named integration is simply not on screen. A column-bound label must yield \
         (min_w(0) + truncate() in the gpui `text` builder) instead of pushing its \
         siblings out.{report}",
        (symbol.x + symbol.width) - (row.x + row.width),
    );

    // 2. The name yielded, visibly.
    assert!(
        name.width < unclipped * 0.8,
        "the name painted {:.0}px wide, about what {LONG_NAME:?} needs unclipped (~{unclipped:.0}px) \
         — so it did not yield, it took the room. The glyph is only still inside the row by \
         luck.{report}",
        name.width,
    );
    assert!(
        name.x + name.width <= symbol.x + EDGE_TOLERANCE_PX,
        "the name overlaps the status glyph — clipping it to the row is not enough if it still \
         paints over its neighbour.{report}"
    );
}

#[test]
fn a_long_integration_name_does_not_push_the_status_out_of_its_row() {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test binary, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", NARROW_WINDOW) };

    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: as above.
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

    runtime.block_on(async {
        let db = backend.db_handle();
        db.execute_values("UPDATE integration_state SET enabled = 1", vec![])
            .await
            .expect("switch every mirrored integration on");
        // No bundled sidecar carries a name this long, and the defect only
        // shows on one that does — so the rung writes the case it is about.
        db.execute_values(
            &format!(
                "UPDATE integration_state SET display_name = '{LONG_NAME}' WHERE provider_name = \
                 '{LONG_PROVIDER}'"
            ),
            vec![],
        )
        .await
        .expect("give one integration a name that over-constrains its row");
    });

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
                "Holon-IntegrationsNarrowRow-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        main_test(&bounds);
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
