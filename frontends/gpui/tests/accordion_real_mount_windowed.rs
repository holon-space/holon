//! Windowed rungs for the SHIPPED "Linked references" accordion, driven through
//! the same boot the app performs: a real `TestEnvironment`, the seeded
//! `assets/default/index.org` layout, `launch_holon_window_rebindable` at a
//! chosen `HOLON_INITIAL_WINDOW_SIZE`, and backlinks that exist in the store
//! before the window opens.
//!
//!   A1 rows:      every row the backlinks query returns is painted, and the
//!                 region grows to hold them, capped at `max_height_fraction`.
//!   A2 frame one: below `ACCORDION_MIN_EXPANDED_WIDTH_PX` the section is
//!                 collapsed on the FIRST painted frame — no navigation.
//!   A3 header:    the header's `icon` prop reaches the icon builder instead of
//!                 being painted as its own name.
//!
//! `accordion_sizes_to_content_windowed` interprets its panel itself and hands
//! the interpreter an `available_space` that production does not have yet at
//! boot, so neither the mount seam nor frame one is observable there.
//!
//! Run: `cargo nextest run -p holon-gpui --features holon-gpui/pbt
//! --test accordion_real_mount_windowed --no-fail-fast`

use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::PAGE_TAG;
use holon_api::Value;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// Above both layout breakpoints — the width the dogfood pass called wide.
const DESKTOP_WINDOW: &str = "1200x900";
const DESKTOP_HEIGHT_PX: f32 = 900.0;
/// Narrower than `ACCORDION_MIN_EXPANDED_WIDTH_PX` (600).
const PHONE_WINDOW: &str = "560x850";

/// The seed's cap on the section (`assets/default/index.org`).
const MAX_HEIGHT_FRACTION: f32 = 0.33;
/// Layout slack on a `relative(f)` cap, matching `accordion_bounded_pbt`.
const EPS: f32 = 2.0;

/// The page the Main region is focused on, and the link target of every
/// reference block below.
const TARGET_PAGE_ID: &str = "accordion-refs-page";
const TARGET_PAGE_TITLE: &str = "Accordion Refs Page";
/// Reference blocks the backlinks query must return.
const REF_COUNT: usize = 4;

fn ref_id(i: usize) -> String {
    format!("accordion-ref-{i}")
}

/// Every element the frame painted for the reference blocks. They hang off
/// `no_parent`, so they are not in the focused page's outline: anything painted
/// for them was painted by the Linked-references section.
fn painted_ref_rows(bounds: &BoundsRegistry) -> Vec<ElementInfo> {
    let ids: Vec<String> = (1..=REF_COUNT)
        .map(|i| EntityUri::block(&ref_id(i)).to_string())
        .collect();
    let mut rows: Vec<ElementInfo> = bounds
        .all_elements()
        .into_iter()
        .filter(|(_, i)| i.height > 0.0)
        .filter(|(_, i)| {
            i.entity_id
                .as_deref()
                .is_some_and(|e| ids.iter().any(|want| want == e))
        })
        .map(|(_, i)| i)
        .collect();
    rows.sort_by(|a, b| a.y.total_cmp(&b.y));
    rows.dedup_by(|a, b| a.entity_id == b.entity_id);
    rows
}

/// Height of the tagged accordion region (0.0 when nothing is tagged).
fn region_height(bounds: &BoundsRegistry) -> f32 {
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, i)| i.widget_type.as_ref() == "accordion")
        .map(|(_, i)| i.height)
        .fold(0.0f32, f32::max)
}

/// What the frame painted, for a failure message that separates "the section
/// never rendered" from "it rendered too little".
fn census(bounds: &BoundsRegistry) -> String {
    let mut tags: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (_, info) in bounds.all_elements() {
        *tags.entry(info.widget_type.to_string()).or_default() += 1;
    }
    let rows: Vec<String> = painted_ref_rows(bounds)
        .iter()
        .map(|i| {
            format!(
                "{}@y{:.0}h{:.0}",
                i.entity_id.as_deref().unwrap_or("?"),
                i.y,
                i.height
            )
        })
        .collect();
    format!(
        "accordion region {:.0}px; ref rows [{}]; widgets {:?}",
        region_height(bounds),
        rows.join(", "),
        tags
    )
}

