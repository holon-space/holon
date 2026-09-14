//! The toast stack is painted in the order the conditions were RAISED.
//!
//! The bus holds its conditions in a `LiveData` keyed `subject` first, so the
//! holder's own order is alphabetical. Replay order is what the reader sees as
//! stack order, and the replay is what a window that opens AFTER a degradation
//! is fed from. A stack ordered by subject name is not the order anything
//! happened in: the newest failure can land above the oldest, and nothing on
//! screen says why.
//!
//! The two conditions here are raised in the order OPPOSITE to how their
//! subjects sort, so key order and raise order disagree and only one of them
//! can pass. They are also raised BEFORE the window opens, which is what makes
//! the rung bite: the stack is then built from the subscription's REPLAY, the
//! one path the raise-order sort governs. Conditions raised after the window
//! exists arrive as live broadcasts and are appended in arrival order whatever
//! the replay does. `condition_replay_order.rs` pins the same law on the bus
//! itself; this pins that the painted stack inherits it.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! toast_stack_order_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind windowed
//! @pbt covers toast-stack-follows-raise-order
//! @pbt slips-if-removed the stack silently reorders itself alphabetically by
//! subject, so which failure looks most recent depends on its file name

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::Condition;
use holon_api::ConditionKind;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::share_ui::TOAST_LINE;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

const WINDOW: &str = "1512x900";

/// Raised FIRST, and sorts LAST. Both conditions are the same kind, so the
/// subject is the only thing that can order them and the two orders cannot
/// coincide by luck.
const FIRST_RAISED: &str = "zeta-notes.org";
/// Raised SECOND, and sorts FIRST.
const SECOND_RAISED: &str = "alpha-notes.org";

fn ingest_failure(subject: &str) -> Condition {
    Condition {
        subject: subject.to_string(),
        reason: ConditionKind::VaultIngestFailed {
            format: "org".to_string(),
            reason: "unreadable".to_string(),
        },
    }
}

#[test]
fn the_toast_stack_follows_raise_order_not_subject_order() {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", WINDOW) };
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

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
    let frontend = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted frontend component");
    let bus = frontend
        .degraded_bus()
        .expect("the booted session's degraded bus");

    // Raised BEFORE the window exists, over the session's own bus, so the
    // stack is built from the subscription's REPLAY — the path the raise-order
    // sort governs. Raising after the window opens would instead arrive as two
    // live broadcasts, which the toast list appends in arrival order no matter
    // how the replay is sorted, and the rung would have no teeth.
    bus.emit(ingest_failure(FIRST_RAISED));
    bus.emit(ingest_failure(SECOND_RAISED));

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
                Some(bus.clone()),
                "Holon-ToastStackOrder-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let lines: Vec<(String, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(id, _)| id.starts_with(TOAST_LINE))
        .collect();

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);

    assert!(
        !lines.is_empty(),
        "no toast line was painted at all, so an ordering assertion would be vacuous"
    );

    // The stack grows downward, so the y of a toast's line is its rank.
    let y_of = |subject: &str| -> f32 {
        lines
            .iter()
            .filter(|(_, info)| {
                info.displayed_text
                    .as_deref()
                    .is_some_and(|t| t.contains(subject))
            })
            .map(|(_, info)| info.y)
            .fold(f32::INFINITY, f32::min)
    };

    let first = y_of(FIRST_RAISED);
    let second = y_of(SECOND_RAISED);
    let painted: Vec<&str> = lines
        .iter()
        .filter_map(|(_, info)| info.displayed_text.as_deref())
        .collect();

    assert!(
        first.is_finite(),
        "the first-raised condition `{FIRST_RAISED}` painted no line. Painted: {painted:?}"
    );
    assert!(
        second.is_finite(),
        "the second-raised condition `{SECOND_RAISED}` painted no line. Painted: {painted:?}"
    );
    assert!(
        first < second,
        "`{FIRST_RAISED}` was raised FIRST and must sit above `{SECOND_RAISED}`, but painted at \
         y={first} against y={second} — the stack is ordered by subject name, not by when the \
         degradations happened. Painted: {painted:?}"
    );
}
