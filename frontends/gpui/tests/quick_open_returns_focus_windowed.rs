//! Windowed regression for "closing quick-open never returns keyboard focus"
//! (dogfood P1, bugfunnel `2026-09-03-closing-quick-open-never-returns-\
//! keyboard-focus`).
//!
//! `cmd+k` moves window focus to the modal's own text input. The
//! editor→window focus bridge (`editor_view::spawn_focus_binding`) is deduped
//! on the `focused_block` signal, and closing the overlay does not move that
//! signal — so unless `close` hands focus back, the window is left focusing a
//! widget that is no longer rendered while the engine still reports a focused
//! block. Every following keystroke is dropped.
//!
//! Every dismissal rung makes two independent assertions, so a fix that only
//! satisfies the oracle cannot pass:
//!   * `inv-window-focus-matches-engine-focus` is RUN against the live window
//!     (the composed windowed oracle for exactly this divergence) — against the
//!     live app that invariant reports `skipped` because the MCP snapshot hosts
//!     only `SutBackend`, so the windowed tier is the only place it can measure
//!     this sequence.
//!   * a typed character is dispatched and must be CONSUMED — the user-visible
//!     effect the driver reports as "dropped all N keystroke(s)".
//!
//! Three dismissal rungs, because `close` is the shared chokepoint but the
//! focus DESTINATION differs per path: Escape and click-away hand focus back
//! to the row that had it, Enter navigates away and the row unmounts.
//!
//! Rung: the windowed `SimUserDriver` (TestPlatform, real gpui action dispatch
//! and real focus handles). No headless rung reaches gpui window focus.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::RebindHandle;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::window_key_bindings;
use holon_integration_tests::pbt::composed::invariants::window_focus;
use holon_integration_tests::pbt::window_slice::builders::compose_windowed_sut;
use holon_integration_tests::pbt::window_slice::seed::CHORD_TARGET_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_chord_target_row;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::run_selected;
use holon_pbt_core::invariant::InvariantResult;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

const FOCUS_INVARIANT: &str = "inv-window-focus-matches-engine-focus";

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