/// Seed the store, THEN open the window: every rung below reads a frame that
/// the app reached on its own, with no navigation performed against the window.
fn with_seeded_window(window_size: &str, run: impl FnOnce(&BoundsRegistry)) {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", window_size) };

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

    runtime.block_on(async {
        let mut page: holon_api::StorageEntity = std::collections::HashMap::new();
        page.insert(
            "id".into(),
            Value::String(EntityUri::block(TARGET_PAGE_ID).to_string()),
        );
        page.insert(
            "parent_id".into(),
            Value::String(EntityUri::no_parent().as_str().to_string()),
        );
        page.insert("content".into(), Value::String(TARGET_PAGE_TITLE.into()));
        page.insert("content_type".into(), ContentType::Text.into());
        page.insert(
            "tags".into(),
            Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
        );
        env.test_ctx()
            .execute_op("block", "create", page)
            .await
            .expect("create the page the references point at");

        for i in 1..=REF_COUNT {
            env.create_block(
                &ref_id(i),
                EntityUri::no_parent().as_str(),
                &format!("[[{TARGET_PAGE_TITLE}]] reference {i}"),
            )
            .await
            .unwrap_or_else(|e| panic!("create reference block {i}: {e}"));
        }

        let mut focus: holon_api::StorageEntity = std::collections::HashMap::new();
        focus.insert("region".into(), Value::from(holon_api::Region::Main));
        focus.insert(
            "block_id".into(),
            Value::String(EntityUri::block(TARGET_PAGE_ID).to_string()),
        );
        env.test_ctx()
            .execute_op("navigation", "focus", focus)
            .await
            .expect("focus Main on the page before the window opens");
    });
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    let bounds = BoundsRegistry::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                NavigationState::new(),
                bounds.clone(),
                Some(debug_services.clone()),
                None,
                "Holon-Accordion-Mount",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    run(&bounds);

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

/// A1 — the section paints what its query returns.
///
/// The seeded query returns one row per block linking to the focused page. A
/// section that paints fewer rows than that, and cannot be scrolled to the
/// rest, has silently dropped the reader's data.
#[test]
fn every_backlink_the_query_returns_is_painted() {
    with_seeded_window(DESKTOP_WINDOW, |bounds| {
        let rows = painted_ref_rows(bounds);
        let region_h = region_height(bounds);
        let cap = MAX_HEIGHT_FRACTION * DESKTOP_HEIGHT_PX;

        assert_eq!(
            rows.len(),
            REF_COUNT,
            "A1 rows VIOLATED: {REF_COUNT} blocks link to the focused page but the \
             Linked-references section painted {}; {}",
            rows.len(),
            census(bounds)
        );

        let span = rows
            .last()
            .map(|last| last.y + last.height - rows[0].y)
            .unwrap_or(0.0);
        assert!(
            region_h >= span,
            "A1 grows VIOLATED: the section is {region_h}px tall but its rows span \
             {span}px, so it is clipping its own content; {}",
            census(bounds)
        );
        assert!(
            region_h <= cap + EPS,
            "A1 capped VIOLATED: the section is {region_h}px tall against its \
             max_height_fraction cap of {cap}px"
        );
    });
}

/// A2 — the width rule applies to the frame the reader actually sees first.
///
/// The collapse default is derived from measured width, which the interpreter
/// does not have until the window publishes a viewport. A section that opens
/// expanded and only collapses once something else re-interprets the tree has
/// applied the rule to every frame except the one that matters.
#[test]
fn a_narrow_first_frame_starts_collapsed() {
    with_seeded_window(PHONE_WINDOW, |bounds| {
        let rows = painted_ref_rows(bounds);
        assert!(
            rows.is_empty(),
            "A2 frame-one VIOLATED: at {PHONE_WINDOW} the section must open COLLAPSED, \
             but the first painted frame already shows {} backlink rows; {}",
            rows.len(),
            census(bounds)
        );
    });
}

/// A3 — the header's icon is an icon.
///
/// Every other header routes its icon name through the icon builder, which owns
/// the glyph lookup, the theme tint and the Android substitution table. A name
/// pasted into a `div` reaches the screen as the word itself and never enters
/// the icon-font coverage sweep.
#[test]
fn the_header_paints_the_icon_through_the_icon_builder() {
    with_seeded_window(DESKTOP_WINDOW, |bounds| {
        assert!(
            bounds
                .find_by_entity_id("accordion-icon:Linked references")
                .is_some(),
            "A3 header-icon VIOLATED: the section header's `icon` prop never reached \
             the icon builder, so the name is painted as text; {}",
            census(bounds)
        );
        let literal: Vec<String> = bounds
            .all_elements()
            .into_iter()
            .filter(|(_, i)| i.displayed_text.as_deref() == Some("link"))
            .map(|(id, _)| id)
            .collect();
        assert!(
            literal.is_empty(),
            "A3 header-icon VIOLATED: the icon NAME is on screen as text at {literal:?}"
        );
    });
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
