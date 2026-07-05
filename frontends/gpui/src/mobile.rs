//! Mobile entry points for iOS and Android.
//!
//! These are activated only with `--features mobile` and compile only on their
//! respective target OS.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;
use holon_frontend::{FrontendSession, HolonConfig, SessionConfig};

use crate::geometry::BoundsRegistry;

fn open_holon_window(cx: &mut App, db_path: Option<PathBuf>, orgmode_root: Option<PathBuf>) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let rt_handle = rt.handle().clone();

    let session = rt.block_on(async {
        let widgets = crate::render_supported_widgets();
        let ui_info = holon_api::UiInfo {
            available_widgets: widgets,
            screen_size: None,
        };
        let mut holon_config = HolonConfig {
            db_path: db_path,
            vault: holon_frontend::config::VaultConfig { root: orgmode_root },
            ..Default::default()
        };
        // Mobile builds don't read `~/.config/holon/holon.toml`, so the
        // desktop-style opt-in (`[crdt] enabled = true`) doesn't apply here.
        // Without this, `LoroModule` is never configured → `Arc<LoroShareBackend>`
        // is never registered → share/accept ops fail with
        // "No provider registered for entity: tree". Mobile is a first-class
        // target for sharing, so enable the CRDT substrate unconditionally.
        holon_config.crdt.enabled = Some(true);
        let config_dir = holon_frontend::config::resolve_config_dir(None);
        let session_config = SessionConfig::new(ui_info);
        holon_app::new_from_config(
            holon_config,
            session_config,
            config_dir,
            std::collections::HashSet::new(),
        )
        .await
        .expect("FrontendSession init failed")
    });

    // Keep runtime alive on a background thread
    std::thread::spawn(move || {
        rt.block_on(std::future::pending::<()>());
    });

    let nav = crate::navigation_state::NavigationState::new();
    let bounds_registry = BoundsRegistry::new();
    crate::launch_holon_window_with_registry(session, rt_handle, nav, bounds_registry, cx);
}

// ─── iOS ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "ios")]
const DEFAULT_INDEX_ORG: &str = include_str!("../../../assets/default/index.org");
#[cfg(target_os = "ios")]
const DEFAULT_JOURNALS_ORG: &str = include_str!("../../../assets/default/Journals.org");

#[cfg(target_os = "ios")]
fn ios_data_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    // On iOS the app sandbox exposes a writable home directory; HOME points
    // at `…/data/Containers/Data/Application/<UUID>`. Put the DB inside
    // Library/ (not backed up to the cloud but persistent) and the org-mode
    // working copy inside Documents/ so the user sees it from the Files app.
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    let db_path = home.as_ref().map(|h| h.join("Library").join("holon.db"));
    let orgmode_root = home.as_ref().map(|h| h.join("Documents").join("holon-pkm"));
    if let Some(db) = db_path.as_ref() {
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent).expect("create Library dir for holon.db");
        }
    }
    if let Some(org) = orgmode_root.as_ref() {
        std::fs::create_dir_all(org).expect("create orgmode root dir");
        let is_empty = std::fs::read_dir(org)
            .expect("read orgmode root dir")
            .next()
            .is_none();
        if is_empty {
            let seed = org.join("index.org");
            std::fs::write(&seed, DEFAULT_INDEX_ORG).expect("write seed index.org");
            eprintln!("GPUI iOS: seeded {}", seed.display());
        }
        // Seed notes.org whenever it doesn't exist — independent of is_empty so
        // existing installs that only have index.org also get a visible document.
        // "index.org" is filtered from the sidebar (name == "index"), so without
        // this file the sidebar is always empty on a fresh install.
        let journals_path = org.join("Journals.org");
        if !journals_path.exists() {
            std::fs::write(&journals_path, DEFAULT_JOURNALS_ORG).expect("write seed Journals.org");
            eprintln!("GPUI iOS: seeded {}", journals_path.display());
        }
    }
    (db_path, orgmode_root)
}

#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn gpui_ios_register_app() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("GPUI PANIC: {info}");
    }));

    gpui_mobile::ios::ffi::set_app_callback(Box::new(|cx: &mut App| {
        let (db_path, orgmode_root) = ios_data_paths();
        eprintln!("GPUI iOS: db_path={db_path:?} orgmode_root={orgmode_root:?}");
        open_holon_window(cx, db_path, orgmode_root);
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

pub fn safe_area_top_px() -> f32 {
    #[cfg(target_os = "android")]
    {
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

pub fn safe_area_bottom_px() -> f32 {
    #[cfg(target_os = "android")]
    {
        return gpui_mobile::android::jni::platform()
            .and_then(|p| p.primary_window())
            .map(|w| w.safe_area_insets_logical().bottom)
            .unwrap_or(0.0);
    }
    #[cfg(target_os = "ios")]
    {
        let safe = gpui_mobile::safe_area_insets().1;
        let kb = gpui_mobile::keyboard_height();
        return safe.max(kb);
    }
    #[allow(unreachable_code)]
    0.0
}

// ─── Soft keyboard lifecycle ─────────────────────────────────────────────
//
// The platform keyboard must be up exactly while a text input owns focus.
// gpui delivers Blur/Focus in no guaranteed order on a block→block focus
// move (the zombie-editor blur can arrive AFTER the next editor's focus),
// so a naive hide-on-blur dismisses the keyboard mid-editing. Guard with a
// focus generation counter: every focus bumps it; a blur schedules a
// deferred hide that only fires if no focus arrived in the meantime.

use std::sync::atomic::{AtomicU64, Ordering};

static KEYBOARD_FOCUS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// How long a scheduled hide waits for a successor focus before firing.
/// One frame is enough for the mount→grab pipeline; 150ms adds margin for
/// slow re-renders (variant switch re-mounts the editor) without a user-
/// perceivable keyboard flicker window.
const KEYBOARD_HIDE_GRACE: std::time::Duration = std::time::Duration::from_millis(150);

/// A text input gained focus: bump the generation (cancels any pending
/// deferred hide) and raise the platform soft keyboard.
pub fn editor_focus_gained() {
    KEYBOARD_FOCUS_GENERATION.fetch_add(1, Ordering::SeqCst);
    tracing::debug!("soft keyboard: show (editor focus)");
    platform_show_keyboard();
}

/// A text input lost focus: schedule a deferred hide. If another input
/// gains focus within the grace window (generation moved), the hide is a
/// no-op and the keyboard stays up.
pub fn editor_focus_lost(cx: &mut App) {
    let generation = KEYBOARD_FOCUS_GENERATION.load(Ordering::SeqCst);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(KEYBOARD_HIDE_GRACE).await;
        if KEYBOARD_FOCUS_GENERATION.load(Ordering::SeqCst) == generation {
            tracing::debug!("soft keyboard: hide (editor blur, no refocus)");
            platform_hide_keyboard();
        } else {
            tracing::debug!("soft keyboard: hide skipped (focus moved to another input)");
        }
    })
    .detach();
}

fn platform_show_keyboard() {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::show_keyboard();
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    tracing::warn!(
        "soft keyboard show requested but this platform has no soft-keyboard backend \
         (mobile feature enabled on a desktop OS) — input continues via hardware keyboard"
    );
}

fn platform_hide_keyboard() {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::hide_keyboard();
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    tracing::warn!(
        "soft keyboard hide requested but this platform has no soft-keyboard backend \
         (mobile feature enabled on a desktop OS)"
    );
}
