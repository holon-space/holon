//! Windowed regression for "the selected search hit's subtitle is illegible"
//! (dogfood P1, bugfunnel
//! `2026-09-03-search-overlay-selected-row-subtitle-is-illegible`).
//!
//! The block id under a hit's label is the only thing separating two hits with
//! the same label, so it is body text and owes WCAG AA's 4.5:1 against the
//! surface it lands on. On the selected row that surface is the accent fill,
//! not the panel background.
//!
//! Measured from the layout record's painted colours (`ElementInfo::painted_fg`
//! / `painted_bg`), which the GPUI tracker cascades exactly as the frontend
//! cascades them — so the subtitle reports the row fill it inherits rather than
//! whatever the panel declares.
//!
//! Rung: a TestPlatform window over a real engine. The colour a widget resolves
//! for itself is a render-time decision; no headless tier makes it.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::theme::BODY_TEXT_CONTRAST_FLOOR;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::RebindHandle;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::search_ui::hit_row_id;
use holon_gpui::search_ui::hit_subtitle_id;
use holon_gpui::window_key_bindings;
use holon_integration_tests::pbt::window_slice::seed::CHORD_TARGET_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_chord_decoy_row;
use holon_integration_tests::pbt::window_slice::seed::graft_chord_target_row;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Same cross-runtime fixed-point settle the other TestPlatform tests use.
fn settle(
    app: &mut HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let count = bounds.all_elements().len();
        let still_loading = bounds
            .all_elements()
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                break;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    runtime.block_on(async { tokio::task::yield_now().await });
    app.run_until_parked();
    bounds.flush();
}

/// The chord the app publishes for `action`, in the wire vocabulary
/// `send_key_chord` speaks.
fn published_chord(action: &str) -> KeyChord {
    let row = window_key_bindings()
        .into_iter()
        .find(|r| r.action == action)
        .unwrap_or_else(|| panic!("no window chord published for {action:?}"));
    let keys: Vec<Key> = row
        .chord
        .split('-')
        .map(|seg| {
            seg.parse::<Key>()
                .unwrap_or_else(|e| panic!("chord {:?} segment {seg:?}: {e}", row.chord))
        })
        .collect();
    KeyChord::new(&keys)
}

/// One booted TestPlatform window over a real engine, with the chord-target row
/// grafted and painted. `app` is boxed because `SimUserDriver` keeps a raw
/// pointer to it — the fixture must be movable without moving the app.
struct Fixture {
    app: Box<HeadlessAppContext>,
    runtime: Arc<tokio::runtime::Runtime>,
    engine: Arc<ReactiveEngine>,
    bounds: BoundsRegistry,
    driver: Arc<SimUserDriver>,
    rebind: RebindHandle,
    target: EntityUri,
    target_element: String,
    root_id: EntityUri,
    /// Owns the temp vault + backend for the window's lifetime.
    _env: TestEnvironment,
}

impl Fixture {
    fn boot(window_name: &str) -> Self {
        let text_system = real_text_system();
        let assets: Arc<dyn AssetSource> = Arc::new(());
        let mut app = Box::new(HeadlessAppContext::with_platform(
            text_system,
            assets,
            gpui_platform::current_headless_renderer,
        ));

        let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
        let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
        runtime.block_on(async { env.start_app(true).await.expect("start_app") });
        runtime
            .block_on(graft_chord_target_row(&env))
            .expect("graft the chord target row");
        runtime
            .block_on(graft_chord_decoy_row(&env))
            .expect("graft the chord decoy row");

        let session = env.session_arc();
        let engine = env
            .reactive_engine
            .get()
            .cloned()
            .expect("reactive engine after start_app");
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
                    window_name,
                    cx,
                )
            })
            .expect("window opened");

        let target_element = format!("block:{CHORD_TARGET_ID}");
        let boot_deadline = Instant::now() + Duration::from_secs(180);
        let mut painted = false;
        while Instant::now() < boot_deadline {
            settle(&mut app, &bounds, &runtime, Duration::from_secs(30));
            painted = bounds
                .all_elements()
                .iter()
                .any(|(_, info)| info.entity_id.as_deref() == Some(target_element.as_str()));
            if painted {
                break;
            }
        }
        assert!(
            painted,
            "boot precondition: the target row must paint before quick-open is opened"
        );

        let interaction_tx = debug_services
            .interaction_tx
            .get()
            .expect("interaction_tx set by the window interaction pump")
            .clone();
        // SAFETY: `app` is boxed and outlives the driver, and stays on this
        // (gpui) thread.
        let driver = Arc::new(SimUserDriver::new(
            &*app,
            rebind.window(),
            bounds.clone(),
            engine.clone(),
            runtime.handle().clone(),
            interaction_tx,
        ));

        // ALLOW(entity_uri_from_raw): the seed grafts this bare id; schemed here.
        let target = EntityUri::from_raw(CHORD_TARGET_ID);
        let root_id = holon_api::root_layout_block_uri();
        Self {
            app,
            runtime,
            engine,
            bounds,
            driver,
            rebind,
            target,
            target_element,
            root_id,
            _env: env,
        }
    }

    fn settle(&mut self) {
        settle(
            &mut self.app,
            &self.bounds,
            &self.runtime,
            Duration::from_secs(30),
        );
    }

    /// Seat both focus authorities on the chord-target row and prove it.
    fn focus_target_row(&mut self) {
        self.engine.set_focus_with_caret(self.target.clone(), 0);
        self.settle();
        let focused: Vec<String> = self
            .bounds
            .all_elements()
            .into_iter()
            .filter(|(_, info)| info.focused == Some(true))
            .filter_map(|(_, info)| info.entity_id.as_deref().map(str::to_string))
            .collect();
        assert_eq!(
            focused,
            vec![self.target_element.clone()],
            "baseline: the focused row's editor must hold window focus before cmd+k"
        );
    }

    /// Press the published `open_search` chord into the focused row.
    fn open_quick_open_from_row(&mut self) {
        let chord = published_chord("open_search");
        let root_tree = self.engine.snapshot_reactive(&self.root_id);
        self.runtime
            .block_on(self.driver.send_key_chord(
                &self.root_id,
                &root_tree,
                &self.target,
                &chord,
                Default::default(),
            ))
            .expect("cmd+k pressed into the focused row");
        self.settle();
        let rebind = &self.rebind;
        assert!(
            self.app.update(|cx| rebind.search_modal_open(cx)),
            "cmd+k must open the quick-open modal"
        );
    }

    fn type_query(&mut self, query: &str) {
        for ch in query.chars() {
            self.runtime
                .block_on(self.driver.send_raw_keystroke(&ch.to_string(), &[]))
                .expect("query characters must reach the overlay's input");
        }
        self.settle();
    }

    fn element(&self, id: &str) -> ElementInfo {
        self.bounds.element_info(id).unwrap_or_else(|| {
            panic!(
                "no layout record for {id:?}; recorded search elements: {:?}",
                self.bounds
                    .all_elements()
                    .into_iter()
                    .filter(|(k, _)| k.starts_with("search-hit-"))
                    .map(|(k, _)| k)
                    .collect::<Vec<_>>()
            )
        })
    }

    /// Move the pointer to the centre of a tracked element and settle.
    fn hover(&mut self, id: &str) {
        let info = self.element(id);
        self.driver
            .hover_point(info.x + info.width / 2.0, info.y + info.height / 2.0);
        self.settle();
    }

    fn shutdown(mut self) {
        drop(self.driver);
        drop(self.rebind);
        self.app.update(|cx| cx.shutdown());
        self.app.run_until_parked();
        std::mem::forget(self.app);
        std::mem::forget(self._env);
    }
}

