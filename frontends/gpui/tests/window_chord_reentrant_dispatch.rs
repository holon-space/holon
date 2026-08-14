//! Windowed regression for the dogfooded "cmd+k / cmd+] fail with `the window
//! is gone`" bug (dogfood P1 F2, 2026-08-08).
//!
//! Global `cx.on_action` handlers are invoked from inside
//! `Window::dispatch_action_on_node`, i.e. while GPUI has the window `take()`n
//! out of `App::windows` (app.rs `update_window_id`). A handler that then calls
//! `cx.update_window(wh, …)` re-enters that same slot, finds `None`, and gets
//! `Err("window not found")` — every time, on a perfectly live window. Undo and
//! redo escape it only because they hop through `cx.spawn`, so their window
//! update runs after the dispatch returns.
//!
//! Three chords, one per handler family, so a fix that repairs only the one
//! chord the dogfood pass happened to press cannot pass:
//!   * `cmd+]`  → `cycle_tab_next` (the `on_cycle!` macro),
//!   * `cmd+2`  → `jump_to_tab_2`  (the `on_jump!` macro),
//!   * `cmd+k`  → `open_search`    (its own handler, and the only one of the
//!     three that genuinely needs a `Window`).
//! `cmd+k` additionally asserts the user-visible effect — the modal is open —
//! so the journal cannot be satisfied by a handler that reports success while
//! doing nothing.
//!
//! Chords are read out of `window_key_bindings()`, the published registry, so a
//! registry/dispatch disagreement is part of what this bites on.
//!
//! Rung: the windowed `SimUserDriver` (TestPlatform, real gpui action
//! dispatch). No headless rung reaches GPUI's action dispatch tree.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::dispatch_journal::DispatchOutcome;
use holon_frontend::dispatch_journal::DispatchedIntent;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::window_key_bindings;
use holon_integration_tests::pbt::window_slice::seed::CHORD_TARGET_ID;
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
/// `send_key_chord` speaks. A segment outside that vocabulary is a bug in
/// `window_key_bindings`, which `publish_window_key_bindings` panics on too.
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

fn window_entry<'a>(dispatched: &'a [DispatchedIntent], action: &str) -> &'a DispatchedIntent {
    dispatched
        .iter()
        .find(|d| {
            d.entity_name == holon_frontend::dispatch_journal::WINDOW_REGISTRY
                && d.op_name == action
        })
        .unwrap_or_else(|| {
            panic!(
                "{action} must journal a window-registry entry; the press window held {:?}",
                dispatched
                    .iter()
                    .map(|d| format!("{}.{}", d.entity_name, d.op_name))
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn window_registry_chords_reach_their_window() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

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
                "Holon-TestPlatform-WindowChords",
                cx,
            )
        })
        .expect("window opened");

    // Precondition: the row must be PAINTED before a chord is pressed into it,
    // or the press would fail for a reason unrelated to the dispatch path.
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
        eprintln!(
            "[window-chord-boot] chord target not painted yet; {} elements",
            bounds.all_elements().len()
        );
    }
    assert!(
        painted,
        "boot precondition: the grafted chord target row must paint before any chord is pressed"
    );

    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    // SAFETY: `app` outlives the driver and stays on this (gpui) thread.
    let driver = SimUserDriver::new(
        &app,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );

    // ALLOW(entity_uri_from_raw): the seed grafts this bare id; schemed here.
    let target = EntityUri::from_raw(CHORD_TARGET_ID);
    let root_id = holon_api::root_layout_block_uri();
    let root_tree = engine.snapshot_reactive(&root_id);
    let journal = engine
        .dispatch_journal()
        .expect("the real engine journals its dispatches");

    engine.set_focus_with_caret(target.clone(), 0);
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let mut press = |chord: KeyChord| -> Vec<DispatchedIntent> {
        let mark = journal.mark();
        runtime
            .block_on(driver.send_key_chord(
                &root_id,
                &root_tree,
                &target,
                &chord,
                Default::default(),
            ))
            .unwrap_or_else(|e| panic!("{chord:?} pressed into the chord target row: {e}"));
        settle(&mut app, &bounds, &runtime, Duration::from_secs(30));
        journal.since(mark).expect("read back the dispatch journal")
    };

    // ── cmd+] — the `on_cycle!` family ───────────────────────────────────
    let dispatched = press(published_chord("cycle_tab_next"));
    let cycle = window_entry(&dispatched, "cycle_tab_next");
    assert_eq!(
        cycle.outcome,
        DispatchOutcome::Succeeded,
        "cmd+] must reach the window it was registered against"
    );

    // ── cmd+2 — the `on_jump!` family ────────────────────────────────────
    let dispatched = press(published_chord("jump_to_tab_2"));
    let jump = window_entry(&dispatched, "jump_to_tab_2");
    assert_eq!(
        jump.outcome,
        DispatchOutcome::Succeeded,
        "cmd+2 must reach the window it was registered against"
    );

    // ── cmd+k — its own handler, and the one that truly needs a `Window` ──
    // Pressed last: opening the modal moves window focus off the row.
    let dispatched = press(published_chord("open_search"));
    let search = window_entry(&dispatched, "open_search");
    assert_eq!(
        search.outcome,
        DispatchOutcome::Succeeded,
        "cmd+k must reach the window it was registered against"
    );
    // The effect, not just the report: a handler that settled Ok without
    // opening anything would still be the bug the user sees.
    let modal_open = app.update(|cx| rebind.search_modal_open(cx));
    assert!(
        modal_open,
        "cmd+k reported success but the quick-open modal is not open"
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
