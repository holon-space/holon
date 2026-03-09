//! Mobile entry points for iOS and Android.
//!
//! These are activated only with `--features mobile` and compile only on their
//! respective target OS.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gpui::*;
use holon_frontend::theme::ThemeColors;
use holon_frontend::{FrontendConfig, FrontendSession};

use crate::geometry::BoundsRegistry;
use crate::state::AppState;

fn open_holon_window(cx: &mut App) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let rt_handle = rt.handle().clone();

    let (session, app_state, watch_handle) = rt.block_on(async {
        let widgets = crate::render_supported_widgets();
        let ui_info = holon_api::UiInfo {
            available_widgets: widgets,
            screen_size: None,
        };
        let config = FrontendConfig::new(ui_info);
        let session = Arc::new(
            FrontendSession::new(config)
                .await
                .expect("FrontendSession init failed"),
        );

        let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
        let app_state = AppState::new(holon_api::widget_spec::WidgetSpec::from_rows(vec![]));
        let watch = session
            .watch_ui(root_id, None, true)
            .await
            .expect("watch_ui failed");

        (session, app_state, watch)
    });

    // Keep runtime alive on a background thread
    std::thread::spawn(move || {
        rt.block_on(std::future::pending::<()>());
    });

    let theme = ThemeColors::default_dark();
    let bounds_registry = BoundsRegistry::new(theme.clone());
    let block_cache =
        holon_frontend::BlockRenderCache::new(Arc::clone(&session), rt_handle.clone());
    let cdc_state = app_state.clone_handle();

    let window_options = WindowOptions {
        window_bounds: None,
        ..Default::default()
    };

    match cx.open_window(window_options, |_window, cx| {
        cx.new(|_cx| crate::HolonApp {
            session: Arc::clone(&session),
            app_state,
            rt_handle: rt_handle.clone(),
            block_cache,
            bounds_registry,
            theme,
            show_settings: Arc::new(AtomicBool::new(false)),
        })
    }) {
        Ok(_handle) => {
            #[cfg(target_os = "android")]
            log::info!("Holon window opened successfully");
        }
        Err(e) => {
            #[cfg(target_os = "android")]
            log::error!("Failed to open Holon window: {e:#}");
            #[cfg(target_os = "ios")]
            eprintln!("Failed to open Holon window: {e:#}");
            return;
        }
    }

    cx.activate(true);

    // Listen for CDC events on a spawned task
    cx.spawn(async move |cx| {
        let mut watch_handle = watch_handle;
        while let Some(event) = watch_handle.recv().await {
            if crate::cdc::apply_event(&cdc_state, event) {
                // Trigger re-render — on mobile we just activate
                let _ = cx.update(|cx| cx.activate(true));
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .detach();
}

// ─── iOS ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn gpui_ios_register_app() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("GPUI PANIC: {info}");
    }));

    gpui_mobile::ios::ffi::set_app_callback(Box::new(|cx: &mut App| {
        open_holon_window(cx);
    }));
}

#[cfg(target_os = "ios")]
pub fn ios_main() {
    gpui_ios_register_app();
    gpui_mobile::ios::ffi::run_app();
}

// ─── Android ─────────────────────────────────────────────────────────────

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("holon-gpui"),
    );

    gpui_mobile::android::jni::install_panic_hook();
    log::info!("android_main: entered");

    let _platform = gpui_mobile::android::jni::init_platform(&app);
    log::info!("android_main: platform initialised");

    let shared = gpui_mobile::android::jni::shared_platform()
        .expect("shared_platform() returned None after init_platform");

    let gpui_app = Application::with_platform(std::rc::Rc::new(shared));
    gpui_app.run(|cx| {
        open_holon_window(cx);
    });
}