/// The selected hit's block id must be readable ON the selection fill.
#[test]
fn selected_hit_subtitle_clears_the_body_text_contrast_floor() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenContrast");
    f.focus_target_row();
    f.open_quick_open_from_row();
    f.type_query("chord");

    // Selection starts at the first flattened hit and no arrow key moved it.
    let row = f.element(&hit_row_id(0));
    let subtitle = f.element(&hit_subtitle_id(0));

    let fill = row
        .painted_bg
        .expect("the hit row declares its own fill, so the record must carry it");
    assert_eq!(
        subtitle.painted_bg,
        Some(fill),
        "the subtitle declares no fill of its own, so it must inherit the row's"
    );

    if let Some(unselected) = f.bounds.element_info(&hit_row_id(1)) {
        assert_ne!(
            unselected.painted_bg,
            Some(fill),
            "row 0 painted the same fill as row 1, so the selection is not on row 0 and this \
             test is measuring the wrong row"
        );
    }

    let ratio = subtitle
        .text_contrast()
        .expect("the subtitle records both a text colour and the fill under it");
    assert!(
        ratio >= BODY_TEXT_CONTRAST_FLOOR,
        "the selected hit's block id paints {ratio:.2}:1 against the selection fill \
         (text {:?} on {fill:?}), below the {BODY_TEXT_CONTRAST_FLOOR}:1 floor for body text — \
         it is the only thing telling two same-label hits apart",
        subtitle.painted_fg,
    );

    f.shutdown();
}

/// The pointer fills a row with the same accent the keyboard selection uses, so
/// "carries the selection fill" is ONE state for colour: a hovered row owes the
/// floor exactly as the selected one does.
#[test]
fn hovered_hit_subtitle_clears_the_body_text_contrast_floor() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenHoverContrast");
    f.focus_target_row();
    f.open_quick_open_from_row();
    f.type_query("chord");

    let fill = f
        .element(&hit_row_id(0))
        .painted_bg
        .expect("the selected hit row declares its own fill, so the record must carry it");
    assert_ne!(
        f.element(&hit_row_id(1)).painted_bg,
        Some(fill),
        "precondition: row 1 must be unselected before the pointer reaches it"
    );

    f.hover(&hit_row_id(1));

    let row = f.element(&hit_row_id(1));
    assert_eq!(
        row.painted_bg,
        Some(fill),
        "hovering an unselected hit paints it with the selection fill, so the layout record \
         must report that fill — a hover the record cannot see is a contrast hole no test closes"
    );
    let subtitle = f.element(&hit_subtitle_id(1));
    assert_eq!(
        subtitle.painted_bg,
        Some(fill),
        "the subtitle declares no fill of its own, so it must inherit the hovered row's"
    );

    let ratio = subtitle
        .text_contrast()
        .expect("the subtitle records both a text colour and the fill under it");
    assert!(
        ratio >= BODY_TEXT_CONTRAST_FLOOR,
        "the hovered hit's block id paints {ratio:.2}:1 against the selection fill \
         (text {:?} on {fill:?}), below the {BODY_TEXT_CONTRAST_FLOOR}:1 floor for body text",
        subtitle.painted_fg,
    );

    f.shutdown();
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