/// Run ONLY the window/engine focus-coherence invariant over the live window's
/// composed `CapMap` and return its outcome. Panics only when the invariant is
/// DESELECTED — that would mean the windowed CapMap stopped supplying the caps
/// the oracle needs, which is a harness break, not a product verdict.
async fn focus_invariant_outcome(
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    driver: Arc<dyn UserDriver>,
) -> InvariantResult {
    let sut_caps = compose_windowed_sut(&ComponentSet::full_gpui(), geometry, engine, driver);
    // The invariant declares no reference capability — it compares the SUT
    // against itself — so an empty ref map selects it and nothing else.
    let registry = vec![window_focus::wire()];
    let report = run_selected(&registry, &sut_caps, &CapMap::new()).await;
    report
        .ran
        .iter()
        .find(|(id, _)| id.0 == FOCUS_INVARIANT)
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| {
            panic!(
                "{FOCUS_INVARIANT} was DESELECTED from the windowed CapMap (deselected: {:?})",
                report.deselected.iter().map(|d| d.0).collect::<Vec<_>>(),
            )
        })
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

    /// Which entities hold window focus in the committed frame, as the composed
    /// oracle reads them.
    fn window_focused(&self) -> Vec<String> {
        self.bounds
            .all_elements()
            .into_iter()
            .filter(|(_, info)| info.focused == Some(true))
            .filter_map(|(_, info)| info.entity_id.as_deref().map(str::to_string))
            .collect()
    }

    /// `(entity, widget_type, focused)` for every painted element that names an
    /// entity — the diagnostic a focus assertion needs to say WHY it failed.
    fn painted_entities(&self) -> Vec<(String, String, Option<bool>)> {
        self.bounds
            .all_elements()
            .into_iter()
            .filter_map(|(_, info)| {
                info.entity_id.as_deref().map(|e| {
                    (
                        e.to_string(),
                        info.widget_type.as_ref().to_string(),
                        info.focused,
                    )
                })
            })
            .collect()
    }

    fn modal_open(&mut self) -> bool {
        let rebind = &self.rebind;
        self.app.update(|cx| rebind.search_modal_open(cx))
    }

    /// Seat both focus authorities on the chord-target row and prove it.
    fn focus_target_row(&mut self) {
        self.engine.set_focus_with_caret(self.target.clone(), 0);
        self.settle();
        assert_eq!(
            self.window_focused(),
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
        assert!(self.modal_open(), "cmd+k must open the quick-open modal");
    }

    fn focus_invariant(&self, context: &str) -> InvariantResult {
        let outcome = self.runtime.block_on(focus_invariant_outcome(
            Box::new(self.bounds.clone()),
            self.engine.clone(),
            self.driver.clone() as Arc<dyn UserDriver>,
        ));
        assert!(
            !matches!(outcome, InvariantResult::Fail(_)),
            "[{context}] {FOCUS_INVARIANT}: {outcome:?}"
        );
        outcome
    }

    /// Whether the window consumes a raw character within `timeout`.
    fn keystroke_lands(&self, key: &str, timeout: Duration) -> bool {
        self.runtime
            .block_on(
                self.driver
                    .send_raw_keystroke_until_handled(key, &[], timeout),
            )
            .is_ok()
    }

    /// `(id, content)` of every child row of `parent`, in `sort_key` order.
    /// The birth oracle: navigation must add none, the first keystroke
    /// exactly one.
    fn children_of(&self, parent: &str) -> Vec<(String, String)> {
        let sql = format!(
            "SELECT id, content FROM block_raw WHERE parent_id = '{parent}' ORDER BY sort_key"
        );
        self.runtime
            .block_on(self._env.query_sql(&sql))
            .expect("read the destination's children")
            .iter()
            .map(|row| {
                let id = row
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("block_raw.id is text")
                    .to_string();
                let content = row
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string();
                (id, content)
            })
            .collect()
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

/// Escape: the overlay closes and the row that had the caret gets it back.
///
/// `Skipped` is not a pass on this path. Against the live app the oracle
/// reports `skipped` (the MCP snapshot hosts only `SutBackend`), which is how
/// the defect stayed invisible; the windowed tier must MEASURE it.
#[test]
fn closing_quick_open_hands_keyboard_focus_back() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenFocus");
    f.focus_target_row();
    f.open_quick_open_from_row();

    // Raw keystroke, not `send_key_chord`: the modal holds window focus, so a
    // chord press would block on the row's focus barrier instead of reaching
    // the overlay's key handler.
    f.runtime
        .block_on(f.driver.send_raw_keystroke("escape", &[]))
        .expect("escape pressed into the open overlay");
    f.settle();
    assert!(!f.modal_open(), "escape must close the quick-open modal");

    let outcome = f.focus_invariant("after escape closed quick-open");
    assert!(
        matches!(outcome, InvariantResult::Ok),
        "after escape {FOCUS_INVARIANT} must measure and pass, got {outcome:?}"
    );
    assert_eq!(
        f.window_focused(),
        vec![f.target_element.clone()],
        "after escape the previously-focused row's editor must hold window focus again"
    );
    assert!(
        f.keystroke_lands("x", Duration::from_secs(5)),
        "a character typed after escape must land in the restored editor"
    );
    f.shutdown();
}

/// Click-away: a real mouse-down outside the panel dismisses the overlay
/// through `on_mouse_down_out`, and it must hand focus back exactly like
/// Escape. Separate rung because the gesture reaches `close` through a
/// different handler.
#[test]
fn clicking_away_from_quick_open_hands_keyboard_focus_back() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenClickAway");
    f.focus_target_row();
    f.open_quick_open_from_row();

    // Top-left corner: the panel is centred, `max_w` 640 and `pt` 80, so this
    // point belongs to the dimmed backdrop and to no tracked entity.
    f.driver.click_point(5.0, 5.0);
    f.settle();
    assert!(
        !f.modal_open(),
        "a mouse-down outside the panel must close the quick-open modal"
    );

    let outcome = f.focus_invariant("after click-away closed quick-open");
    assert!(
        matches!(outcome, InvariantResult::Ok),
        "after click-away {FOCUS_INVARIANT} must measure and pass, got {outcome:?}"
    );
    assert_eq!(
        f.window_focused(),
        vec![f.target_element.clone()],
        "after click-away the previously-focused row's editor must hold window focus again"
    );
    assert!(
        f.keystroke_lands("q", Duration::from_secs(5)),
        "a character typed after click-away must land in the restored editor"
    );
    f.shutdown();
}

/// The destination's creation slot, which is the only editable row an EMPTY
/// destination has. `chord-target` is grafted childless on purpose.
fn destination_slot() -> String {
    // ALLOW(entity_uri_from_raw): the seed grafts this bare id; schemed here.
    holon_frontend::row_origin::RowOrigin::creation_placeholder_id(&EntityUri::from_raw(
        CHORD_TARGET_ID,
    ))
}

