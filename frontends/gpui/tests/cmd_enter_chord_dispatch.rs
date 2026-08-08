//! Windowed regression for the dogfooded "cmd+enter fires split_block, never
//! cycle_task_state" bug (dogfood P1-1, 2026-08-08).
//!
//! `gpui_component` binds `enter`, `shift-enter` and `secondary-enter` to ONE
//! action, `input::Enter { secondary }` — the modifier lives in the action
//! PAYLOAD, which GPUI has already parsed off the keystroke by the time a
//! capture handler runs. Holon's two `Enter` capture handlers instead asked
//! `window.modifiers()`, ambient state that only a `ModifiersChanged` platform
//! event maintains. Every simulated press therefore read "no cmd held" and fell
//! into the plain-Enter arm: a split, plus the empty junk block it leaves
//! behind, while the keymap kept reporting a `cycle_task_state` match.
//!
//! The test presses BOTH arms of the chord class through the same driver so it
//! cannot pass by making Enter inert:
//!   * `cmd+enter` must dispatch `cycle_task_state` and split nothing;
//!   * plain `enter` must still dispatch `split_block`.
//! The block count is read on either side of each press, so the user-visible
//! junk-block symptom is asserted directly and neither count assertion is
//! vacuous (the second press must move it).
//!
//! Rung: the windowed `SimUserDriver` (TestPlatform, real gpui action
//! dispatch). The keymap layer this bug lives in is `gpui_component`'s, so no
//! headless rung can reach it.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::TestApp;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
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
    app: &mut TestApp,
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

#[test]
fn cmd_enter_cycles_task_state_and_plain_enter_still_splits() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

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
                "Holon-TestPlatform-CmdEnter",
                cx,
            )
        })
        .expect("window opened");

    // Precondition, not a timing guess: the row must be PAINTED before a click
    // can seat a caret in it. A window that boots into the loading state needs
    // several settle rounds, and a test that pressed keys before the row
    // existed would fail for a reason that has nothing to do with the chord.
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
            "[cmd-enter-boot] chord target not painted yet; {} elements",
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

    // Chords come from the registry the app publishes, so a registry/dispatch
    // disagreement is what the test bites on — not a hardcoded guess.
    let chord_for = |op: &str| -> KeyChord {
        engine
            .key_bindings()
            .lock_ref()
            .get(op)
            .cloned()
            .unwrap_or_else(|| panic!("no keybinding registered for {op:?}"))
    };
    let cmd_enter = chord_for("cycle_task_state");
    assert!(
        cmd_enter.0.contains(&Key::Cmd) && cmd_enter.0.contains(&Key::Enter),
        "precondition: cycle_task_state is the cmd+enter chord, got {cmd_enter:?}"
    );

    // Seat engine focus directly rather than through a click: the driver then
    // takes its already-focused path and still waits for the row's editor to
    // take WINDOW focus, so the chord is pressed into a real mounted editor.
    // Click TARGETING is a separate concern with its own coverage; mixing it in
    // would let a click regression masquerade as a chord regression.
    engine.set_focus_with_caret(target.clone(), 0);
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let blocks = || runtime.block_on(env.non_page_block_rows()).len();
    let before_cmd_enter = blocks();

    // ── cmd+enter ────────────────────────────────────────────────────────
    let mark = journal.mark();
    runtime
        .block_on(driver.send_key_chord(
            &root_id,
            &root_tree,
            &target,
            &cmd_enter,
            Default::default(),
        ))
        .expect("cmd+enter pressed into the chord target row");
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let dispatched = journal.since(mark).expect("read back the dispatch journal");
    let ops: Vec<&str> = dispatched.iter().map(|d| d.op_name.as_str()).collect();
    assert!(
        ops.contains(&"cycle_task_state"),
        "cmd+enter must dispatch cycle_task_state; it dispatched {ops:?}"
    );
    assert!(
        !ops.contains(&"split_block"),
        "cmd+enter must not split — the cmd modifier was dropped and Enter's own binding ran \
         ({ops:?})"
    );
    assert_eq!(
        blocks(),
        before_cmd_enter,
        "cmd+enter created a block — the junk empty block the dogfood pass saw persisting across \
         restart"
    );

    // ── plain enter ──────────────────────────────────────────────────────
    // The counter-case: the fix must discriminate the chord, not disable Enter.
    let mark = journal.mark();
    runtime
        .block_on(driver.send_key_chord(
            &root_id,
            &root_tree,
            &target,
            &KeyChord::new(&[Key::Enter]),
            Default::default(),
        ))
        .expect("enter pressed into the chord target row");
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let dispatched = journal.since(mark).expect("read back the dispatch journal");
    let ops: Vec<&str> = dispatched.iter().map(|d| d.op_name.as_str()).collect();
    assert!(
        ops.contains(&"split_block"),
        "plain enter must still dispatch split_block; it dispatched {ops:?}"
    );
    // Proves the count assertion above is not vacuous: this gesture DOES move
    // the count, so a frozen counter cannot fake the cmd+enter result.
    assert_eq!(
        blocks(),
        before_cmd_enter + 1,
        "plain enter must add the split's second half"
    );

    // ── cmd+z: the WINDOW registry ───────────────────────────────────────
    // Undo runs `FrontendSession::undo` directly and never touches
    // `dispatch_intent`. A journal that only saw the structural registry
    // reported a working cmd+z as "nothing ran", so the reply built on it was
    // wrong in a new way. This asserts the window registry is journaled too.
    //
    // Re-seat focus first: the split above moved it to the block it minted,
    // and the driver would otherwise fall back to click-to-focus (the gap
    // this test routes around).
    engine.set_focus_with_caret(target.clone(), 0);
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));
    let mark = journal.mark();
    runtime
        .block_on(driver.send_key_chord(
            &root_id,
            &root_tree,
            &target,
            &KeyChord::new(&[Key::Cmd, Key::Char('z')]),
            Default::default(),
        ))
        .expect("cmd+z pressed into the chord target row");
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let dispatched = journal.since(mark).expect("read back the dispatch journal");
    let undo = dispatched
        .iter()
        .find(|d| d.op_name == "undo")
        .unwrap_or_else(|| {
            panic!(
                "cmd+z must journal the window-registry `undo` action; the press window held {:?}",
                dispatched
                    .iter()
                    .map(|d| format!("{}.{}", d.entity_name, d.op_name))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        undo.entity_name,
        holon_frontend::dispatch_journal::WINDOW_REGISTRY,
        "undo belongs to the window registry"
    );
    assert!(
        !undo.outcome.is_pending(),
        "cmd+z's undo never reported an outcome — a reply built on this would have to say pending, \
         not executed"
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}
