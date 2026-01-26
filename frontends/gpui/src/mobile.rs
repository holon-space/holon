//! Mobile entry points for iOS and Android.
//!
//! These are activated only with `--features mobile` and compile only on their
//! respective target OS.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gpui::*;
use holon_frontend::{FrontendConfig, FrontendSession};

use crate::geometry::BoundsRegistry;
use crate::state::AppState;

fn open_holon_window(cx: &mut App, db_path: Option<PathBuf>, orgmode_root: Option<PathBuf>) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let rt_handle = rt.handle().clone();

    let (session, app_state) = rt.block_on(async {
        let widgets = crate::render_supported_widgets();
        let ui_info = holon_api::UiInfo {
            available_widgets: widgets,
            screen_size: None,
        };
        let mut config = FrontendConfig::new(ui_info);
        if let Some(db) = db_path {
            config = config.with_db_path(db);
        }
        if let Some(org) = orgmode_root {
            config = config.with_orgmode(org);
        }
        let session = Arc::new(
            FrontendSession::new(config)
                .await
                .expect("FrontendSession init failed"),
        );

        let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
        let watch = session
            .watch_ui(root_id, None, true)
            .await
            .expect("watch_ui failed");
        let app_state = holon_frontend::cdc::spawn_ui_listener(watch);

        (session, app_state)
    });

    // Keep runtime alive on a background thread
    std::thread::spawn(move || {
        rt.block_on(std::future::pending::<()>());
    });

    let bounds_registry = BoundsRegistry::new();
    let block_watch =
        holon_frontend::BlockWatchRegistry::new(Arc::clone(&session), rt_handle.clone());
    let mut cdc_state = app_state.clone();

    let window_options = WindowOptions {
        window_bounds: None,
        ..Default::default()
    };

    let view_slot: Arc<std::sync::OnceLock<Entity<crate::HolonApp>>> =
        Arc::new(std::sync::OnceLock::new());
    let view_slot_inner = Arc::clone(&view_slot);

    match cx.open_window(window_options, |_window, cx| {
        let view = cx.new(|_cx| crate::HolonApp {
            session: Arc::clone(&session),
            app_state,
            rt_handle: rt_handle.clone(),
            block_watch,
            bounds_registry,
            show_settings: Arc::new(AtomicBool::new(false)),
            safe_area_top: safe_area_top_px(),
        });
        view_slot_inner.set(view.clone()).ok();
        view
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
    };

    let view = view_slot
        .get()
        .expect("view must be set after open_window")
        .clone();

    cx.activate(true);

    // Periodic refresh for matview population after startup
    let refresh_view = view.clone();
    cx.spawn(async move |cx| {
        for _ in 0..10 {
            smol::Timer::after(std::time::Duration::from_secs(2)).await;
            let _ = cx.update(|cx| {
                refresh_view.update(cx, |_, cx| cx.notify());
            });
        }
        Ok::<_, anyhow::Error>(())
    })
    .detach();

    // Reactive CDC loop: wait for watch-channel notifications, then refresh GPUI
    cx.spawn(async move |cx| {
        while cdc_state.changed().await {
            let _ = cx.update(|cx| {
                view.update(cx, |_, cx| cx.notify());
            });
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
        // TODO: resolve iOS data paths
        open_holon_window(cx, None, None);
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

    let internal = app.internal_data_path();
    let external = app.external_data_path();
    log::info!("android_main: internal_data_path={internal:?}, external_data_path={external:?}");

    let db_path = internal.map(|p| p.join("holon.db"));
    let orgmode_root = external.map(|p| p.join("holon-pkm"));
    log::info!("android_main: db_path={db_path:?}, orgmode_root={orgmode_root:?}");

    let _platform = gpui_mobile::android::jni::init_platform(&app);
    log::info!("android_main: platform initialised");

    let shared = gpui_mobile::android::jni::shared_platform()
        .expect("shared_platform() returned None after init_platform");

    let gpui_app = Application::with_platform(std::rc::Rc::new(shared));
    gpui_app.run(|cx| {
        open_holon_window(cx, db_path, orgmode_root);
    });
}

fn safe_area_top_px() -> f32 {
    #[cfg(target_os = "android")]
    {
        // Android 15+ edge-to-edge: content_rect covers the full window,
        // so gpui-mobile reports 0 insets. Fall back to 32dp (standard
        // status bar height) when the platform reports nothing.
        let from_platform = gpui_mobile::android::jni::platform()
            .and_then(|p| p.primary_window())
            .map(|w| w.safe_area_insets_logical().top)
            .unwrap_or(0.0);
        return from_platform.max(32.0);
    }
    #[cfg(target_os = "ios")]
    {
        return gpui_mobile::safe_area_insets().0.max(20.0);
    }
    #[allow(unreachable_code)]
    0.0
}