/// Enter on a hit navigates, so the row that held the caret UNMOUNTS and the
/// destination becomes the main region's view root. The destination itself
/// renders through the editor-less `page_title` variant, so seating the caret
/// on it would leave the keyboard dead — D97.a seats it on the destination's
/// first editable row instead.
///
/// This destination is empty, so that row is its creation SLOT: it takes the
/// caret without being born, and the first keystroke births through
/// `caret_block_for_edit`. Both halves are measured here — the seat (engine
/// focus, window focus, the oracle reaching `Ok`) and the birth (exactly one
/// real child, carrying the typed character) — because a seat that mounts no
/// live editor is indistinguishable from the bug it replaces.
#[test]
fn enter_navigating_out_of_quick_open_seats_a_caret() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenEnter");
    f.focus_target_row();
    f.open_quick_open_from_row();

    for ch in "chord".chars() {
        f.runtime
            .block_on(f.driver.send_raw_keystroke(&ch.to_string(), &[]))
            .expect("query characters must reach the overlay's input");
    }
    f.settle();
    f.runtime
        .block_on(f.driver.send_raw_keystroke("enter", &[]))
        .expect("enter must be consumed by the overlay's key handler");
    f.settle();

    assert!(
        !f.modal_open(),
        "enter on a hit must close the quick-open modal"
    );
    let slot = destination_slot();
    assert_eq!(
        f.engine.focused_block().map(|u| u.to_string()),
        Some(slot.clone()),
        "enter must seat the caret on the empty destination's creation slot, not on the \
         destination itself (which renders no editor)"
    );
    assert_eq!(
        f.window_focused(),
        vec![slot.clone()],
        "the seated row's editor must hold WINDOW focus too, else the keyboard is dead \
         despite the engine reporting a caret. Painted: {:?}",
        f.painted_entities()
    );
    let outcome = f.focus_invariant("after enter navigated out of quick-open");
    assert!(
        matches!(outcome, InvariantResult::Ok),
        "the destination now has a mounted editor, so {FOCUS_INVARIANT} must MEASURE and \
         pass rather than skip; got {outcome:?}"
    );
    assert_eq!(
        f.children_of(&f.target.to_string()),
        Vec::<(String, String)>::new(),
        "navigation alone must create nothing"
    );

    assert!(
        f.keystroke_lands("v", Duration::from_secs(5)),
        "a character typed straight after the jump must land in the seated editor"
    );
    f.settle();
    let born = f.children_of(&f.target.to_string());
    assert_eq!(
        born.len(),
        1,
        "the first keystroke must birth EXACTLY one block under the destination, got {born:?}"
    );
    assert_eq!(
        born[0].1, "v",
        "the newborn must carry the typed character, got {born:?}"
    );
    f.shutdown();
}

/// The slot caret must not leak a blank block: jumping into an empty page and
/// then navigating away again — without typing — leaves the store exactly as
/// it was. This is the half a "birth on focus" implementation gets wrong, and
/// a blank newborn survives into the org write-back as an empty headline.
#[test]
fn a_jump_into_an_empty_page_left_again_births_nothing() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenNoBirth");
    f.focus_target_row();
    f.open_quick_open_from_row();

    for ch in "chord".chars() {
        f.runtime
            .block_on(f.driver.send_raw_keystroke(&ch.to_string(), &[]))
            .expect("query characters must reach the overlay's input");
    }
    f.settle();
    f.runtime
        .block_on(f.driver.send_raw_keystroke("enter", &[]))
        .expect("enter must be consumed by the overlay's key handler");
    f.settle();
    assert_eq!(
        f.engine.focused_block().map(|u| u.to_string()),
        Some(destination_slot()),
        "the jump must seat the slot caret, else this rung proves nothing"
    );

    let before = f.runtime.block_on(f._env.non_page_block_rows()).len();

    f.runtime
        .block_on(async {
            f.engine
                .dispatch_intent_sync(holon_frontend::operations::OperationIntent::new(
                    "navigation".into(),
                    "go_home".to_string(),
                    [(
                        "region".to_string(),
                        holon_api::Value::String("main".into()),
                    )]
                    .into_iter()
                    .collect(),
                ))
                .await
        })
        .expect("navigate the main region away from the empty destination");
    f.settle();

    let after = f.runtime.block_on(f._env.non_page_block_rows()).len();
    assert_eq!(
        before, after,
        "leaving an empty destination the user never typed into must leave the block count \
         alone; a blank newborn here becomes an empty org headline on write-back"
    );
    f.shutdown();
}

/// Opening and closing quick-open with NO focused editor must be
/// keystroke-neutral: it neither grants focus the user never asked for nor
/// leaves a zombie behind. On a fresh window nothing is focused and a raw
/// character is already unconsumed — the overlay round-trip must not change
/// that verdict in either direction.
#[test]
fn quick_open_round_trip_without_a_focused_editor_is_neutral() {
    let mut f = Fixture::boot("Holon-TestPlatform-QuickOpenNoFocus");
    assert_eq!(
        f.engine.focused_block(),
        None,
        "fresh window: no engine focus"
    );
    let before = f.keystroke_lands("w", Duration::from_secs(3));

    f.runtime
        .block_on(f.driver.send_raw_keystroke("k", &["cmd"]))
        .expect("cmd+k must open quick-open even with nothing focused");
    f.settle();
    assert!(f.modal_open(), "cmd+k must open the quick-open modal");
    f.runtime
        .block_on(f.driver.send_raw_keystroke("escape", &[]))
        .expect("escape pressed into the open overlay");
    f.settle();
    assert!(!f.modal_open(), "escape must close the quick-open modal");

    assert_eq!(
        f.engine.focused_block(),
        None,
        "the round trip moved engine focus"
    );
    assert!(
        f.window_focused().is_empty(),
        "the round trip left a window-focused editor: {:?}",
        f.window_focused()
    );
    f.focus_invariant("after escape with nothing focused");
    let after = f.keystroke_lands("y", Duration::from_secs(3));
    assert_eq!(
        before, after,
        "the overlay round trip changed whether a keystroke lands"
    );
    f.shutdown();
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
