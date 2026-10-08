//! A condition placed in a banner that this window has no banner for is still
//! painted: ADR 0035 has a frontend without the declared surface fall back to
//! a toast (docs/Architecture/Model.md, "Conditions").
//!
//! @pbt kind windowed
//! @pbt covers degraded-disclosure-banner-fallback — every Banner condition
//! without its own GPUI surface is painted in the toast stack, once per
//! condition (BugFunnel
//! 2026-10-08-gpui-drops-banner-conditions-without-a-surface) @pbt overlaps
//! general_e2e_composed_pbt — kept: the keystone is headless and paints nothing

use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::TestApp;
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_with_engine_and_share;
use holon_gpui::share_ui::DEGRADED_TOAST_STACK;
use holon_integration_tests::test_environment::TestEnvironment;

fn banner_conditions() -> Vec<Condition> {
    vec![
        Condition {
            subject: "prior.rs:1:1".to_string(),
            reason: ConditionKind::PreviousRunPanicked {
                message: "the previous run".to_string(),
                thread: "main".to_string(),
            },
        },
        Condition {
            subject: "now.rs:2:2".to_string(),
            reason: ConditionKind::TaskPanicked {
                message: "a task".to_string(),
                thread: "worker".to_string(),
            },
        },
        Condition {
            subject: "database#1".to_string(),
            reason: ConditionKind::DatabaseStuck {
                command: "Transaction".to_string(),
                running_secs: 31,
                report: "stuck".to_string(),
            },
        },
        Condition {
            subject: "views".to_string(),
            reason: ConditionKind::ViewEngineStopped("the first error".to_string()),
        },
    ]
}

fn draw(app: &mut TestApp, runtime: &tokio::runtime::Runtime, bounds: &BoundsRegistry) {
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
    app.run_until_parked();
    app.update(|cx| {
        for window in cx.windows() {
            window
                .update(cx, |_, window, cx| window.draw(cx).clear(cx))
                .expect("draw the window");
        }
    });
    bounds.flush();
}

#[test]
fn banner_conditions_without_a_banner_surface_are_painted_as_one_toast_each() {
    let mut app = TestApp::with_text_system_and_assets(
        gpui_platform::current_text_system(),
        Arc::new(()) as Arc<dyn AssetSource>,
    );
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    env.set_enable_loro(false);
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });
    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");
    let bus = Arc::new(ConditionBus::new());

    let bounds = app.update(|cx| {
        launch_holon_window_with_engine_and_share(
            session,
            engine,
            debug_services,
            None,
            bus.clone(),
            runtime.handle().clone(),
            cx,
        )
    });
    let toast = Condition {
        subject: "integration:probe".to_string(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: "probe".to_string(),
            error: "sidecar not found".to_string(),
        },
    };
    bus.emit(toast.clone());
    for condition in banner_conditions() {
        bus.emit(condition.clone());
        bus.emit(condition);
    }
    draw(&mut app, &runtime, &bounds);

    let painted = bounds
        .element_info(DEGRADED_TOAST_STACK)
        .and_then(|info| info.displayed_text)
        .map(|text| text.to_string())
        .unwrap_or_default();
    assert!(
        painted.contains(toast.reason.profile().label()),
        "a toast-placed condition must be painted; the stack reads {painted:?}"
    );
    for condition in banner_conditions() {
        let headline = format!("{} — ", condition.reason.profile().label());
        assert_eq!(
            painted.matches(&headline).count(),
            1,
            "{} must be painted once in the toast stack; it reads {painted:?}",
            condition.reason.condition_kind()
        );
    }

    std::mem::forget(app);
    std::mem::forget(env);
}

mod test_init;
